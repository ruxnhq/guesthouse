//! Pictures out of the guest: a buffer handed to a viewer as a file, and frames copied between
//! surfaces in the daemon's own address space.
//!
//! Frames in rows at both ends are copied here by the CPU, through its view of each end. That is
//! right for any memory and quick for the guest's own pages, where Intel's drivers keep nearly
//! everything. For a picture that stays in the card's memory it is slow, as the CPU reads that
//! memory uncached, and a copy by the card's copy engine would be the better way;
//! [`crate::xe::batch::copy_rows`] is its instruction, and nothing sends it yet. A frame in tiles
//! at either end already goes by the copy engine: see [`super::blits`].

use super::CardWork;
use crate::xe::plan::Backing;
use guesthouse_backend::{BackendError, SlotCopy, SpaceId, StagingCopy};
use std::os::fd::OwnedFd;

/// Rows to copy from one GPU address to another in one space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rows {
    pub source: u64,
    pub target: u64,
    /// The bytes of each row that are copied.
    pub row_bytes: u64,
    pub rows: u64,
    /// The bytes from one row to the next at each end.
    pub source_pitch: u64,
    pub target_pitch: u64,
}

impl Rows {
    /// The bytes from an end's first to its last, or `None` if there is nothing to copy or the
    /// numbers don't add up.
    fn extent(&self, pitch: u64) -> Option<u64> {
        if self.rows == 0 || self.row_bytes == 0 || pitch < self.row_bytes {
            return None;
        }
        (self.rows - 1).checked_mul(pitch)?.checked_add(self.row_bytes)
    }

    /// A whole picture into a slot of the frame ring. `None` for a slot laid out in tiles, which
    /// nothing here writes, or for numbers that don't add up.
    pub fn of_slot(copy: &SlotCopy, pixel_bytes: u32) -> Option<Self> {
        if copy.slot_tiling.is_some() || copy.source.tiling.is_some() {
            return None;
        }
        Some(Self {
            source: copy.source_address,
            target: copy.slot_address,
            row_bytes: u64::from(copy.width).checked_mul(pixel_bytes.into())?,
            rows: copy.height.into(),
            source_pitch: copy.source.pitch.into(),
            target_pitch: copy.slot_pitch.into(),
        })
    }

    /// A rectangle of one surface into another. `None` for a tiled surface, or a rectangle that
    /// isn't inside both.
    pub fn of_staging(copy: &StagingCopy, pixel_bytes: u32) -> Option<Self> {
        if copy.source.tiling.is_some() || copy.target.tiling.is_some() {
            return None;
        }
        let pixel = u64::from(pixel_bytes);
        let (left, top) = (u64::try_from(copy.left).ok()?, u64::try_from(copy.top).ok()?);
        let (right, bottom) = (u64::try_from(copy.right).ok()?, u64::try_from(copy.bottom).ok()?);
        let (target_left, target_top) =
            (u64::try_from(copy.target_left).ok()?, u64::try_from(copy.target_top).ok()?);
        let (width, height) = (right.checked_sub(left)?, bottom.checked_sub(top)?);
        let inside = right <= copy.source.width.into()
            && bottom <= copy.source.height.into()
            && target_left + width <= copy.target.width.into()
            && target_top + height <= copy.target.height.into();
        if !inside {
            return None;
        }
        let (source_pitch, target_pitch) = (u64::from(copy.source.pitch), u64::from(copy.target.pitch));
        Some(Self {
            source: copy.source_address + top * source_pitch + left * pixel,
            target: copy.target_address + target_top * target_pitch + target_left * pixel,
            row_bytes: width * pixel,
            rows: height,
            source_pitch,
            target_pitch,
        })
    }
}

impl CardWork {
    /// Hands a buffer out as a file a viewer on the same card can import. The guest's own pages
    /// are no buffer of the card's, so they can't be.
    pub fn export(&mut self, memory: guesthouse_backend::MemoryId) -> Result<OwnedFd, BackendError> {
        match self.blocks.get(&memory).map(|block| block.backing) {
            Some(Backing::Buffer { handle, .. }) => self
                .xe
                .export_buffer(handle)
                .map_err(|error| BackendError::new(format!("xe exported no buffer: {error}"))),
            Some(Backing::HostPages { .. }) => {
                Err(BackendError::new("the guest's own pages are no buffer of the card's to export"))
            }
            None => Err(BackendError::new("exporting memory that does not exist")),
        }
    }

