//! Copies by the card's copy engine, for pictures laid out in tiles, which the CPU can't copy row by
//! row. One instruction, `XY_FAST_COPY_BLT` ([`batch::fast_copy`]), reads a tiled surface and writes
//! rows, or the other way round, so a frame in the card's memory never passes through the CPU,
//! which reads that memory uncached. Surfaces in rows at both ends are still the CPU's: see
//! [`super::copies`].
//!
//! The daemon's own space gets, the first time, a page for the batch, a queue on the first copy
//! engine and a timeline its runs signal. One copy runs at a time, as the core asks of a backend,
//! and its point on the timeline says when it is over. After a batch xe writes an `MI_FLUSH_DW`
//! before it signals (`__emit_job_gen12_simple` in Linux 7.2's `xe_ring_ops.c`), so the copy's
//! writes are out by then. Each copy being a job of its own, that flush also falls between any two
//! copies, as the PRM asks between fast copies that change their tiling methods or write over each
//! other.
//!
//! Which tiles are copied: gmmlib's Tile4 and TileX, which don't depend on the pixel's size, and
//! its Tile64 for a 2D surface of one sample and 32-bit elements, as a back buffer is. Any other
//! tile mode (Tile64 of other sizes or with samples, 1D and 3D layouts, the legacy Tile-Y, Yf and
//! Ys) is refused, both by [`tiled_bytes`], so the core turns such a copy down, and here.

use super::CardWork;
use super::copies::{Area, CopyKind, CopyTally};
use crate::protocol::allocation::tile_mode;
use crate::xe::abi::{EngineInstance, Keying};
use crate::xe::batch::{self, BlitSide, Tiling};
use crate::xe::plan::{self, BufferUse, ExecVerdict, Point};
use crate::xe_query::Engine;
use guesthouse_backend::{
    BackendError, COPY_PIXEL_BYTES, HostServices, MapMemoryType, MapRequest, MemoryId, SlotCopy, SpaceId,
    StagingCopy, SurfaceShape,
};
use std::time::{Duration, Instant};

/// How long a copy may take before it counts as failed. A frame's copy takes about a millisecond,
/// even across the bus into the guest's pages, and NVIDIA's copies are given as long.
pub const COPY_PATIENCE: Duration = Duration::from_secs(2);

/// The batch's page.
const BATCH_BYTES: u64 = 0x1000;

/// How many failed copies the log gets as warnings. The rest go to the debug log.
const FAILURES_TO_WARN_OF: u64 = 16;

/// The layout the copy engine is given for a surface's tiling, as the shape keeps it: `None` in the
/// shape is rows, and a gmmlib tile mode this backend doesn't copy is `None` here.
pub fn tiling_of(tiling: Option<u32>) -> Option<Tiling> {
    match tiling {
        None => Some(Tiling::Linear),
        Some(tile_mode::LEGACY_TILE_X) => Some(Tiling::TileX),
        Some(tile_mode::TILE4) => Some(Tiling::Tile4),
        Some(tile_mode::TILE_64_2D_32BPE) => Some(Tiling::Tile64),
        Some(_) => None,
    }
}

/// The bytes a tiled surface fills as the copy engine lays it out: its pitch, a whole number of
/// tiles, down to the end of the tile that holds its last row. `None` for a tile mode the engine
/// isn't given here, a pitch it can't take, or a corner at byte `x` of row `y` past what a
/// coordinate holds.
pub fn tiled_bytes(surface: &SurfaceShape, x: u64, y: u64) -> Option<u64> {
    let tiling = tiling_of(Some(surface.tiling?))?;
    let pitch = surface.pitch;
    if pitch == 0
        || !pitch.is_multiple_of(tiling.width())
        || tiling.pitch_field(pitch) > batch::FAST_COPY_PITCH_MOST
    {
        return None;
    }
    let most = u64::from(batch::FAST_COPY_COORDINATE_MOST);
    if x / u64::from(COPY_PIXEL_BYTES) > most || y > most {
        return None;
    }
    let rows = u64::from(surface.height).next_multiple_of(tiling.rows().into());
    rows.checked_mul(pitch.into())
}

/// A copy for the copy engine: a rectangle of `width` by `height` pixels from one side to the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Blit {
    pub source: BlitSide,
    pub target: BlitSide,
    pub width: u32,
    pub height: u32,
}

