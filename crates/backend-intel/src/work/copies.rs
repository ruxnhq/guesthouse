//! Pictures out of the guest: a buffer handed to a viewer as a file, and frames copied between
//! surfaces in the daemon's own address space.
//!
//! Every frame goes by the card's copy engine where the engine can be given it: see
//! [`super::blits`]. The CPU copies here only frames in rows at both ends that the engine can't be
//! given, through its view of each end. The CPU reads the card's memory uncached, at well under a
//! gigabyte a second, and the daemon answers the guest on the thread that copies, so a frame by
//! the CPU holds the guest up for milliseconds.
//!
//! The time each kind of copy takes is added up and said every [`TALLY_EVERY`] at most, while
//! copies happen.

use super::CardWork;
use crate::xe::plan::Backing;
use guesthouse_backend::{BackendError, SlotCopy, SpaceId, StagingCopy, SurfaceShape};
use std::os::fd::OwnedFd;
use std::time::{Duration, Instant};

/// How often the copies' times are said, at most.
pub const TALLY_EVERY: Duration = Duration::from_secs(10);

/// The kinds of work on a frame that are timed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyKind {
    /// Rows copied by the CPU.
    Cpu,
    /// A copy by the copy engine, from when it was sent to when it was seen to end.
    Engine,
    /// Red and blue changed over by the CPU after a copy.
    Colours,
    /// An allocation's contents moved into the guest's pages on its first lock, by the copy engine.
    MoveByEngine,
    /// The same, by the CPU.
    MoveByCpu,
}

/// How many, how long in all, and the longest.
#[derive(Debug, Clone, Copy, Default)]
struct Tally {
    count: u64,
    total: Duration,
    longest: Duration,
}

impl Tally {
    fn add(&mut self, taken: Duration) {
        self.count += 1;
        self.total += taken;
        self.longest = self.longest.max(taken);
    }
}

impl std::fmt::Display for Tally {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mean = self.total.checked_div(self.count.max(1) as u32).unwrap_or_default();
        write!(
            f,
            "{} taking {:.1} ms in all, {:.2} ms on average and {:.2} ms at most",
            self.count,
            self.total.as_secs_f64() * 1000.0,
            mean.as_secs_f64() * 1000.0,
            self.longest.as_secs_f64() * 1000.0
        )
    }
}

/// The time frames took since it was last said.
#[derive(Debug, Default)]
pub struct CopyTally {
    since: Option<Instant>,
    cpu: Tally,
    engine: Tally,
    colours: Tally,
    moved_by_engine: Tally,
    moved_by_cpu: Tally,
}

impl CopyTally {
    /// Adds one, and gives the line to say if [`TALLY_EVERY`] has passed since the last.
    pub fn add(&mut self, kind: CopyKind, taken: Duration, now: Instant) -> Option<String> {
        let since = *self.since.get_or_insert(now);
        match kind {
            CopyKind::Cpu => self.cpu.add(taken),
            CopyKind::Engine => self.engine.add(taken),
            CopyKind::Colours => self.colours.add(taken),
            CopyKind::MoveByEngine => self.moved_by_engine.add(taken),
            CopyKind::MoveByCpu => self.moved_by_cpu.add(taken),
        }
        if now.saturating_duration_since(since) < TALLY_EVERY {
            return None;
        }
        let line = format!(
            "intel work: frames in the last {:.1} s: by the copy engine {}; by the CPU {}; red and blue \
             changed over {}; first locks moved by the copy engine {}; by the CPU {}",
            now.saturating_duration_since(since).as_secs_f64(),
            self.engine,
            self.cpu,
            self.colours,
            self.moved_by_engine,
            self.moved_by_cpu
        );
        *self = Self { since: Some(now), ..Self::default() };
        Some(line)
    }
}

/// gmmlib's formats with red first in memory, as a Direct3D 11 `R8G8B8A8` swap chain's back buffer
/// has: `R8G8B8A8_UNORM` and `R8G8B8A8_UNORM_SRGB`, counted in gmmlib's `GmmFormatTable.h` from 1.
const RED_FIRST_FORMATS: [u32; 2] = [0x7E, 0x7F];
/// The Direct3D formats of the kernel's staging surface, `A8R8G8B8` and `X8R8G8B8`, which are blue
/// first in memory.
const BLUE_FIRST_FORMATS: [u32; 2] = [21, 22];

