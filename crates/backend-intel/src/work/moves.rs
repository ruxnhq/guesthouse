//! A first lock's move: an allocation's contents, from its buffer on the card into the guest's
//! pages, which the core then maps at the same GPU addresses.
//!
//! The CPU reads the card's memory uncached. On the tester's B580 that was about 20 MB/s, 50 ms
//! for each MiB moved, and the daemon answers every guest process on the thread that moves, so a
//! move of a few MiB froze the whole guest for a tenth of a second or more. The copy engine moves
//! it at the speed of the bus instead.
//!
//! Both ends are mapped in the daemon's own space, at most [`MOVE_PIECE`] of each at a time, in a
//! window of addresses taken once, and moved as the kernel moves a buffer itself (`emit_copy` in
//! Linux 7.2's `xe_migrate.c`): with `MEM_COPY` in its page mode where the card has it, every Xe2
//! card, which is also the copy the card's self-test runs at every start, from card memory into host
//! pages; and with `XY_FAST_COPY_BLT` in rows of a page before that, the kernel's
//! `emit_xy_fast_copy`. The daemon waits for each piece, as the core asks of `preserve_contents`,
//! and takes the mappings away again.
//!
//! The fast copy is used only from graphics version 12.50 (Alchemist and later): there the kernel
//! sets the Tile4 bits of dword 1, as [`batch::fast_copy`] always does, so the dwords are the
//! kernel's own. Older cards, and a move the engine can't be given or fails, go by the CPU as
//! before. A copy that never ends leaves its window mapped, as the engine may still be reading it,
//! and every later move goes by the CPU.

use super::CardWork;
use super::copies::CopyKind;
use crate::xe::batch::{self, BlitSide, Tiling};
use crate::xe_query::IpVersion;
use guesthouse_backend::{
    BackendError, HostServices, MapMemoryType, MapRequest, MemoryId, SpaceId, UnmapRequest,
};
use std::time::Instant;

/// The most of an allocation each end of a move maps at a time.
pub const MOVE_PIECE: u64 = 32 << 20;

/// How many moves that went by the CPU instead the log gets at info level. The rest go to the debug
/// log.
const CPU_INSTEAD_TO_SAY: u64 = 16;

/// The rows of a fast copy: a page, as `xe_migrate_copy` gives `emit_copy`.
const FAST_COPY_PITCH: u32 = 0x1000;

/// How the copy engine moves a piece.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveCopy {
    /// `MEM_COPY` in its page mode, which every Xe2 card has (`has_mem_copy_instr` in `xe_pci.c`).
    MemCopy,
    /// `XY_FAST_COPY_BLT` in rows of a page.
    FastCopy,
}

impl MoveCopy {
    /// How a card of this graphics version moves, if the engine does: see the module's notes.
    pub fn for_version(ip: IpVersion) -> Option<Self> {
        const XE2: u16 = 20;
        const TILE4_BITS_SET: u32 = 1250;
        let version = u32::from(ip.major) * 100 + u32::from(ip.minor);
        if ip.major >= XE2 {
            Some(Self::MemCopy)
        } else if version >= TILE4_BITS_SET {
            Some(Self::FastCopy)
        } else {
            None
        }
    }
}

/// What the moves keep between first locks.
#[derive(Debug, Default)]
pub(super) struct Moves {
    /// How the engine moves on this card, if it does.
    copy: Option<MoveCopy>,
    /// A move's copy never ended, so later moves go by the CPU.
    stuck: bool,
    /// Moves that went by the CPU though the card has `MEM_COPY`.
    cpu_instead: u64,
}

impl Moves {
    pub fn for_card(ip: Option<IpVersion>) -> Self {
        Self { copy: ip.and_then(MoveCopy::for_version), ..Self::default() }
    }
}

/// Where a piece of a move is: its offset in both blocks, and its length.
#[derive(Debug, Clone, Copy)]
struct Piece {
    offset: u64,
    length: u64,
}

impl CardWork {
    /// Moves `size` bytes from a buffer on the card to the guest's pages adopted as `to`, by the copy
    /// engine in `own_space` where it can, and by the CPU where it can't.
    pub fn preserve(
        &mut self,
        host: &mut dyn HostServices,
        own_space: SpaceId,
        from: MemoryId,
        to: MemoryId,
        size: u64,
    ) -> Result<(), BackendError> {
        let started = Instant::now();
        if let Some(copy) = self.moves.copy.filter(|_| !self.moves.stuck) {
            match self.move_by_engine(host, own_space, copy, (from, to), size) {
                Ok(()) => {
                    self.note_copy(CopyKind::MoveByEngine, started.elapsed());
                    return Ok(());
                }
                Err(error) => self.note_move_by_cpu(&error),
            }
        }
        let started = Instant::now();
        self.move_by_cpu(from, to, size)?;
        self.note_copy(CopyKind::MoveByCpu, started.elapsed());
        Ok(())
    }