    /// Copies rows between two surfaces mapped in a space, by the CPU. Each end must lie wholly
    /// inside one mapping.
    pub fn copy_rows(&mut self, space: SpaceId, rows: &Rows) -> Result<(), BackendError> {
        let wrong = || BackendError::new("a copy whose rows and pitches don't add up");
        let source_extent = rows.extent(rows.source_pitch).ok_or_else(wrong)?;
        let target_extent = rows.extent(rows.target_pitch).ok_or_else(wrong)?;
        let held = self.spaces.get(&space).ok_or(BackendError::new("a copy in a space that is closed"))?;
        let unmapped = |what: &str, address: u64| {
            BackendError::new(format!("a copy's {what} at {address:#x} isn't all inside one mapping"))
        };
        let (from, from_offset) =
            held.holding(rows.source, source_extent).ok_or_else(|| unmapped("source", rows.source))?;
        let (to, to_offset) =
            held.holding(rows.target, target_extent).ok_or_else(|| unmapped("target", rows.target))?;
        let mut row = vec![0u8; rows.row_bytes as usize];
        for index in 0..rows.rows {
            if !self.cpu_view(from)?.read(from_offset + index * rows.source_pitch, &mut row) {
                return Err(BackendError::new("a copy's source is smaller than its mapping says"));
            }
            if !self.cpu_view(to)?.write(to_offset + index * rows.target_pitch, &row) {
                return Err(BackendError::new("a copy's target is smaller than its mapping says"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stage::Stage;
    use crate::work::testing::{self, Pages, map_request};
    use crate::xe::plan::BufferUse;
    use guesthouse_backend::{GPU_RANGES, SurfaceShape};

    fn surface(width: u32, height: u32, pitch: u32) -> SurfaceShape {
        SurfaceShape {
            width,
            height,
            format: 0,
            pitch,
            tiling: None,
            refresh: (0, 1),
            samples: (1, 0),
            colour: 0,
        }
    }

    const SOURCE_AT: u64 = 0x400_0000;
    const TARGET_AT: u64 = 0x500_0000;

    #[test]
    fn a_frame_in_guest_pages_is_copied_row_by_row_into_a_slot_with_another_pitch() {
        let (mut work, xe) = testing::work(Stage::Everything);
        let space = work.create_space(&GPU_RANGES).unwrap();
        let (frame, slot) = (Pages::new(0x1000), Pages::new(0x1000));
        let picture: Vec<u8> = (0..0x1000u32).map(|byte| (byte % 251) as u8).collect();
        frame.write(0, &picture);
        let from = work.adopt(frame.address(), 0x1000).unwrap();
        let to = work.adopt(slot.address(), 0x1000).unwrap();
        work.map(&map_request(space, from, SOURCE_AT, 0, 0x1000)).unwrap();
        work.map(&map_request(space, to, TARGET_AT, 0, 0x1000)).unwrap();
        // 8 pixels by 4 rows: 64 bytes a row at the source, 32 at the slot.
        let copy = SlotCopy {
            source: surface(8, 4, 64),
            source_address: SOURCE_AT,
            slot_address: TARGET_AT,
            slot_pitch: 32,
            width: 8,
            height: 4,
            own_space: space,
            slot_tiling: None,
        };
        let rows = Rows::of_slot(&copy, 4).unwrap();
        let calls = xe.calls().len();
        work.copy_rows(space, &rows).unwrap();
        assert_eq!(xe.calls().len(), calls, "the CPU did it: nothing was asked of xe");
        let copied = slot.bytes();
        for row in 0..4 {
            assert_eq!(copied[row * 32..row * 32 + 32], picture[row * 64..row * 64 + 32], "row {row}");
        }
        assert!(copied[4 * 32..].iter().all(|&byte| byte == 0), "nothing past the last row");
    }

    #[test]
    fn a_frame_in_card_memory_is_read_through_the_cpus_view() {
        let (mut work, _xe) = testing::work(Stage::Everything);
        let space = work.create_space(&GPU_RANGES).unwrap();
        let (from, _) = work.allocate(0x1000, BufferUse::Card).unwrap();
        let slot = Pages::new(0x1000);
        let to = work.adopt(slot.address(), 0x1000).unwrap();
        work.map(&map_request(space, from, SOURCE_AT, 0, 0x1000)).unwrap();
        work.map(&map_request(space, to, TARGET_AT, 0, 0x1000)).unwrap();
        work.write_for_test(from, &[7u8; 0x1000]);
        let rows = Rows {
            source: SOURCE_AT,
            target: TARGET_AT,
            row_bytes: 16,
            rows: 2,
            source_pitch: 16,
            target_pitch: 16,
        };
        work.copy_rows(space, &rows).unwrap();
        assert_eq!(slot.bytes()[..32], [7u8; 32]);
    }

    #[test]
    fn a_copy_that_leaves_its_mapping_or_does_not_add_up_is_refused() {
        let (mut work, _xe) = testing::work(Stage::Everything);
        let space = work.create_space(&GPU_RANGES).unwrap();
        let pages = Pages::new(0x2000);
        let memory = work.adopt(pages.address(), 0x2000).unwrap();
        work.map(&map_request(space, memory, SOURCE_AT, 0, 0x1000)).unwrap();
        work.map(&map_request(space, memory, TARGET_AT, 0x1000, 0x1000)).unwrap();
        let rows = |row_bytes, rows, pitch| Rows {
            source: SOURCE_AT,
            target: TARGET_AT,
            row_bytes,
            rows,
            source_pitch: pitch,
            target_pitch: pitch,
        };
        assert!(work.copy_rows(space, &rows(64, 64, 64)).is_ok(), "exactly the mapping");
        assert!(work.copy_rows(space, &rows(64, 65, 64)).is_err(), "one row past it");
        assert!(work.copy_rows(space, &rows(64, 2, 32)).is_err(), "rows that overlap");
        assert!(work.copy_rows(space, &rows(0, 2, 32)).is_err());
        assert!(work.copy_rows(space, &rows(u64::MAX, u64::MAX, u64::MAX)).is_err());
        assert!(work.copy_rows(SpaceId(99), &rows(64, 1, 64)).is_err());
    }

    #[test]
    fn a_rectangle_is_placed_by_its_corners_and_one_outside_a_surface_is_refused() {
        let copy = StagingCopy {
            source: surface(100, 50, 400),
            target: surface(200, 80, 800),
            source_address: 0x1000_0000,
            target_address: 0x2000_0000,
            left: 10,
            top: 5,
            right: 30,
            bottom: 25,
            target_left: 100,
            target_top: 40,
            target_guest_address: None,
            is_screen: true,
            own_space: SpaceId(1),
        };
        let rows = Rows::of_staging(&copy, 4).unwrap();
        assert_eq!(rows.source, 0x1000_0000 + 5 * 400 + 10 * 4);
        assert_eq!(rows.target, 0x2000_0000 + 40 * 800 + 100 * 4);
        assert_eq!((rows.row_bytes, rows.rows, rows.source_pitch, rows.target_pitch), (80, 20, 400, 800));
        assert_eq!(Rows::of_staging(&StagingCopy { right: 101, ..copy }, 4), None);
        assert_eq!(Rows::of_staging(&StagingCopy { left: -1, ..copy }, 4), None);
        assert_eq!(Rows::of_staging(&StagingCopy { target_top: 61, ..copy }, 4), None);
        assert_eq!(Rows::of_staging(&StagingCopy { right: 5, ..copy }, 4), None, "right of its own left");
        let tiled = SurfaceShape { tiling: Some(4), ..copy.source };
        assert_eq!(Rows::of_staging(&StagingCopy { source: tiled, ..copy }, 4), None);
    }

    #[test]
    fn a_buffer_is_exported_and_guest_pages_are_not() {
        let (mut work, xe) = testing::work(Stage::Everything);
        let (buffer, _) = work.allocate(0x1000, BufferUse::Card).unwrap();
        let pages = Pages::new(0x1000);
        let guest = work.adopt(pages.address(), 0x1000).unwrap();
        work.export(buffer).unwrap();
        assert_eq!(xe.calls_named("export_buffer").len(), 1);
        assert!(work.export(guest).is_err());
        assert!(work.export(guesthouse_backend::MemoryId(99)).is_err());
        assert_eq!(xe.calls_named("export_buffer").len(), 1);
    }
}