/// Whether a copy goes from a surface of Intel's drivers with red first to the kernel's staging
/// surface, which has blue first, as a windowed present from a Direct3D 11 `R8G8B8A8` back buffer
/// does. Bytes moved as they are would show red and blue changed over, and neither the CPU's rows
/// nor the copy engine change them on the way. Intel's surfaces leave the Direct3D format unknown
/// and give gmmlib's in their colour; the kernel's give a Direct3D format and the standard colour.
pub fn colours_turn(source: &SurfaceShape, target: &SurfaceShape) -> bool {
    source.format == 0
        && RED_FIRST_FORMATS.contains(&source.colour)
        && BLUE_FIRST_FORMATS.contains(&target.format)
        && target.colour == 0
        && target.tiling.is_none()
}

/// Rows of a surface in one space: where the first starts, how many bytes of each, how many, and
/// how far apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Area {
    pub address: u64,
    pub row_bytes: u64,
    pub rows: u64,
    pub pitch: u64,
}

impl Area {
    /// Where a staging copy's rectangle lands in its target. `None` for a target in tiles, or
    /// numbers that don't add up.
    pub fn written_by(copy: &StagingCopy, pixel_bytes: u32) -> Option<Self> {
        if copy.target.tiling.is_some() {
            return None;
        }
        let pixel = u64::from(pixel_bytes);
        let width = u64::try_from(copy.right.checked_sub(copy.left)?).ok()?;
        let rows = u64::try_from(copy.bottom.checked_sub(copy.top)?).ok()?;
        let (left, top) = (u64::try_from(copy.target_left).ok()?, u64::try_from(copy.target_top).ok()?);
        let pitch = u64::from(copy.target.pitch);
        let address = copy
            .target_address
            .checked_add(top.checked_mul(pitch)?)?
            .checked_add(left.checked_mul(pixel)?)?;
        Some(Self { address, row_bytes: width.checked_mul(pixel)?, rows, pitch })
    }
}

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
        let started = Instant::now();
        let copied = self.copy_rows_now(space, rows);
        if copied.is_ok() {
            self.note_copy(CopyKind::Cpu, started.elapsed());
        }
        copied
    }

    /// Adds a frame's time to the tally, and says the tally when its time has come.
    pub(super) fn note_copy(&mut self, kind: CopyKind, taken: Duration) {
        if let Some(line) = self.blits.tally.add(kind, taken, Instant::now()) {
            log::info!("{line}");
        }
    }

    fn copy_rows_now(&mut self, space: SpaceId, rows: &Rows) -> Result<(), BackendError> {
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

    /// Changes red and blue over in every pixel of some rows mapped in a space, by the CPU: the
    /// first and third byte of each four. The rows must lie wholly inside one mapping. See
    /// [`colours_turn`] for when.
    pub fn turn_colours(&mut self, space: SpaceId, area: &Area) -> Result<(), BackendError> {
        if area.rows == 0 || area.row_bytes == 0 {
            return Ok(());
        }
        let wrong = || BackendError::new("rows to turn whose sizes don't add up");
        if area.pitch < area.row_bytes || !area.row_bytes.is_multiple_of(4) {
            return Err(wrong());
        }
        let extent = (area.rows - 1)
            .checked_mul(area.pitch)
            .and_then(|bytes| bytes.checked_add(area.row_bytes))
            .ok_or_else(wrong)?;
        let held =
            self.spaces.get(&space).ok_or(BackendError::new("turning colours in a space that is closed"))?;
        let (memory, offset) = held.holding(area.address, extent).ok_or_else(|| {
            BackendError::new(format!("rows to turn at {:#x} aren't all inside one mapping", area.address))
        })?;
        let started = Instant::now();
        let view = self.cpu_view(memory)?;
        let row_bytes = area.row_bytes as usize;
        for index in 0..area.rows {
            if !view.turn_red_and_blue(offset + index * area.pitch, row_bytes) {
                return Err(BackendError::new("rows to turn lie past their memory"));
            }
        }
        self.note_copy(CopyKind::Colours, started.elapsed());
        if !self.blits.turned_said {
            self.blits.turned_said = true;
            log::info!(
                "intel work: a copy from a surface with red first into the kernel's staging surface, which \
                 has blue first, has the two changed over, as each such copy will"
            );
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

    #[test]
    fn the_times_of_frames_are_said_once_in_ten_seconds_and_then_start_again() {
        let mut tally = CopyTally::default();
        let start = Instant::now();
        assert_eq!(tally.add(CopyKind::Engine, Duration::from_micros(200), start), None);
        assert_eq!(tally.add(CopyKind::Cpu, Duration::from_millis(14), start + Duration::from_secs(3)), None);
        let line = tally.add(CopyKind::Colours, Duration::from_millis(1), start + TALLY_EVERY).unwrap();
        assert!(line.contains("in the last 10.0 s"), "{line}");
        assert!(line.contains("by the copy engine 1 taking 0.2 ms in all"), "{line}");
        assert!(line.contains("by the CPU 1 taking 14.0 ms in all, 14.00 ms on average"), "{line}");
        assert!(line.contains("changed over 1 taking 1.0 ms"), "{line}");
        assert!(line.contains("first locks moved by the copy engine 0 taking"), "{line}");
        assert!(
            line.ends_with("by the CPU 0 taking 0.0 ms in all, 0.00 ms on average and 0.00 ms at most"),
            "{line}"
        );
        let later = start + TALLY_EVERY + Duration::from_secs(1);
        assert_eq!(tally.add(CopyKind::Engine, Duration::from_micros(200), later), None, "ten seconds anew");
        let line = tally.add(CopyKind::Engine, Duration::from_micros(100), start + TALLY_EVERY * 2).unwrap();
        assert!(
            line.contains("by the copy engine 2 taking 0.3 ms") && line.contains("by the CPU 0 taking"),
            "{line}"
        );
    }

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
    fn only_a_red_first_surface_of_intels_into_the_kernels_staging_surface_has_its_colours_changed_over() {
        let red_first = SurfaceShape { colour: 0x7E, ..surface(8, 4, 32) };
        let staging = SurfaceShape { format: 21, ..surface(8, 4, 32) };
        assert!(colours_turn(&red_first, &staging));
        assert!(colours_turn(
            &SurfaceShape { colour: 0x7F, ..red_first },
            &SurfaceShape { format: 22, ..staging }
        ));
        assert!(!colours_turn(&surface(8, 4, 32), &staging), "a surface that names no red-first format");
        assert!(
            !colours_turn(&SurfaceShape { format: 21, ..red_first }, &staging),
            "a surface of the kernel's"
        );
        assert!(
            !colours_turn(&red_first, &SurfaceShape { tiling: Some(73), ..staging }),
            "a target in tiles"
        );
        assert!(
            !colours_turn(&red_first, &SurfaceShape { format: 0, colour: 0x7E, ..staging }),
            "one of Intel's"
        );
    }

    #[test]
    fn red_and_blue_change_over_in_the_rows_and_nowhere_else() {
        let (mut work, _xe) = testing::work(Stage::Everything);
        let space = work.create_space(&GPU_RANGES).unwrap();
        let target = Pages::new(0x1000);
        let picture: Vec<u8> = (0..0x1000u32).map(|byte| (byte % 251) as u8).collect();
        target.write(0, &picture);
        let to = work.adopt(target.address(), 0x1000).unwrap();
        work.map(&map_request(space, to, TARGET_AT, 0, 0x1000)).unwrap();
        // 4 pixels by 3 rows, from the second pixel of the second row, the rows 64 bytes apart.
        let area = Area { address: TARGET_AT + 64 + 4, row_bytes: 16, rows: 3, pitch: 64 };
        work.turn_colours(space, &area).unwrap();
        let mut expected = picture.clone();
        for row in 0..3 {
            let start = 64 + 4 + row * 64;
            for pixel in expected[start..start + 16].as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
        }
        assert_eq!(target.bytes(), expected);
        assert!(work.turn_colours(space, &Area { row_bytes: 6, ..area }).is_err(), "not whole pixels");
        assert!(
            work.turn_colours(space, &Area { address: TARGET_AT + 0xFE0, ..area }).is_err(),
            "past the mapping"
        );
        assert_eq!(target.bytes(), expected, "neither touched anything");
    }

    #[test]
    fn a_staging_copys_rectangle_lands_at_its_corner_in_the_target() {
        let (mut work, _xe) = testing::work(Stage::Everything);
        let space = work.create_space(&GPU_RANGES).unwrap();
        let copy = StagingCopy {
            source: surface(64, 64, 256),
            target: surface(64, 64, 512),
            source_address: SOURCE_AT,
            target_address: TARGET_AT,
            left: 2,
            top: 3,
            right: 10,
            bottom: 7,
            target_left: 5,
            target_top: 1,
            target_guest_address: None,
            is_screen: false,
            own_space: space,
        };
        let area = Area { address: TARGET_AT + 512 + 5 * 4, row_bytes: 8 * 4, rows: 4, pitch: 512 };
        assert_eq!(Area::written_by(&copy, 4), Some(area));
        let tiled = StagingCopy { target: SurfaceShape { tiling: Some(73), ..copy.target }, ..copy };
        assert_eq!(Area::written_by(&tiled, 4), None);
        assert_eq!(Area::written_by(&StagingCopy { right: 1, ..copy }, 4), None, "inside out");
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