    fn move_by_engine(
        &mut self,
        host: &mut dyn HostServices,
        space: SpaceId,
        copy: MoveCopy,
        ends: (MemoryId, MemoryId),
        size: u64,
    ) -> Result<(), BackendError> {
        let window = self.move_window(host, space, MOVE_PIECE)?;
        let mut offset = 0;
        while offset < size {
            let piece = Piece { offset, length: (size - offset).min(MOVE_PIECE) };
            let instructions = copy_instructions(copy, window, piece.length, self.copy_mocs())?;
            self.move_piece(host, space, window, ends, piece, &instructions)?;
            offset += piece.length;
        }
        Ok(())
    }

    /// Maps a piece of each end in the window, copies it, waits, and takes the mappings away.
    fn move_piece(
        &mut self,
        host: &mut dyn HostServices,
        space: SpaceId,
        (source, target): (u64, u64),
        (from, to): (MemoryId, MemoryId),
        piece: Piece,
        instructions: &[u32],
    ) -> Result<(), BackendError> {
        self.map(&window_mapping(space, from, source, piece, false))?;
        if let Err(error) = self.map(&window_mapping(space, to, target, piece, true)) {
            self.unmap_window(space, from, source, piece);
            return Err(error);
        }
        if let Err(error) = self.run_on_copy_engine(host, space, instructions) {
            self.unmap_window(space, from, source, piece);
            self.unmap_window(space, to, target, piece);
            return Err(error);
        }
        let copied = self.wait_for_copy();
        if copied.is_err() && !self.copy_engine_idle() {
            // The engine may still be reading and writing through the window, so it stays mapped.
            self.moves.stuck = true;
            log::warn!("intel work: a first lock's move never finished, so later moves go by the CPU");
            return copied;
        }
        self.unmap_window(space, from, source, piece);
        self.unmap_window(space, to, target, piece);
        copied
    }

    fn unmap_window(&mut self, space: SpaceId, memory: MemoryId, address: u64, piece: Piece) {
        let unmap = UnmapRequest { space, memory, address, length: piece.length };
        if let Err(error) = self.unmap(&unmap) {
            log::warn!("intel work: taking a move's mapping at {address:#x} away failed: {error}");
        }
    }

    /// Says why a move went by the CPU on a card whose copy engine could have made it: the first
    /// few at info level, the rest in the debug log.
    fn note_move_by_cpu(&mut self, error: &BackendError) {
        self.moves.cpu_instead += 1;
        let count = self.moves.cpu_instead;
        let level = if count <= CPU_INSTEAD_TO_SAY { log::Level::Info } else { log::Level::Debug };
        log::log!(level, "intel work: a first lock's move goes by the CPU instead ({count} so far): {error}");
    }
}

/// A piece of one end, in the daemon's own space. The daemon's copies carry no cache invalidates,
/// so its mappings are uncached.
fn window_mapping(
    space: SpaceId,
    memory: MemoryId,
    address: u64,
    piece: Piece,
    writable: bool,
) -> MapRequest {
    MapRequest {
        space,
        memory,
        offset: piece.offset,
        length: piece.length,
        address,
        small_pages: false,
        gpu_cached: false,
        readable: true,
        writable,
        executable: false,
        memory_type: MapMemoryType::Default,
        driver_protection: 0,
    }
}

/// The instructions that copy `length` bytes from the window's source half to its target half.
fn copy_instructions(
    copy: MoveCopy,
    (source, target): (u64, u64),
    length: u64,
    mocs: Option<u32>,
) -> Result<Vec<u32>, BackendError> {
    match copy {
        MoveCopy::MemCopy => mem_copies(source, target, length, mocs.unwrap_or(0)),
        MoveCopy::FastCopy => fast_copy_rows(source, target, length, mocs),
    }
}

/// `MEM_COPY` in its page mode for `length` bytes, in as many copies as its count of pages needs.
fn mem_copies(source: u64, target: u64, length: u64, mocs: u32) -> Result<Vec<u32>, BackendError> {
    let mut words = Vec::new();
    let mut done = 0;
    while done < length {
        let bytes = (length - done).min(batch::MEM_COPY_MOST_BYTES);
        if !bytes.is_multiple_of(batch::MEM_COPY_PAGE) {
            return Err(BackendError::new(format!("{length:#x} bytes aren't whole pages of the copy")));
        }
        let copy =
            batch::copy_bytes(source + done, target + done, bytes, mocs).map_err(|batch::CopyTooBig| {
                BackendError::new(format!("{bytes:#x} bytes is too much for one copy"))
            })?;
        words.extend_from_slice(&copy);
        done += bytes;
    }
    Ok(words)
}