impl Blit {
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// A rectangle of one surface into another, as [`super::copies::Rows::of_staging`] places
    /// one in rows. Refused for a tile mode the engine isn't given here, or a rectangle that isn't
    /// inside both surfaces.
    pub fn of_staging(copy: &StagingCopy) -> Result<Self, &'static str> {
        let corner = |value: i32| u32::try_from(value).map_err(|_| "a corner before its surface's start");
        let (left, top, right, bottom) =
            (corner(copy.left)?, corner(copy.top)?, corner(copy.right)?, corner(copy.bottom)?);
        let (target_left, target_top) = (corner(copy.target_left)?, corner(copy.target_top)?);
        let inside_out = "a rectangle inside out";
        let (width, height) =
            (right.checked_sub(left).ok_or(inside_out)?, bottom.checked_sub(top).ok_or(inside_out)?);
        let ends = |start: u32, length: u32| u64::from(start) + u64::from(length);
        let inside = right <= copy.source.width
            && bottom <= copy.source.height
            && ends(target_left, width) <= copy.target.width.into()
            && ends(target_top, height) <= copy.target.height.into();
        if !inside {
            return Err("a rectangle that isn't inside both surfaces");
        }
        Ok(Self {
            source: side_of(copy.source_address, &copy.source, left, top)?,
            target: side_of(copy.target_address, &copy.target, target_left, target_top)?,
            width,
            height,
        })
    }

    /// A whole picture into a slot of the frame ring, which is in rows: this backend offers no
    /// tiled slots.
    pub fn of_slot(copy: &SlotCopy) -> Result<Self, &'static str> {
        if copy.slot_tiling.is_some() {
            return Err("a slot laid out in tiles, which this backend never offers");
        }
        if copy.width > copy.source.width || copy.height > copy.source.height {
            return Err("a picture larger than its surface");
        }
        let slot = BlitSide {
            address: copy.slot_address,
            pitch: copy.slot_pitch,
            tiling: Tiling::Linear,
            x: 0,
            y: 0,
        };
        Ok(Self {
            source: side_of(copy.source_address, &copy.source, 0, 0)?,
            target: slot,
            width: copy.width,
            height: copy.height,
        })
    }

    /// The instruction, with the cache index for both ends where the card has the fields.
    pub fn dwords(&self, mocs: Option<u32>) -> Result<[u32; batch::FAST_COPY_DWORDS], batch::NotEncodable> {
        batch::fast_copy(&self.source, &self.target, self.width, self.height, mocs)
    }
}

fn side_of(address: u64, surface: &SurfaceShape, x: u32, y: u32) -> Result<BlitSide, &'static str> {
    let tiling = tiling_of(surface.tiling).ok_or("a tile mode the copy engine isn't given here")?;
    Ok(BlitSide { address, pitch: surface.pitch, tiling, x, y })
}

/// What the daemon keeps on the card for the copy engine's copies.
#[derive(Debug)]
struct CopyEngine {
    space: SpaceId,
    engine: EngineInstance,
    /// Made when a copy first needs it, and again after xe banned the last one.
    queue: Option<u32>,
    /// The timeline the runs signal, and the last point a run was sent to signal.
    done: u32,
    sent: u64,
    /// The page the batch is written in, and where it is mapped in the space.
    batch: MemoryId,
    batch_address: u64,
    /// The copy under way: its point, and when it was sent.
    running: Option<(u64, Instant)>,
}

/// The copy engine, once made, and what the log has been told of it.
#[derive(Debug, Default)]
pub(super) struct Blits {
    engine: Option<CopyEngine>,
    /// The cache index both ends are read and written with, on a card that has the fields.
    mocs: Option<u32>,
    /// The kinds of copy the log has named, by the layouts at each end.
    kinds_said: Vec<(Tiling, Tiling)>,
    first_finished_said: bool,
    failures: u64,
    /// Copies in rows that went by the CPU, as the copy engine wasn't given them.
    cpu_instead: u64,
    /// Rows of the copy under way's target whose red and blue change over once it ends.
    turn_after: Option<(SpaceId, Area)>,
    /// Whether the log has been told colours are changed over.
    pub(super) turned_said: bool,
    /// The time frames took, said now and then: see [`super::copies::CopyTally`].
    pub(super) tally: CopyTally,
}

impl Blits {
    /// On an Xe2 card or later the fast copy takes a cache index for each end, and the copies use
    /// the uncached one, as the kernel's own copies do (`emit_xy_fast_copy` in `xe_migrate.c`
    /// sets it only from graphics version 20). Before that those bits are reserved.
    pub fn new(graphics_major: u16) -> Self {
        const XE2: u16 = 20;
        Self { mocs: (graphics_major >= XE2).then_some(batch::XE2_UNCACHED_MOCS), ..Self::default() }
    }
}

