//! A guest's work on the card: real buffers, one xe address space for each guest process with the
//! guest's mappings at the guest's own addresses, and an xe queue behind each of the guest's.
//!
//! [`CardWork`] is what the backend's trait methods call once the stage is past listening. Its
//! parts are in the modules here: [`memory`] for buffers and guest pages, [`spaces`] for address
//! spaces and their mappings, [`queues`] for submissions and what became of them, [`marks`] for what
//! a context's submission has written when it is done, [`copies`] and [`blits`] for pictures on
//! their way out, and [`moves`] for an allocation's contents on its first lock. [`choices`] holds
//! what the card has yet to confirm.

mod bans;
pub mod blits;
pub mod cap;
pub mod choices;
pub mod copies;
pub mod engines;
pub mod marks;
mod memory;
mod moves;
mod queues;
#[cfg(test)]
mod session_tests;
mod spaces;
#[cfg(test)]
pub(crate) mod testing;
mod wake;

use crate::card::CardFacts;
use crate::stage::Stage;
use crate::xe::abi::{EngineInstance, Keying};
use crate::xe::device::Xe;
use crate::xe::plan::Placements;
use crate::xe_query::Engine;
use guesthouse_backend::{MemoryId, SpaceId};
use std::collections::HashMap;
use std::time::Instant;

pub use queues::Refused;

/// See the module documentation.
pub struct CardWork {
    xe: Box<dyn Xe + Send>,
    stage: Stage,
    placements: Placements,
    engines: Vec<Engine>,
    /// How every queue on a render engine has its colour cache keyed, decided once at the start
    /// and told to the guest's driver in the record: see [`IntelBackend::render_keying`].
    ///
    /// [`IntelBackend::render_keying`]: crate::IntelBackend::render_keying
    render_keying: Keying,
    /// The engine whose cycle counter is the GPU's clock, and its rate.
    clock: Option<(EngineInstance, u32)>,
    next_id: u64,
    blocks: HashMap<MemoryId, memory::Block>,
    /// Bytes of buffers held, against `cap`.
    held: u64,
    cap: u64,
    spaces: HashMap<SpaceId, spaces::Space>,
    queues: HashMap<u32, queues::Queue>,
    prepared: HashMap<u64, queues::Prepared>,
    /// A context's marks whose run will never end, as xe refused it or its queue went, to be
    /// written at the next chance: each with its space and queue, and why.
    left_marks: Vec<(SpaceId, u32, marks::Marks, &'static str)>,
    last_look: Option<Instant>,
    /// The copy engine's copies, in the daemon's own space.
    blits: blits::Blits,
    /// First locks' moves into the guest's pages.
    moves: moves::Moves,
    /// What wakes the daemon when a run the host finishes has ended.
    wake: wake::Wake,
}

impl std::fmt::Debug for CardWork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CardWork")
            .field("stage", &self.stage)
            .field("blocks", &self.blocks.len())
            .field("spaces", &self.spaces.len())
            .field("queues", &self.queues.len())
            .finish_non_exhaustive()
    }
}

impl CardWork {
    /// `None` if the card's memory regions give nowhere to put a buffer. `cap` is the most video
    /// memory the guest may hold: see [`cap::video_cap`]. `render_keying` is how every queue made
    /// on a render engine has its colour cache keyed.
    pub fn new(
        xe: Box<dyn Xe + Send>,
        card: &CardFacts,
        stage: Stage,
        cap: u64,
        render_keying: Keying,
    ) -> Option<Self> {
        let clock_engine = crate::xe::plan::one_engine_per_class(&card.engines).first().copied();
        let rate = card.main_gt().map_or(0, |gt| gt.reference_clock);
        Some(Self {
            xe,
            stage,
            placements: Placements::of(&card.regions)?,
            engines: card.engines.clone(),
            render_keying,
            clock: clock_engine.filter(|_| rate != 0).map(|engine| (engine, rate)),
            next_id: 0,
            blocks: HashMap::new(),
            held: 0,
            cap,
            spaces: HashMap::new(),
            queues: HashMap::new(),
            prepared: HashMap::new(),
            left_marks: Vec::new(),
            last_look: None,
            blits: blits::Blits::new(card.main_gt().map_or(0, |gt| gt.ip.major)),
            moves: moves::Moves::for_card(card.main_gt().map(|gt| gt.ip)),
            wake: wake::Wake::default(),
        })
    }

    pub fn stage(&self) -> Stage {
        self.stage
    }

    pub fn render_keying(&self) -> Keying {
        self.render_keying
    }

    /// The most video memory the guest may hold.
    pub fn cap(&self) -> u64 {
        self.cap
    }

    fn new_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// The GPU's clock in nanoseconds, from an engine's cycle counter, or `None` if xe won't say.
    pub fn clock(&mut self) -> Option<u64> {
        let (engine, rate) = self.clock?;
        let read = self.xe.engine_cycles(engine).ok()?;
        Some(nanoseconds(read.cycles, rate))
    }
}

/// Engine cycles as nanoseconds, for a counter that ticks `rate` times a second.
fn nanoseconds(cycles: u64, rate: u32) -> u64 {
    const NANOSECONDS: u128 = 1_000_000_000;
    (u128::from(cycles) * NANOSECONDS / u128::from(rate.max(1))).min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycles_become_nanoseconds_without_overflow() {
        assert_eq!(nanoseconds(19_200_000, 19_200_000), 1_000_000_000);
        assert_eq!(nanoseconds(1, 19_200_000), 52);
        assert_eq!(nanoseconds(u64::MAX, 1), u64::MAX, "it stops at the most, past 584 years");
        assert_eq!(nanoseconds(5, 0), 5_000_000_000, "a rate of zero is not divided by");
    }

    #[test]
    fn the_gpu_clock_is_an_engines_cycle_counter() {
        let (mut work, xe) = testing::work(Stage::Everything);
        let first = work.clock().unwrap();
        let second = work.clock().unwrap();
        assert!(second > first);
        assert_eq!(xe.calls_named("engine_cycles").len(), 2);
        xe.fail("engine_cycles", libc::EINVAL);
        assert_eq!(work.clock(), None);
    }
}