/// One `XY_FAST_COPY_BLT` of `length` bytes in rows of [`FAST_COPY_PITCH`], each row a page of
/// 32-bit pixels, as `emit_xy_fast_copy` writes it.
fn fast_copy_rows(
    source: u64,
    target: u64,
    length: u64,
    mocs: Option<u32>,
) -> Result<Vec<u32>, BackendError> {
    let pitch = u64::from(FAST_COPY_PITCH);
    if !length.is_multiple_of(pitch) {
        return Err(BackendError::new(format!("{length:#x} bytes aren't whole rows of the fast copy")));
    }
    let rows =
        u32::try_from(length / pitch).map_err(|_| BackendError::new("too many rows for one fast copy"))?;
    let side = |address| BlitSide { address, pitch: FAST_COPY_PITCH, tiling: Tiling::Linear, x: 0, y: 0 };
    let width = FAST_COPY_PITCH / batch::FAST_COPY_PIXEL_BYTES;
    let copy = batch::fast_copy(&side(source), &side(target), width, rows, mocs).map_err(
        |batch::NotEncodable(why)| BackendError::new(format!("the fast copy can't be given this: {why}")),
    )?;
    Ok(copy.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card;
    use crate::stage::Stage;
    use crate::work::testing::{self, FakeHost, Pages};
    use crate::xe::abi::Keying;
    use crate::xe::fake::FakeXe;
    use crate::xe::plan::BufferUse;
    use guesthouse_backend::GPU_RANGES;

    struct Rig {
        work: CardWork,
        xe: FakeXe,
        host: FakeHost,
        space: SpaceId,
    }

    impl Rig {
        fn on(work: CardWork, xe: FakeXe) -> Self {
            let mut work = work;
            let space = work.create_space(&GPU_RANGES).unwrap();
            Self { work, xe, host: FakeHost::default(), space }
        }

        /// A buffer on the card holding a picture, and zeroed guest pages to move it into.
        fn ends(&mut self, size: u64) -> (MemoryId, MemoryId, Pages, Vec<u8>) {
            let (from, _) = self.work.allocate(size, BufferUse::Card).unwrap();
            let picture: Vec<u8> = (0..size).map(|at| (at % 251) as u8).collect();
            self.work.write_for_test(from, &picture);
            let pages = Pages::new(size as usize);
            let to = self.work.adopt(pages.address(), size).unwrap();
            (from, to, pages, picture)
        }

        fn preserve(&mut self, from: MemoryId, to: MemoryId, size: u64) {
            self.work.preserve(&mut self.host, self.space, from, to, size).unwrap();
        }

        /// Mappings left in the daemon's own space, past the copy engine's batch page.
        fn mappings_left(&self) -> usize {
            let batch = usize::from(self.work.copy_engine_space().is_some());
            self.work.spaces[&self.space].mappings.len() - batch
        }
    }

    fn battlemage() -> Rig {
        let (work, xe) = testing::work(Stage::Everything);
        Rig::on(work, xe)
    }

    #[test]
    fn contents_move_by_the_copy_engine_and_its_mappings_go() {
        let mut rig = battlemage();
        let (from, to, pages, picture) = rig.ends(0x3000);
        rig.preserve(from, to, 0x3000);
        assert_eq!(pages.bytes(), picture);
        assert_eq!(rig.xe.calls_named("exec").len(), 1);
        assert_eq!(rig.mappings_left(), 0, "both ends' mappings are gone again");
        let (from, to, pages, picture) = rig.ends(0x1000);
        rig.preserve(from, to, 0x1000);
        assert_eq!(pages.bytes(), picture, "the window is used again");
        assert_eq!(rig.xe.calls_named("exec").len(), 2);
    }

    #[test]
    fn a_move_larger_than_the_window_goes_in_pieces() {
        let mut rig = battlemage();
        let size = MOVE_PIECE + 0x2000;
        let (from, to, pages, picture) = rig.ends(size);
        rig.preserve(from, to, size);
        assert!(pages.bytes() == picture, "every byte, across both pieces");
        assert_eq!(rig.xe.calls_named("exec").len(), 2, "one run for each piece");
        assert_eq!(rig.mappings_left(), 0);
    }

    fn card_of_version(major: u16, minor: u16) -> Rig {
        let mut card = card::tests::battlemage();
        card.gts[0].ip.major = major;
        card.gts[0].ip.minor = minor;
        let xe = FakeXe::default();
        let work =
            CardWork::new(Box::new(xe.clone()), &card, Stage::Everything, 1 << 40, Keying::BindingIndex);
        Rig::on(work.unwrap(), xe)
    }

    #[test]
    fn a_card_before_alchemist_moves_by_the_cpu() {
        let mut rig = card_of_version(12, 10);
        let (from, to, pages, picture) = rig.ends(0x2000);
        rig.preserve(from, to, 0x2000);
        assert_eq!(pages.bytes(), picture);
        assert!(rig.xe.calls_named("exec").is_empty());
    }

    #[test]
    fn an_alchemist_card_moves_by_a_fast_copy_in_rows_of_a_page() {
        let mut rig = card_of_version(12, 55);
        let (from, to, pages, picture) = rig.ends(0x3000);
        rig.preserve(from, to, 0x3000);
        assert_eq!(pages.bytes(), picture);
        assert_eq!(rig.xe.calls_named("exec").len(), 1);
        assert_eq!(rig.mappings_left(), 0);
    }

    #[test]
    fn each_card_moves_as_the_kernel_does() {
        let version = |major, minor| MoveCopy::for_version(IpVersion { major, minor, revision: 0 });
        assert_eq!(version(20, 1), Some(MoveCopy::MemCopy), "Battlemage");
        assert_eq!(version(30, 0), Some(MoveCopy::MemCopy));
        assert_eq!(version(12, 55), Some(MoveCopy::FastCopy), "Alchemist");
        assert_eq!(version(12, 70), Some(MoveCopy::FastCopy), "Meteor Lake");
        assert_eq!(version(12, 10), None, "the kernel leaves the Tile4 bits clear there");
        assert_eq!(version(0, 0), None, "a card that gives no version");
    }

    /// `emit_xy_fast_copy` in Linux 7.2's `xe_migrate.c` before Xe2, from graphics 12.50:
    /// `XY_FAST_COPY_BLT_CMD | (10 - 2)`, `XY_FAST_COPY_BLT_DEPTH_32 | pitch | tile_y`, 0,
    /// `(size / pitch) << 16 | pitch / 4`, the target, 0, `pitch`, the source; `tile_y` is bits 31
    /// and 30, and no cache index. `xe_migrate_copy` gives it a page as the pitch.
    #[test]
    fn a_fast_copy_piece_is_the_kernels_own() {
        let (source, target, size) = (0x7F_0020_0000u64, 0x7F_0220_0000u64, 0x20_0000u32);
        let pitch = 0x1000u32;
        let kernel = [
            (2 << 29 | 0x42 << 22) | (10 - 2),
            (3 << 24) | pitch | (1 << 31 | 1 << 30),
            0,
            ((size / pitch) << 16) | (pitch / 4),
            target as u32,
            (target >> 32) as u32,
            0,
            pitch,
            source as u32,
            (source >> 32) as u32,
        ];
        let ours = copy_instructions(MoveCopy::FastCopy, (source, target), size.into(), None).unwrap();
        assert_eq!(ours, kernel);
        assert!(copy_instructions(MoveCopy::FastCopy, (source, target), 0x800, None).is_err(), "half a row");
    }

    #[test]
    fn a_move_xe_refuses_goes_by_the_cpu_and_leaves_nothing_mapped() {
        let mut rig = battlemage();
        let (from, to, pages, picture) = rig.ends(0x2000);
        rig.xe.fail("exec", libc::EINVAL);
        rig.preserve(from, to, 0x2000);
        assert_eq!(pages.bytes(), picture);
        assert_eq!(rig.mappings_left(), 0);
        assert!(!rig.work.moves.stuck, "nothing ran, so the next move tries the engine again");
    }

    #[test]
    fn a_move_that_never_ends_keeps_its_window_and_later_moves_go_by_the_cpu() {
        let mut rig = battlemage();
        let (from, to, pages, picture) = rig.ends(0x2000);
        rig.xe.hold_runs();
        rig.preserve(from, to, 0x2000);
        assert_eq!(pages.bytes(), picture, "the CPU moved it instead");
        assert!(rig.work.moves.stuck);
        assert_eq!(rig.mappings_left(), 2, "the engine may still be using both ends");
        let (from, to, pages, picture) = rig.ends(0x1000);
        rig.preserve(from, to, 0x1000);
        assert_eq!(pages.bytes(), picture);
        assert_eq!(rig.xe.calls_named("exec").len(), 1, "no second run");
    }

    #[test]
    fn a_piece_is_whole_copies_of_at_most_sixteen_mib() {
        let copy = |length| copy_instructions(MoveCopy::MemCopy, (0x1_0000, 0x900_0000), length, Some(3));
        let words = copy(0x180_0000).unwrap();
        assert_eq!(words.len(), 2 * batch::MEM_COPY_DWORDS);
        assert_eq!(
            words[..batch::MEM_COPY_DWORDS],
            batch::copy_bytes(0x1_0000, 0x900_0000, 1 << 24, 3).unwrap()
        );
        let second = batch::copy_bytes(0x101_0000, 0xA00_0000, 0x80_0000, 3).unwrap();
        assert_eq!(words[batch::MEM_COPY_DWORDS..], second);
        assert!(copy(0x80).is_err(), "less than a page of the copy");
    }
}