fn xe_error(what: &str, error: &std::io::Error) -> BackendError {
    BackendError::new(format!("{what}: {error}"))
}

impl CardWork {
    /// Starts a copy on the copy engine, in the daemon's own space. `Ok` means it is under way and
    /// [`CardWork::poll_blit`] says when it ends. The instruction is written before anything is
    /// made, so a copy the engine can't be given leaves nothing behind.
    pub fn start_blit(
        &mut self,
        host: &mut dyn HostServices,
        space: SpaceId,
        blit: &Blit,
    ) -> Result<(), BackendError> {
        self.blits.turn_after = None;
        let dwords = blit.dwords(self.blits.mocs).map_err(|batch::NotEncodable(why)| {
            BackendError::new(format!("the copy engine can't be given this copy: {why}: {blit:?}"))
        })?;
        if self.blits.engine.as_ref().is_some_and(|engine| engine.space != space) {
            self.close_copy_engine();
        }
        if self.blits.engine.is_none() {
            self.blits.engine = Some(self.make_copy_engine(host, space)?);
        }
        let engine = self.blits.engine.as_ref().expect("made just above");
        let (done, sent, batch_memory, batch_address) =
            (engine.done, engine.sent, engine.batch, engine.batch_address);
        let (queue, instance) = (engine.queue, engine.engine);
        if engine.running.is_some() {
            return Err(BackendError::new("a copy is already under way on the copy engine"));
        }
        // The batch's page is written over, so the run before has to be done with it. Only one
        // that ran past its patience can still be going.
        let earlier_done = sent == 0
            || self
                .xe
                .point_reached(done, sent, Duration::ZERO)
                .map_err(|error| xe_error("asking whether the copy engine's last run ended", &error))?;
        if !earlier_done {
            return Err(BackendError::new(
                "the copy engine is still on an earlier copy that ran past its time",
            ));
        }
        let mut words = dwords.to_vec();
        words.push(batch::BATCH_END);
        if !self.cpu_view(batch_memory)?.write(0, &batch::bytes(&words)) {
            return Err(BackendError::new("the copy's batch doesn't fit its page"));
        }
        let held = self.spaces.get(&space).ok_or(BackendError::new("the daemon's own space is closed"))?;
        let vm = held.vm;
        let last_bind =
            (held.bind_point > 0).then_some(Point { timeline: held.binds, point: held.bind_point });
        let queue = match queue {
            Some(queue) => queue,
            None => {
                let made = self
                    .xe
                    .create_queue(vm, instance, Keying::BindingIndex)
                    .map_err(|error| xe_error("xe made no queue on the copy engine", &error))?;
                self.blits.engine.as_mut().expect("made above").queue = Some(made);
                made
            }
        };
        let point = sent + 1;
        let syncs = plan::exec_syncs(last_bind, None, Point { timeline: done, point });
        if let Err(error) = self.xe.exec(queue, batch_address, &syncs) {
            if plan::exec_verdict(&error) == ExecVerdict::Lost {
                self.drop_copy_queue();
            }
            return Err(xe_error("xe refused the copy engine's batch", &error));
        }
        let engine = self.blits.engine.as_mut().expect("made above");
        engine.sent = point;
        engine.running = Some((point, Instant::now()));
        let kind = (blit.source.tiling, blit.target.tiling);
        if !self.blits.kinds_said.contains(&kind) {
            self.blits.kinds_said.push(kind);
            log::info!(
                "intel work: copies from {:?} to {:?} go by the copy engine; the first is {}x{}, pitches {} and {}",
                kind.0,
                kind.1,
                blit.width,
                blit.height,
                blit.source.pitch,
                blit.target.pitch
            );
        }
        Ok(())
    }

    /// Has red and blue changed over in these rows of the copy under way's target once it ends
    /// well, as the copy engine moves bytes as they are. See [`super::copies::colours_turn`].
    pub fn turn_after_blit(&mut self, space: SpaceId, area: Option<Area>) {
        self.blits.turn_after = area.map(|area| (space, area));
    }

    /// Whether the copy under way has ended: `None` while it runs, then how it went. A run that
    /// passes [`COPY_PATIENCE`] counts as failed, and the next copy waits for it to end all the
    /// same, as it reads the same page.
    pub fn poll_blit(&mut self, now: Instant) -> Option<Result<(), BackendError>> {
        let nothing = || Some(Err(BackendError::new("no copy is under way on the copy engine")));
        let Some(engine) = self.blits.engine.as_mut() else { return nothing() };
        let Some((point, since)) = engine.running else { return nothing() };
        let (done, queue) = (engine.done, engine.queue);
        let reached = self.xe.point_reached(done, point, Duration::ZERO);
        if matches!(reached, Ok(false)) && now.saturating_duration_since(since) <= COPY_PATIENCE {
            return None;
        }
        self.blits.engine.as_mut().expect("found above").running = None;
        let turn = self.blits.turn_after.take();
        match reached {
            Ok(true) => {}
            Ok(false) => {
                return Some(Err(BackendError::new(format!(
                    "the copy engine had not finished a copy after {COPY_PATIENCE:?}"
                ))));
            }
            Err(error) => return Some(Err(xe_error("asking whether the copy engine's run ended", &error))),
        }
        // The timeline is signalled however the run ended, so one that faulted or hung is told
        // apart by its queue's ban.
        match queue.map(|queue| self.xe.queue_banned(queue)) {
            Some(Ok(true)) => {
                self.drop_copy_queue();
                return Some(Err(BackendError::new(
                    "xe banned the copy engine's queue: the copy faulted or hung",
                )));
            }
            Some(Err(error)) => {
                return Some(Err(xe_error("asking whether xe banned the copy engine's queue", &error)));
            }
            _ => {}
        }
        if !self.blits.first_finished_said {
            self.blits.first_finished_said = true;
            log::info!(
                "intel work: the copy engine's first copy finished after {:?}",
                now.saturating_duration_since(since)
            );
        }
        self.note_copy(CopyKind::Engine, now.saturating_duration_since(since));
        if let Some((space, area)) = turn
            && let Err(error) = self.turn_colours(space, &area)
        {
            return Some(Err(error));
        }
        Some(Ok(()))
    }

    /// Says why a copy by the copy engine failed: the first few as warnings, the rest in the debug
    /// log, as one can fail every frame.
    pub fn note_blit_failure(&mut self, error: &BackendError) {
        self.blits.failures += 1;
        let level =
            if self.blits.failures <= FAILURES_TO_WARN_OF { log::Level::Warn } else { log::Level::Debug };
        log::log!(
            level,
            "intel work: a copy by the copy engine failed ({} so far): {error}",
            self.blits.failures
        );
    }

    /// Says why a copy in rows went by the CPU instead of the copy engine: the first few at info
    /// level, the rest in the debug log.
    pub fn note_cpu_instead(&mut self, error: &BackendError) {
        self.blits.cpu_instead += 1;
        let level =
            if self.blits.cpu_instead <= FAILURES_TO_WARN_OF { log::Level::Info } else { log::Level::Debug };
        log::log!(
            level,
            "intel work: a copy in rows goes by the CPU instead ({} so far): {error}",
            self.blits.cpu_instead
        );
    }

    /// The batch's page mapped in the space, and the timeline. The queue comes with the first copy.
    fn make_copy_engine(
        &mut self,
        host: &mut dyn HostServices,
        space: SpaceId,
    ) -> Result<CopyEngine, BackendError> {
        let engine = plan::engine(&self.engines, Engine::COPY, 0)
            .ok_or(BackendError::new("the card lists no copy engine"))?;
        if !self.spaces.contains_key(&space) {
            return Err(BackendError::new("the daemon's own space is closed"));
        }
        let (batch, size) = self.allocate(BATCH_BYTES, BufferUse::HostFilled)?;
        match self.place_batch(host, space, batch, size) {
            Ok((batch_address, done)) => Ok(CopyEngine {
                space,
                engine,
                queue: None,
                done,
                sent: 0,
                batch,
                batch_address,
                running: None,
            }),
            Err(error) => {
                self.release(batch);
                Err(error)
            }
        }
    }

    fn place_batch(
        &mut self,
        host: &mut dyn HostServices,
        space: SpaceId,
        batch: MemoryId,
        size: u64,
    ) -> Result<(u64, u32), BackendError> {
        let address = host.take_gpu_addresses(size, plan::HOST_PAGE)?;
        self.map(&MapRequest {
            space,
            memory: batch,
            offset: 0,
            length: size,
            address,
            small_pages: true,
            gpu_cached: false,
            readable: true,
            writable: false,
            executable: true,
            memory_type: MapMemoryType::Default,
            driver_protection: 0,
        })?;
        let done =
            self.xe.create_syncobj().map_err(|error| xe_error("xe made no timeline for copies", &error))?;
        Ok((address, done))
    }

    /// Lets the queue go, after xe banned it or lost it; the next copy makes another.
    fn drop_copy_queue(&mut self) {
        let Some(queue) = self.blits.engine.as_mut().and_then(|engine| engine.queue.take()) else { return };
        if let Err(error) = self.xe.destroy_queue(queue) {
            log::warn!("intel work: destroying the copy engine's queue failed: {error}");
        }
    }

    /// Lets everything of the copy engine go, as its space is going.
    pub(super) fn close_copy_engine(&mut self) {
        self.blits.turn_after = None;
        self.drop_copy_queue();
        let Some(engine) = self.blits.engine.take() else { return };
        if let Err(error) = self.xe.destroy_syncobj(engine.done) {
            log::warn!("intel work: destroying the copy engine's timeline failed: {error}");
        }
        self.release(engine.batch);
    }

    /// Whether the copy engine is set up in this space.
    pub(super) fn copy_engine_in(&self, space: SpaceId) -> bool {
        self.blits.engine.as_ref().is_some_and(|engine| engine.space == space)
    }

    /// The space the copy engine is set up in, for a test.
    #[cfg(test)]
    pub fn copy_engine_space(&self) -> Option<SpaceId> {
        self.blits.engine.as_ref().map(|engine| engine.space)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stage::Stage;
    use crate::work::testing::{self, FakeHost, Pages, map_request};
    use crate::xe::fake::{Call, surface_offset};
    use guesthouse_backend::GPU_RANGES;

    const SOURCE_AT: u64 = 0x400_0000;
    const TARGET_AT: u64 = 0x800_0000;

    fn surface(width: u32, height: u32, pitch: u32, tiling: Option<u32>) -> SurfaceShape {
        SurfaceShape { width, height, format: 0, pitch, tiling, refresh: (0, 1), samples: (1, 0), colour: 0 }
    }

    /// A picture's bytes in rows, every byte telling where it is.
    fn picture(row_bytes: usize, rows: usize) -> Vec<u8> {
        (0..row_bytes * rows)
            .map(|at| ((at / row_bytes) * 7 + at % row_bytes) as u8 ^ (at >> 11) as u8)
            .collect()
    }

    /// The same picture laid out as the stand-in card lays out `tiling`.
    fn tiled(rows_of: &[u8], row_bytes: usize, tiling: Tiling, pitch: u32, bytes: usize) -> Vec<u8> {
        let mut out = vec![0u8; bytes];
        for (at, &byte) in rows_of.iter().enumerate() {
            let (x, y) = ((at % row_bytes) as u64, (at / row_bytes) as u64);
            out[surface_offset(tiling, pitch.into(), x, y) as usize] = byte;
        }
        out
    }

    fn staging(source: SurfaceShape, target: SurfaceShape) -> StagingCopy {
        StagingCopy {
            source,
            target,
            source_address: SOURCE_AT,
            target_address: TARGET_AT,
            left: 0,
            top: 0,
            right: source.width as i32,
            bottom: source.height as i32,
            target_left: 0,
            target_top: 0,
            target_guest_address: None,
            is_screen: false,
            own_space: SpaceId(0),
        }
    }

    /// A frame in tiles in a buffer on the card, and a target in the guest's pages, both mapped in
    /// one space.
    struct Rig {
        work: CardWork,
        xe: crate::xe::fake::FakeXe,
        host: FakeHost,
        space: SpaceId,
        source: MemoryId,
        target: Pages,
    }

    fn rig(source_bytes: u64, target_bytes: usize) -> Rig {
        let (mut work, xe) = testing::work(Stage::Everything);
        let space = work.create_space(&GPU_RANGES).unwrap();
        let (source, size) = work.allocate(source_bytes, BufferUse::Card).unwrap();
        let target = Pages::new(target_bytes);
        let to = work.adopt(target.address(), target_bytes as u64).unwrap();
        work.map(&map_request(space, source, SOURCE_AT, 0, size)).unwrap();
        work.map(&map_request(space, to, TARGET_AT, 0, target_bytes as u64)).unwrap();
        Rig { work, xe, host: FakeHost::default(), space, source, target }
    }

    /// Tile64 for 32-bit pixels, gmmlib's mode 81, as the tester's back buffer is: two tiles across
    /// (256 pixels, 1024 bytes) and 130 rows, which take two tiles down. The engine gets the
    /// surface's start and the rectangle, and the rows that land are the picture's.
    #[test]
    fn a_frame_in_tile64_is_untiled_by_the_copy_engine_into_rows() {
        let mut rig = rig(0x4_0000, 0x2_1000);
        let rows = picture(1024, 130);
        rig.work.write_for_test(rig.source, &tiled(&rows, 1024, Tiling::Tile64, 1024, 0x4_0000));
        let source = surface(256, 130, 1024, Some(tile_mode::TILE_64_2D_32BPE));
        let copy = StagingCopy { own_space: rig.space, ..staging(source, surface(256, 130, 1024, None)) };
        let blit = Blit::of_staging(&copy).unwrap();
        rig.work.start_blit(&mut rig.host, rig.space, &blit).unwrap();
        let execs = rig.xe.calls_named("exec");
        let [Call::Exec { queue, batch, syncs }] = &execs[..] else { panic!("{execs:?}") };
        let made = rig.xe.calls_named("create_queue");
        let [Call::CreateQueue { engine, keying, .. }] = &made[..] else { panic!("{made:?}") };
        assert_eq!((engine.class, engine.instance, *keying), (Engine::COPY, 0, Keying::BindingIndex));
        assert!(rig.xe.calls_named("destroy_queue").is_empty() && *queue != 0);
        assert!(rig.work.spaces[&rig.space].mappings.contains_key(batch), "the batch is in the own space");
        assert!(matches!(syncs.last(), Some(crate::xe::abi::Sync::Signal { point: 1, .. })));
        assert!(matches!(syncs.first(), Some(crate::xe::abi::Sync::After { .. })), "after the last bind");
        let mut written = [0u8; 4 * (batch::FAST_COPY_DWORDS + 1)];
        let page = rig.work.blits.engine.as_ref().unwrap().batch;
        assert!(rig.work.cpu_view(page).unwrap().read(0, &mut written));
        let mut expected = blit.dwords(Some(batch::XE2_UNCACHED_MOCS)).unwrap().to_vec();
        expected.push(batch::BATCH_END);
        assert_eq!(
            written[..],
            batch::bytes(&expected)[..],
            "the fast copy with Xe2's uncached index, then the end"
        );
        assert!(rig.work.poll_blit(Instant::now()).unwrap().is_ok());
        let landed = rig.target.bytes();
        assert_eq!(landed[..rows.len()], rows[..], "every row, untiled");
        assert!(landed[rows.len()..].iter().all(|&byte| byte == 0), "and nothing after the last");
    }

    /// Tile4 into the middle of a target, from the middle of the source: corners that are whole
    /// OWords, and nothing written outside the rectangle.
    #[test]
    fn a_rectangle_of_tile4_lands_at_its_corner_and_nowhere_else() {
        let mut rig = rig(0x2_0000, 512 * 64);
        let rows = picture(1024, 64);
        rig.work.write_for_test(rig.source, &tiled(&rows, 1024, Tiling::Tile4, 1024, 0x2_0000));
        let source = surface(256, 64, 1024, Some(tile_mode::TILE4));
        let copy = StagingCopy {
            left: 8,
            top: 5,
            right: 40,
            bottom: 37,
            target_left: 12,
            target_top: 20,
            own_space: rig.space,
            ..staging(source, surface(128, 64, 512, None))
        };
        rig.work.start_blit(&mut rig.host, rig.space, &Blit::of_staging(&copy).unwrap()).unwrap();
        assert!(rig.work.poll_blit(Instant::now()).unwrap().is_ok());
        let landed = rig.target.bytes();
        for y in 0..64 {
            for x in 0..128 {
                let at = (y * 512 + x * 4) as usize;
                let inside = (12..44).contains(&x) && (20..52).contains(&y);
                let expected = if inside {
                    let from = ((y - 20 + 5) * 1024 + (x - 12 + 8) * 4) as usize;
                    rows[from..from + 4].to_vec()
                } else {
                    vec![0; 4]
                };
                assert_eq!(landed[at..at + 4], expected[..], "pixel ({x}, {y})");
            }
        }
    }

    /// `emit_xy_fast_copy` sets a cache index only from graphics version 20; before, the PRM has
    /// those bits reserved.
    #[test]
    fn only_an_xe2_card_or_later_gets_a_cache_index() {
        assert_eq!(Blits::new(20).mocs, Some(batch::XE2_UNCACHED_MOCS));
        assert_eq!(Blits::new(30).mocs, Some(batch::XE2_UNCACHED_MOCS));
        assert_eq!(Blits::new(12).mocs, None, "Alchemist and the Xe LPG cards");
        assert_eq!(Blits::new(0).mocs, None, "a card that gives no IP version");
    }

    #[test]
    fn a_run_is_waited_for_and_one_past_its_patience_fails_and_holds_the_next_back() {
        let mut rig = rig(0x2_0000, 512 * 64);
        let source = surface(128, 64, 512, Some(tile_mode::TILE4));
        let copy = StagingCopy { own_space: rig.space, ..staging(source, surface(128, 64, 512, None)) };
        let blit = Blit::of_staging(&copy).unwrap();
        rig.xe.hold_runs();
        rig.work.start_blit(&mut rig.host, rig.space, &blit).unwrap();
        let now = Instant::now();
        assert!(rig.work.poll_blit(now).is_none(), "under way");
        assert!(rig.work.start_blit(&mut rig.host, rig.space, &blit).is_err(), "one at a time");
        let late = rig.work.poll_blit(now + COPY_PATIENCE + Duration::from_millis(1)).unwrap();
        assert!(late.unwrap_err().0.contains("had not finished"));
        let refused = rig.work.start_blit(&mut rig.host, rig.space, &blit).unwrap_err();
        assert!(refused.0.contains("still on an earlier copy"), "{refused}");
        assert_eq!(rig.xe.calls_named("exec").len(), 1, "nothing written over the batch it reads");
        rig.xe.release_runs();
        rig.work.start_blit(&mut rig.host, rig.space, &blit).unwrap();
        assert_eq!(rig.xe.calls_named("exec").len(), 2);
        rig.xe.release_runs();
        assert!(rig.work.poll_blit(Instant::now()).unwrap().is_ok());
        assert!(rig.work.poll_blit(Instant::now()).unwrap().is_err(), "nothing under way");
    }

    #[test]
    fn a_banned_queue_fails_its_copy_and_the_next_copy_gets_a_new_one() {
        let mut rig = rig(0x2_0000, 512 * 64);
        let source = surface(128, 64, 512, Some(tile_mode::TILE4));
        let copy = StagingCopy { own_space: rig.space, ..staging(source, surface(128, 64, 512, None)) };
        let blit = Blit::of_staging(&copy).unwrap();
        rig.xe.hold_runs();
        rig.work.start_blit(&mut rig.host, rig.space, &blit).unwrap();
        rig.xe.lose_held_runs();
        let failed = rig.work.poll_blit(Instant::now()).unwrap().unwrap_err();
        assert!(failed.0.contains("banned"), "{failed}");
        assert_eq!(rig.xe.calls_named("destroy_queue").len(), 1);
        rig.xe.release_runs();
        rig.work.start_blit(&mut rig.host, rig.space, &blit).unwrap();
        assert_eq!(rig.xe.calls_named("create_queue").len(), 2);
        assert!(rig.work.poll_blit(Instant::now()).unwrap().is_ok());
    }

    #[test]
    fn a_copy_the_engine_cant_be_given_makes_nothing_and_xe_refusing_makes_no_run() {
        let mut rig = rig(0x2_0000, 512 * 64);
        let calls = rig.xe.calls().len();
        let source = surface(128, 64, 512, Some(tile_mode::TILE4));
        let off_an_oword =
            StagingCopy { left: 1, own_space: rig.space, ..staging(source, surface(128, 64, 512, None)) };
        let blit = Blit::of_staging(&off_an_oword).unwrap();
        let refused = rig.work.start_blit(&mut rig.host, rig.space, &blit).unwrap_err();
        assert!(refused.0.contains("OWord"), "{refused}");
        assert_eq!(rig.xe.calls().len(), calls, "nothing asked of xe");
        assert_eq!(rig.work.copy_engine_space(), None);
        let whole = Blit::of_staging(&StagingCopy { left: 0, ..off_an_oword }).unwrap();
        rig.xe.fail("exec", libc::EINVAL);
        assert!(rig.work.start_blit(&mut rig.host, rig.space, &whole).is_err());
        assert!(rig.work.poll_blit(Instant::now()).unwrap().is_err(), "no run under way");
        rig.xe.heal();
        rig.work.start_blit(&mut rig.host, rig.space, &whole).unwrap();
        assert!(rig.work.poll_blit(Instant::now()).unwrap().is_ok());
    }

    #[test]
    fn the_copy_engine_goes_with_its_space_and_leaves_nothing() {
        let (mut work, xe) = testing::work(Stage::Everything);
        let mut host = FakeHost::default();
        let space = work.create_space(&GPU_RANGES).unwrap();
        let (source, size) = work.allocate(0x2_0000, BufferUse::Card).unwrap();
        let (target, target_size) = work.allocate(0x2_0000, BufferUse::Card).unwrap();
        work.map(&map_request(space, source, SOURCE_AT, 0, size)).unwrap();
        work.map(&map_request(space, target, TARGET_AT, 0, target_size)).unwrap();
        let live = xe.live();
        let source_shape = surface(128, 64, 512, Some(tile_mode::TILE_64_2D_32BPE));
        let copy = StagingCopy { own_space: space, ..staging(source_shape, surface(128, 64, 512, None)) };
        work.start_blit(&mut host, space, &Blit::of_staging(&copy).unwrap()).unwrap();
        assert!(work.poll_blit(Instant::now()).unwrap().is_ok());
        assert_eq!(work.copy_engine_space(), Some(space));
        assert_eq!(xe.live(), live + 3, "the batch's page, the timeline and the queue");
        work.destroy_space(space);
        assert_eq!(work.copy_engine_space(), None);
        assert_eq!(xe.live(), live - 2, "the space's VM and timeline went too, and the two surfaces stay");
        assert_eq!(work.live_blocks(), 2, "the batch's page is freed");
    }

    /// What the core measures a tiled side by: the tester's back buffer fills 0x870000 bytes of its
    /// 0x880000, 15 tiles of 512 bytes across and 9 of 128 rows down. Tile4 and TileX fill whole
    /// tiles of their own shapes, and any other tile mode, or a pitch that isn't whole tiles, isn't
    /// measured at all, which the core takes as a copy it can't make.
    #[test]
    fn a_tiled_surface_fills_whole_tiles_and_other_tile_modes_are_not_measured() {
        let back_buffer = surface(1920, 1071, 7680, Some(tile_mode::TILE_64_2D_32BPE));
        assert_eq!(tiled_bytes(&back_buffer, 0, 0), Some(0x87_0000));
        assert_eq!(tiled_bytes(&surface(1920, 1071, 7680, Some(tile_mode::TILE4)), 0, 0), Some(7680 * 1088));
        assert_eq!(
            tiled_bytes(&surface(1920, 1071, 7680, Some(tile_mode::LEGACY_TILE_X)), 0, 0),
            Some(7680 * 1072)
        );
        for other in [0, 2, 3, 9, 72, 74, 79, 80, 82, 83, 84, 120] {
            assert_eq!(tiled_bytes(&surface(1920, 1071, 7680, Some(other)), 0, 0), None, "tile mode {other}");
        }
        assert_eq!(
            tiled_bytes(&surface(1920, 1071, 7680, None), 0, 0),
            None,
            "rows are the core's to measure"
        );
        assert_eq!(tiled_bytes(&surface(1900, 1071, 7600, Some(tile_mode::TILE_64_2D_32BPE)), 0, 0), None);
        assert_eq!(
            tiled_bytes(&back_buffer, 4 * 0x7FFF, 0x7FFF),
            Some(0x87_0000),
            "the last corner a field holds"
        );
        assert_eq!(tiled_bytes(&back_buffer, 4 * 0x8000, 0), None);
        assert_eq!(tiled_bytes(&back_buffer, 0, 0x8000), None);
    }

    #[test]
    fn a_rectangle_outside_either_surface_or_in_another_tile_mode_is_no_blit() {
        let source = surface(1920, 1071, 7680, Some(tile_mode::TILE_64_2D_32BPE));
        let copy = staging(source, surface(1920, 1071, 7680, None));
        let blit = Blit::of_staging(&copy).unwrap();
        assert_eq!(
            (blit.width, blit.height, blit.source.tiling, blit.target.tiling),
            (1920, 1071, Tiling::Tile64, Tiling::Linear)
        );
        assert_eq!(blit.source.pitch, 7680);
        assert!(Blit::of_staging(&StagingCopy { right: 1921, ..copy }).is_err());
        assert!(Blit::of_staging(&StagingCopy { bottom: 1072, ..copy }).is_err());
        assert!(Blit::of_staging(&StagingCopy { target_left: 1, ..copy }).is_err());
        assert!(Blit::of_staging(&StagingCopy { left: -4, ..copy }).is_err());
        assert!(Blit::of_staging(&StagingCopy { right: 0, left: 4, ..copy }).is_err(), "inside out");
        let other = SurfaceShape { tiling: Some(82), ..source };
        assert!(Blit::of_staging(&StagingCopy { source: other, ..copy }).is_err(), "Tile64 of 64-bit pixels");
        let empty = Blit::of_staging(&StagingCopy { right: 0, ..copy }).unwrap();
        assert!(empty.is_empty());
    }
}
