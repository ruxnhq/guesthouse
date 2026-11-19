//! Submissions: an xe exec queue behind each of the guest's queues, made on its first work, and
//! what becomes of each batch.
//!
//! A batch that ends runs in the process's VM. xe writes the guest's fence at its end, as a user
//! fence, and signals a point of the queue's own timeline, by which a run that ended without
//! writing its fence is found. A batch that parks is a ring: see [`super::choices::Rings`].
//!
//! A context's submission also names marks of its own in its header: a monitored fence and a tag.
//! See [`super::marks`].

use super::choices::{self, Rings};
use super::marks::{Marks, Monitored};
use super::{CardWork, engines};
use crate::protocol::command_header::{Header, Length};
use crate::xe::abi::{EngineInstance, Keying, Sync};
use crate::xe::batch::{self, Ending};
use crate::xe::plan::{self, Backing, ExecVerdict, Point};
use crate::xe_query::Engine;
use guesthouse_backend::{
    BackendError, CommandBuffer, Doorbell, HostServices, SpaceId, Work, WorkSource, WorkStart,
};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// A run on the card whose end is waited for.
#[derive(Debug, Clone, Copy)]
pub(super) struct Run {
    /// The point of the queue's timeline xe signals when it ends, however it ends. Zero for a ring.
    pub point: u64,
    /// The guest's fence, by guest physical address, and the value this run brings it to.
    pub fence: u64,
    pub value: u64,
    /// Whether the card writes that fence. If not, the host does once the run has ended.
    pub card_writes_fence: bool,
    /// What a context's submission has the host write once it is done.
    pub marks: Option<Marks>,
    /// The batch: its GPU address, its length in bytes, and where it goes on if it chains.
    pub batch: u64,
    pub length: u64,
    pub chain: Option<u64>,
    /// When it was sent, and how many changes its space's mappings had had by then.
    pub sent: Option<Instant>,
    pub changes_before: u64,
}

#[derive(Debug)]
pub(super) struct Queue {
    pub space: SpaceId,
    /// The xe queue for batches that end, with the timeline its runs signal.
    ordinary: Option<(u32, u32)>,
    next_point: u64,
    pub in_flight: VecDeque<Run>,
    /// The xe queue for a ring, in the space's long-running VM, and the last fence value a ring
    /// submission named.
    ring: Option<u32>,
    ring_fence: Option<(u64, u64)>,
    /// The latest ring submission, and whether its queue's ban has been looked into.
    ring_run: Option<Run>,
    ring_banned_seen: bool,
}

/// Work checked and ready, waiting for its doorbell.
#[derive(Debug)]
pub(super) struct Prepared {
    queue: u32,
    /// `None` for a context's submission that runs nothing but has to wait for the runs before it.
    xe_queue: Option<u32>,
    batch: u64,
    syncs: Vec<Sync>,
    /// `None` for a ring, which signals nothing.
    run: Option<Run>,
    /// What a ring submission is, for the log if its queue is banned.
    ring_run: Option<Run>,
}

/// Why a span of GPU memory couldn't be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReadError {
    NotMapped,
    Unreadable,
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotMapped => "it isn't inside one mapping",
            Self::Unreadable => "it can't be read",
        })
    }
}

/// Why a submission is refused and its queue stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    SpaceFailed,
    HeaderSize(usize),
    PatchList,
    NoEngine(u32),
    NotMapped { address: u64, length: u64 },
    Unreadable { address: u64 },
    RunsOn { address: u64 },
    Ring,
    ContextRing,
    Xe(String),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SpaceFailed => f.write_str("its process's work was given up earlier"),
            Self::HeaderSize(size) => {
                write!(f, "its private data is {size} bytes, which is no header's size")
            }
            Self::PatchList => {
                f.write_str("it asks for addresses to be patched into the batch, which nothing does")
            }
            Self::NoEngine(node) => write!(f, "node {node} has no engine on this card"),
            Self::NotMapped { address, length } => {
                write!(f, "its batch at {address:#x} for {length:#x} bytes isn't inside one mapping")
            }
            Self::Unreadable { address } => write!(f, "the end of its batch at {address:#x} can't be read"),
            Self::RunsOn { address } => {
                write!(f, "its batch at {address:#x} neither ends, chains nor parks, and would run on")
            }
            Self::Ring => f.write_str("its batch parks and never ends, and rings are switched off"),
            Self::ContextRing => f.write_str(
                "its batch parks and never ends, so the marks a context's work names would never come",
            ),
            Self::Xe(error) => write!(f, "xe made no queue for it: {error}"),
        }
    }
}

/// What a submission's batch is, once checked.
enum Kind {
    /// Ends, or chains on at the address.
    Ends(Option<u64>),
    Ring,
}

impl CardWork {
    /// Checks a submission and prepares it. Nothing is sent before its doorbell.
    pub fn start(&mut self, host: &mut dyn HostServices, work: &Work<'_>, header: &Header) -> WorkStart {
        self.write_left_marks(host);
        let marks = (work.source == WorkSource::Context).then(|| Marks::of(host, work, header));
        // Work that may not run is dropped, and the guest hears so. Its marks, and a refused one's,
        // are written at once, as its process is losing its device.
        let dropped = !work.may_run || work.queue_failed;
        let unrun = if dropped {
            WorkStart::Finished
        } else {
            match self.prepare(host, work, header, marks) {
                Ok(Some(prepared)) => {
                    let doorbell = self.new_id();
                    self.prepared.insert(doorbell, prepared);
                    return WorkStart::Started { doorbell, work: 1 };
                }
                Ok(None) => WorkStart::Finished,
                Err(refused) => {
                    log::warn!(
                        "intel work: queue {:#x} in space {:?} stops: {refused}",
                        work.queue,
                        work.space
                    );
                    WorkStart::QueueFailed
                }
            }
        };
        match marks {
            Some(marks) if dropped || unrun != WorkStart::Finished => {
                self.write_marks(host, work.space, work.queue, &marks, "at once, as it won't run");
                unrun
            }
            Some(marks) => self.finish_unrun(host, work, marks),
            None => unrun,
        }
    }

    /// A context's submission that runs nothing is done at once, unless runs before it on its queue
    /// are not: then it waits for them, so its marks come in order. Its queue's fence is the core's
    /// to write when nothing waits, as for any work finished unrun.
    fn finish_unrun(&mut self, host: &mut dyn HostServices, work: &Work<'_>, marks: Marks) -> WorkStart {
        let in_flight = self.queues.get(&work.queue).is_some_and(|queue| !queue.in_flight.is_empty());
        let prepared =
            self.prepared.values().any(|prepared| prepared.queue == work.queue && prepared.run.is_some());
        if !(in_flight || prepared) {
            self.write_marks(host, work.space, work.queue, &marks, "at once, as nothing ran");
            return WorkStart::Finished;
        }
        let run = Run {
            point: 0,
            fence: work.fence,
            value: work.fence_value,
            card_writes_fence: false,
            marks: Some(Marks { queue_fence: Some((work.fence, work.fence_value)), ..marks }),
            batch: 0,
            length: 0,
            chain: None,
            sent: None,
            changes_before: 0,
        };
        let doorbell = self.new_id();
        let prepared = Prepared {
            queue: work.queue,
            xe_queue: None,
            batch: 0,
            syncs: Vec::new(),
            run: Some(run),
            ring_run: None,
        };
        self.prepared.insert(doorbell, prepared);
        WorkStart::Started { doorbell, work: 1 }
    }

    /// `Ok(None)` is work this stage doesn't run, or can't yet: it is finished unrun.
    fn prepare(
        &mut self,
        host: &mut dyn HostServices,
        work: &Work<'_>,
        header: &Header,
        marks: Option<Marks>,
    ) -> Result<Option<Prepared>, Refused> {
        if self.space_failed(work.space) {
            return Err(Refused::SpaceFailed);
        }
        let Some(commands) = work.commands else {
            log::debug!(
                "intel work: queue {:#x} names no commands, so its work is finished unrun",
                work.queue
            );
            return Ok(None);
        };
        if header.verdict == Length::Neither {
            return Err(Refused::HeaderSize(header.length));
        }
        let compact = header.compact.as_ref();
        if compact.is_some_and(|compact| compact.patch_list) {
            return Err(Refused::PatchList);
        }
        if !self.stage.runs(compact.map(|compact| compact.context_type)) {
            return Ok(None);
        }
        let engine = engines::for_node(&self.engines, work.node).ok_or(Refused::NoEngine(work.node))?;
        let kind = match self.ending(host, work.space, commands)? {
            Ending::Ends => Kind::Ends(None),
            Ending::Chains(to) => Kind::Ends(Some(to)),
            Ending::Parks if marks.is_some() => return Err(Refused::ContextRing),
            Ending::Parks if choices::RINGS == Rings::LongRunning => Kind::Ring,
            Ending::Parks => return Err(Refused::Ring),
            Ending::RunsOn => return Err(Refused::RunsOn { address: commands.address }),
        };
        let space = work.space;
        let queue = self.queues.entry(work.queue).or_insert_with(|| Queue {
            space,
            ordinary: None,
            next_point: 0,
            in_flight: VecDeque::new(),
            ring: None,
            ring_fence: None,
            ring_run: None,
            ring_banned_seen: false,
        });
        let length = u64::from(commands.length) * choices::COMMAND_LENGTH_UNIT;
        let run_of = |point: u64, chain: Option<u64>| Run {
            point,
            fence: work.fence,
            value: work.fence_value,
            card_writes_fence: true,
            marks: None,
            batch: commands.address,
            length,
            chain,
            sent: None,
            changes_before: 0,
        };
        match kind {
            Kind::Ring => {
                let xe_queue = match queue.ring {
                    Some(xe_queue) => xe_queue,
                    None => {
                        let vm = self.ring_vm(space).map_err(|error| Refused::Xe(error.to_string()))?;
                        let made =
                            self.new_queue(vm, engine).map_err(|error| Refused::Xe(error.to_string()))?;
                        self.queues.get_mut(&work.queue).expect("made above").ring = Some(made);
                        made
                    }
                };
                // The queue's fence is due when the ring ends, which is when the guest's driver
                // stops it and waits for this value. The ring itself writes only a fence of the
                // driver's own. So the card is asked to write this one after the batch, if it is
                // mapped and aligned as xe wants. A long-running VM signals nothing else.
                let held = self.spaces.get(&space).ok_or(Refused::SpaceFailed)?;
                let fence_mapped = work.fence_gpu_address.is_multiple_of(plan::FENCE_ALIGNMENT)
                    && held.holding(work.fence_gpu_address, plan::FENCE_ALIGNMENT).is_some();
                let syncs = if fence_mapped {
                    vec![Sync::UserFence { address: work.fence_gpu_address, value: work.fence_value }]
                } else {
                    log::warn!(
                        "intel work: queue {:#x}'s fence is not mapped, so nothing will say when its ring ends",
                        work.queue
                    );
                    Vec::new()
                };
                let queue = self.queues.get_mut(&work.queue).expect("made above");
                queue.ring_fence = Some((work.fence, work.fence_value));
                Ok(Some(Prepared {
                    queue: work.queue,
                    xe_queue: Some(xe_queue),
                    batch: commands.address,
                    syncs,
                    run: None,
                    ring_run: Some(run_of(0, None)),
                }))
            }
            Kind::Ends(chain) => {
                let (xe_queue, done) = match queue.ordinary {
                    Some(made) => made,
                    None => {
                        let vm = self.spaces.get(&space).ok_or(Refused::SpaceFailed)?.vm;
                        let made =
                            self.make_ordinary(vm, engine).map_err(|error| Refused::Xe(error.to_string()))?;
                        self.queues.get_mut(&work.queue).expect("made above").ordinary = Some(made);
                        made
                    }
                };
                let queue = self.queues.get_mut(&work.queue).expect("made above");
                queue.next_point += 1;
                let point = queue.next_point;
                let held = self.spaces.get(&space).ok_or(Refused::SpaceFailed)?;
                let last_bind =
                    (held.bind_point > 0).then_some(Point { timeline: held.binds, point: held.bind_point });
                let mapped = |address: u64| {
                    address.is_multiple_of(plan::FENCE_ALIGNMENT)
                        && held.holding(address, plan::FENCE_ALIGNMENT).is_some()
                };
                let mut run = run_of(point, chain);
                let fence = match marks {
                    // A context's: the card writes the monitored fence the guest waits on, if it is
                    // mapped and aligned as xe wants, and the host the rest when the run has ended.
                    // One run takes one fence of the card's, and the guest waits on this one.
                    Some(marks) => {
                        let card_writes = marks.monitored.filter(|monitored| {
                            mapped(monitored.gpu_address) && self.card_may_raise(host, monitored)
                        });
                        // The host raises the monitored fence too, which changes nothing once the
                        // card has, and covers a run that never gets there.
                        run.marks =
                            Some(Marks { queue_fence: Some((work.fence, work.fence_value)), ..marks });
                        if let Some(Monitored { guest_address, value, .. }) = card_writes {
                            (run.fence, run.value) = (guest_address, value);
                        } else {
                            run.card_writes_fence = false;
                        }
                        card_writes.map(|monitored| (monitored.gpu_address, monitored.value))
                    }
                    // The card writes the fence where the guest watches it, if that is mapped and
                    // aligned as xe wants. Otherwise the host writes it when the run has ended.
                    None => {
                        mapped(work.fence_gpu_address).then_some((work.fence_gpu_address, work.fence_value))
                    }
                };
                let syncs = plan::exec_syncs(last_bind, fence, Point { timeline: done, point });
                Ok(Some(Prepared {
                    queue: work.queue,
                    xe_queue: Some(xe_queue),
                    batch: commands.address,
                    syncs,
                    run: Some(run),
                    ring_run: None,
                }))
            }
        }
    }

    /// Whether the card may write a monitored fence as a run's own: only to raise it, past what it
    /// holds and what the card is already on its way to write there. The host never lowers one.
    fn card_may_raise(&self, host: &dyn HostServices, monitored: &Monitored) -> bool {
        let holds = host.fence_value(monitored.guest_address).unwrap_or(u64::MAX);
        let in_flight = self.queues.values().flat_map(|queue| queue.in_flight.iter());
        let unsent = self.prepared.values().filter_map(|prepared| prepared.run.as_ref());
        let coming = in_flight
            .chain(unsent)
            .filter(|run| run.card_writes_fence && run.fence == monitored.guest_address)
            .map(|run| run.value)
            .max();
        monitored.value > holds && coming.is_none_or(|value| monitored.value > value)
    }

    /// An xe queue in a VM with the timeline its runs signal.
    fn make_ordinary(&mut self, vm: u32, engine: EngineInstance) -> std::io::Result<(u32, u32)> {
        let done = self.xe.create_syncobj()?;
        match self.new_queue(vm, engine) {
            Ok(queue) => Ok((queue, done)),
            Err(error) => {
                let _ = self.xe.destroy_syncobj(done);
                Err(error)
            }
        }
    }

    /// An xe queue for a guest's queue, ring or not. One on a render engine has its colour cache
    /// keyed as decided at the start, which is what the record told the guest's driver; the
    /// other engines draw nothing through a render target and xe keys none of theirs.
    ///
    /// A render queue xe won't key by binding table is not made without it: the guest's driver
    /// leaves out the flushes keying by index alone needs, so its draws would land in the wrong
    /// surfaces without a word. Its queue stops instead, which the guest is told.
    fn new_queue(&mut self, vm: u32, engine: EngineInstance) -> std::io::Result<u32> {
        let keying = if engine.class == Engine::RENDER { self.render_keying } else { Keying::BindingIndex };
        self.xe.create_queue(vm, engine, keying).inspect_err(|error| {
            if keying == Keying::BindingTableAndIndex {
                log::warn!(
                    "intel work: xe refused a render queue with its colour cache keyed by binding table \
                     ({error}), and none is made without that: the guest's driver was told the cache is \
                     keyed so and leaves out the flushes that keying by index alone needs"
                );
            }
        })
    }

    /// How a batch ends, read from its last bytes where they lie: a buffer through the CPU's view
    /// of it, guest pages through the core.
    fn ending(
        &mut self,
        host: &mut dyn HostServices,
        space: SpaceId,
        commands: CommandBuffer,
    ) -> Result<Ending, Refused> {
        let (address, length) = (commands.address, u64::from(commands.length) * choices::COMMAND_LENGTH_UNIT);
        let not_mapped = Refused::NotMapped { address, length };
        let held = self.spaces.get(&space).ok_or(Refused::SpaceFailed)?;
        if held.holding(address, length).is_none() || length == 0 {
            return Err(not_mapped);
        }
        let tail_length = length.min(batch::ENDING_BYTES as u64) & !3;
        let skipped = length - tail_length;
        let tail =
            self.read_gpu(host, space, address + skipped, tail_length).map_err(|error| match error {
                ReadError::NotMapped => Refused::NotMapped { address, length },
                ReadError::Unreadable => Refused::Unreadable { address: address + skipped },
            })?;
        Ok(batch::ending_at(&tail, address + skipped))
    }

    /// Reads bytes of a space at a GPU address, all of which must lie in one mapping: of a buffer
    /// through the CPU's view of it, of guest pages through the core.
    pub(super) fn read_gpu(
        &mut self,
        host: &mut dyn HostServices,
        space: SpaceId,
        address: u64,
        length: u64,
    ) -> Result<Vec<u8>, ReadError> {
        let held = self.spaces.get(&space).ok_or(ReadError::NotMapped)?;
        let (memory, offset) = held.holding(address, length).ok_or(ReadError::NotMapped)?;
        let mut bytes = vec![0u8; usize::try_from(length).map_err(|_| ReadError::Unreadable)?];
        let in_buffer =
            matches!(self.blocks.get(&memory).map(|block| block.backing), Some(Backing::Buffer { .. }));
        let read = if in_buffer {
            self.cpu_view(memory).is_ok_and(|view| view.read(offset, &mut bytes))
        } else {
            host.read_guest_span(space, address, &mut bytes) == Some(bytes.len())
        };
        if read { Ok(bytes) } else { Err(ReadError::Unreadable) }
    }

    /// Sends prepared work. A refusal for now keeps it for the next ring of the same doorbell.
    pub fn ring_doorbell(&mut self, doorbell: u64) -> Result<Doorbell, BackendError> {
        let Some(prepared) = self.prepared.get(&doorbell) else {
            return Err(BackendError::new("a doorbell for work that isn't prepared"));
        };
        let Some(space) = self.queues.get(&prepared.queue).map(|queue| queue.space) else {
            self.prepared.remove(&doorbell);
            return Err(BackendError::new("a doorbell for a queue that has gone"));
        };
        let Some(xe_queue) = prepared.xe_queue else {
            // Nothing to send: it waits behind the runs before it.
            let prepared = self.prepared.remove(&doorbell).expect("found above");
            let queue = self.queues.get_mut(&prepared.queue).expect("found above");
            queue.in_flight.extend(prepared.run);
            return Ok(Doorbell::Rung);
        };
        // A ring's VM signals nothing, so its binds are waited for here rather than by xe.
        if prepared.run.is_none() {
            let ring = self.spaces.get(&space).and_then(|space| space.ring.as_ref());
            if ring.is_some_and(|ring| !ring.binds_in_place()) {
                return Ok(Doorbell::NotYet);
            }
        }
        let attempted = prepared.run.or(prepared.ring_run);
        let mut sent = self.xe.exec(xe_queue, prepared.batch, &prepared.syncs);
        // A ring that xe won't take with a fence still runs without one. Its end then goes
        // unsaid, which is better than no ring.
        let refused = sent.as_ref().is_err_and(|error| plan::exec_verdict(error) == ExecVerdict::Failed);
        if refused && prepared.ring_run.is_some() && !prepared.syncs.is_empty() {
            log::warn!(
                "intel work: xe refused a ring with a fence ({}), so it is sent with none",
                sent.as_ref().expect_err("just seen")
            );
            sent = self.xe.exec(xe_queue, prepared.batch, &[]);
        }
        match sent.as_ref().map_err(plan::exec_verdict) {
            Ok(()) => {
                let prepared = self.prepared.remove(&doorbell).expect("found above");
                let changes_before = self.spaces.get(&space).map_or(0, |held| held.changes_made);
                if let Some(queue) = self.queues.get_mut(&prepared.queue) {
                    let stamped = |run: Run| Run { sent: Some(Instant::now()), changes_before, ..run };
                    if let Some(run) = prepared.run {
                        queue.in_flight.push_back(stamped(run));
                    }
                    if let Some(run) = prepared.ring_run {
                        queue.ring_run = Some(stamped(run));
                        queue.ring_banned_seen = false;
                        return Ok(Doorbell::RungToStay);
                    }
                }
                Ok(Doorbell::Rung)
            }
            Err(ExecVerdict::NotYet) => Ok(Doorbell::NotYet),
            Err(verdict) => {
                let error = sent.expect_err("matched as an error");
                let gone = self.prepared.remove(&doorbell);
                let prepared_queue = gone.as_ref().map_or(0, |prepared| prepared.queue);
                // Nothing of it will run, so a context's marks are written now.
                if let Some(marks) = gone.and_then(|prepared| prepared.run).and_then(|run| run.marks) {
                    // The core writes the queue's own fence for work that never ran.
                    let marks = Marks { queue_fence: None, ..marks };
                    self.left_marks.push((space, prepared_queue, marks, "after xe refused its run"));
                }
                if verdict == ExecVerdict::Lost {
                    self.explain_cancel(prepared_queue, space, attempted);
                    self.fail_space(
                        space,
                        "xe cancelled a submission, as it does once a queue is reset or banned",
                    );
                } else {
                    log::error!(
                        "intel work: xe refused a submission, which is this backend's fault: {error}"
                    );
                }
                Err(BackendError::new(format!("xe refused the submission: {error}")))
            }
        }
    }

    /// Looks at runs on the card for ones xe ended early: it stands in at the fence of each, and a
    /// queue xe banned takes its whole process's work with it.
    pub fn poll(&mut self, host: &mut dyn HostServices, now: Instant) {
        self.write_left_marks(host);
        if self
            .last_look
            .is_some_and(|last| now.saturating_duration_since(last) < choices::LOOK_AT_RUNS_EVERY)
        {
            return;
        }
        self.last_look = Some(now);
        let ids: Vec<u32> = self.queues.keys().copied().collect();
        for id in ids {
            self.look_at_runs(host, id, now);
            self.look_at_ring(host, id, now);
        }
    }

    fn look_at_runs(&mut self, host: &mut dyn HostServices, id: u32, now: Instant) {
        let Some(queue) = self.queues.get_mut(&id) else { return };
        let Some((xe_queue, done)) = queue.ordinary else { return };
        let space = queue.space;
        let mut ended = Vec::new();
        while let Some(run) = queue.in_flight.front().copied() {
            // A context's submission that ran nothing waits only for the runs before it.
            let reached = run.point == 0
                || self.xe.point_reached(done, run.point, Duration::ZERO).unwrap_or_else(|error| {
                    log::warn!("intel work: queue {id:#x}: asking xe whether a run ended failed: {error}");
                    false
                });
            if !reached {
                break;
            }
            queue.in_flight.pop_front();
            ended.push(run);
        }
        // The first run that ended without its fence, which is the one xe gave up on, and how many
        // after it were lost with it. A run whose fence the host writes was cut short if xe banned
        // its queue.
        let mut cut_short: Option<Run> = None;
        let mut lost_with_it = 0;
        for run in ended {
            let lost = if run.card_writes_fence {
                stand_in(host, id, run.fence, run.value)
            } else {
                run.point != 0 && self.xe.queue_banned(xe_queue).unwrap_or(true)
            };
            if lost {
                match cut_short {
                    None => cut_short = Some(run),
                    Some(_) => lost_with_it += 1,
                }
            }
            if let Some(marks) = run.marks {
                let when = if run.point == 0 { "after the runs before it" } else { "after its run" };
                self.write_marks(host, space, id, &marks, when);
            }
        }
        // A run that ended without its fence was cut short. Whether the queue was banned for it
        // decides if the process goes on; a question xe won't answer counts as banned, as in Mesa.
        if let Some(run) = cut_short
            && self.xe.queue_banned(xe_queue).unwrap_or(true)
        {
            let behind = self.queues.get(&id).map_or(0, |queue| queue.in_flight.len());
            let lines = self.ban_lines(host, id, &run, lost_with_it, behind, now);
            for line in lines {
                log::warn!("{line}");
            }
            self.fail_space(space, "xe banned one of its queues after a hang or a fault");
        }
    }

    fn look_at_ring(&mut self, host: &mut dyn HostServices, id: u32, now: Instant) {
        let Some(queue) = self.queues.get(&id) else { return };
        let (Some(xe_queue), space) = (queue.ring, queue.space) else { return };
        if queue.ring_banned_seen || !self.xe.queue_banned(xe_queue).unwrap_or(false) {
            return;
        }
        let (fence, run) = (queue.ring_fence, queue.ring_run);
        if let Some(queue) = self.queues.get_mut(&id) {
            queue.ring_banned_seen = true;
        }
        if let Some((fence, value)) = fence {
            stand_in(host, id, fence, value);
        }
        if let Some(run) = run {
            let lines = self.ban_lines(host, id, &run, 0, 0, now);
            for line in lines {
                log::warn!("{line}");
            }
        }
        self.fail_space(space, "xe banned the queue its ring ran on");
    }

    /// Destroys what backs a queue. Work prepared for it and not yet sent goes with it.
    pub fn close_queue(&mut self, queue: u32) {
        let mut doorbells: Vec<u64> = self
            .prepared
            .iter()
            .filter(|(_, prepared)| prepared.queue == queue)
            .map(|(&doorbell, _)| doorbell)
            .collect();
        // Doorbells count up, so this is the order the work came in.
        doorbells.sort_unstable();
        let unsent: Vec<Prepared> =
            doorbells.iter().filter_map(|doorbell| self.prepared.remove(doorbell)).collect();
        let Some(gone) = self.queues.remove(&queue) else { return };
        // A context's marks still to come are written, in order, as nothing will run now. The
        // queue's own fence goes back to the core with it, so it is left alone.
        let runs = gone.in_flight.iter().copied().chain(unsent.iter().filter_map(|prepared| prepared.run));
        for marks in runs.filter_map(|run| run.marks) {
            let marks = Marks { queue_fence: None, ..marks };
            self.left_marks.push((gone.space, queue, marks, "as its queue went"));
        }
        let (ordinary, done) = gone.ordinary.unzip();
        for xe_queue in [ordinary, gone.ring].into_iter().flatten() {
            if let Err(error) = self.xe.destroy_queue(xe_queue) {
                log::warn!("intel work: destroying queue {queue:#x}'s xe queue failed: {error}");
            }
        }
        if let Some(done) = done
            && let Err(error) = self.xe.destroy_syncobj(done)
        {
            log::warn!("intel work: destroying queue {queue:#x}'s sync object failed: {error}");
        }
    }

    /// Whether prepared work sends anything to the card when its doorbell rings. A context's
    /// submission that runs nothing but waits for the runs before it sends nothing.
    pub fn sends(&self, doorbell: u64) -> bool {
        self.prepared.get(&doorbell).is_some_and(|prepared| prepared.xe_queue.is_some())
    }

    pub fn live_queues(&self) -> usize {
        self.queues.len()
    }
}

/// Writes a fence the card never wrote, for a run that will never get there. Says whether it had to.
fn stand_in(host: &mut dyn HostServices, queue: u32, fence: u64, value: u64) -> bool {
    match host.fence_value(fence) {
        Ok(reached) if reached >= value => false,
        Ok(_) => {
            log::warn!(
                "intel work: queue {queue:#x}: xe ended a run before fence value {value:#x} was written, so \
                 the host writes it"
            );
            if let Err(error) = host.write_u64(fence, value) {
                log::warn!("intel work: standing in for a lost run: {error}");
            }
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::command_header::tests::compact as compact_header;
    use crate::stage::Stage;
    use crate::work::testing::Rig;
    use crate::xe::abi;
    use crate::xe::fake::Call;

    /// A space with a batch of one store, then the end, and the fence block: [`Rig::new`].
    fn ready(stage: Stage) -> Rig {
        let mut rig = Rig::new(stage);
        let mut dwords = batch::store_qword(rig.data_address() + 0x40, 0xC0FFEE).to_vec();
        dwords.push(batch::BATCH_END);
        rig.write_batch(&dwords);
        rig
    }

    #[test]
    fn a_batch_that_ends_runs_and_the_card_writes_the_guests_fence() {
        let mut rig = ready(Stage::Everything);
        let work = rig.submission(1, 7);
        let start = rig.start(&work);
        let WorkStart::Started { doorbell, work: 1 } = start else { panic!("{start:?}") };
        assert!(rig.xe.calls_named("exec").is_empty(), "nothing is sent before the doorbell");
        assert_eq!(rig.work.ring_doorbell(doorbell).unwrap(), Doorbell::Rung);
        let Some(Call::Exec { batch, syncs, .. }) = rig.xe.calls_named("exec").pop() else { panic!() };
        assert_eq!(batch, rig.batch_address());
        // The last bind is waited for, the guest's fence is the user fence, and the queue's own
        // timeline is signalled.
        assert!(matches!(syncs[0], Sync::After { .. }));
        assert_eq!(syncs[1], Sync::UserFence { address: rig.fence_gpu_address(), value: 7 });
        assert!(matches!(syncs[2], Sync::Signal { point: 1, .. }));
        assert_eq!(rig.fence(), 7, "written by the card, not by the host");
        assert_eq!(rig.data_qword(0x40), 0xC0FFEE);
        rig.poll();
        assert!(!rig.work.space_failed(rig.space));
        assert!(rig.host.written.is_empty(), "the host stood in for nothing");
    }

    #[test]
    fn a_queue_gets_one_xe_queue_on_its_nodes_engine_at_its_first_work() {
        let mut rig = ready(Stage::Everything);
        for value in 1..=3 {
            let work = rig.submission(1, value);
            let WorkStart::Started { doorbell, .. } = rig.start(&work) else { panic!() };
            rig.work.ring_doorbell(doorbell).unwrap();
        }
        let made = rig.xe.calls_named("create_queue");
        assert_eq!(made.len(), 1);
        let Call::CreateQueue { engine, .. } = &made[0] else { panic!() };
        assert_eq!(engine.class, crate::xe_query::Engine::COMPUTE, "node 5");
        assert_eq!(rig.fence(), 3);
        rig.work.close_queue(1);
        assert_eq!(rig.work.live_queues(), 0);
        assert_eq!(rig.xe.calls_named("destroy_queue").len(), 1);
    }

    #[test]
    fn work_that_may_not_run_or_that_the_stage_leaves_out_is_finished_unrun() {
        let mut rig = ready(Stage::Compute);
        let mut work = rig.submission(1, 1);
        work.may_run = false;
        assert_eq!(rig.start(&work), WorkStart::Finished);
        let mut work = rig.submission(1, 1);
        work.queue_failed = true;
        assert_eq!(rig.start(&work), WorkStart::Finished);
        // Direct3D 12's context type, at the stage that runs compute only.
        let work = rig.submission(1, 1).context_type(7);
        assert_eq!(rig.start(&work), WorkStart::Finished);
        // A context's submission names no commands yet.
        let mut work = rig.submission(1, 1);
        work.commands = None;
        assert_eq!(rig.start(&work), WorkStart::Finished);
        assert!(rig.xe.calls_named("create_queue").is_empty() && rig.xe.calls_named("exec").is_empty());
        let work = rig.submission(1, 1);
        assert!(matches!(rig.start(&work), WorkStart::Started { .. }), "OpenCL's does run");
    }

    #[test]
    fn a_submission_the_kernel_driver_would_refuse_stops_its_queue() {
        let mut rig = ready(Stage::Everything);
        let mut work = rig.submission(1, 1);
        work.private.truncate(100);
        assert_eq!(rig.start(&work), WorkStart::QueueFailed, "no header's size");
        let work = rig.submission(1, 1).patch_list();
        assert_eq!(rig.start(&work), WorkStart::QueueFailed);
        let mut work = rig.submission(1, 1);
        work.node = 9;
        assert_eq!(rig.start(&work), WorkStart::QueueFailed, "a node with no engine");
        assert!(rig.xe.calls_named("exec").is_empty());
    }

    #[test]
    fn a_batch_that_is_not_mapped_or_does_not_end_is_refused_before_xe_sees_it() {
        let mut rig = ready(Stage::Everything);
        let mut work = rig.submission(1, 1);
        work.commands = Some(CommandBuffer { address: 0x9_0000_0000, length: 64 });
        assert_eq!(rig.start(&work), WorkStart::QueueFailed, "nothing is mapped there");
        let mut work = rig.submission(1, 1);
        work.commands = Some(CommandBuffer { address: rig.batch_address() + 0x1000 - 8, length: 64 });
        assert_eq!(rig.start(&work), WorkStart::QueueFailed, "it runs past its mapping's end");
        rig.write_batch(&[0x7A00_0004, 0x1234, 0x5678]);
        let work = rig.submission(1, 1);
        assert_eq!(rig.start(&work), WorkStart::QueueFailed, "no end");
        assert!(rig.xe.calls_named("create_queue").is_empty());
    }

    #[test]
    fn a_batch_in_guest_pages_is_read_through_the_core() {
        let mut rig = ready(Stage::Everything);
        // The data pages are guest pages: put a batch there.
        let dwords = [batch::NOOP, batch::BATCH_END];
        rig.write_data(0x800, &batch::bytes(&dwords));
        let mut work = rig.submission(1, 1);
        work.commands = Some(CommandBuffer { address: rig.data_address() + 0x800, length: 8 });
        assert!(matches!(rig.start(&work), WorkStart::Started { .. }));
        rig.write_data(0x800, &batch::bytes(&[batch::NOOP, batch::NOOP]));
        assert_eq!(rig.start(&work), WorkStart::QueueFailed);
    }

    #[test]
    fn a_full_queue_is_not_yet_and_the_same_doorbell_rings_again() {
        let mut rig = ready(Stage::Everything);
        let work = rig.submission(1, 4);
        let WorkStart::Started { doorbell, .. } = rig.start(&work) else { panic!() };
        rig.xe.fail("exec", libc::EAGAIN);
        assert_eq!(rig.work.ring_doorbell(doorbell).unwrap(), Doorbell::NotYet);
        assert_eq!(rig.fence(), 0);
        rig.xe.heal();
        assert_eq!(rig.work.ring_doorbell(doorbell).unwrap(), Doorbell::Rung);
        assert_eq!(rig.fence(), 4);
        assert!(rig.work.ring_doorbell(doorbell).is_err(), "sent once");
    }

    #[test]
    fn a_cancelled_submission_fails_its_process_and_another_refusal_only_the_call() {
        let mut rig = ready(Stage::Everything);
        let work = rig.submission(1, 1);
        let WorkStart::Started { doorbell, .. } = rig.start(&work) else { panic!() };
        rig.xe.fail("exec", libc::EINVAL);
        assert!(rig.work.ring_doorbell(doorbell).is_err());
        assert!(!rig.work.space_failed(rig.space), "a refusal that is our own fault bans nothing");
        let work = rig.submission(1, 2);
        let WorkStart::Started { doorbell, .. } = rig.start(&work) else { panic!() };
        rig.xe.fail("exec", libc::ECANCELED);
        assert!(rig.work.ring_doorbell(doorbell).is_err());
        assert!(rig.work.space_failed(rig.space));
        rig.xe.heal();
        let work = rig.submission(1, 3);
        assert_eq!(rig.start(&work), WorkStart::QueueFailed, "nothing more of this process runs");
    }

    #[test]
    fn a_run_xe_ended_early_is_stood_in_for_and_a_banned_queue_fails_only_its_own_process() {
        let mut rig = ready(Stage::Everything);
        let other = rig.second_space();
        rig.xe.hold_runs();
        let work = rig.submission(1, 9);
        let WorkStart::Started { doorbell, .. } = rig.start(&work) else { panic!() };
        rig.work.ring_doorbell(doorbell).unwrap();
        rig.poll();
        assert!(rig.host.written.is_empty(), "still running: nothing to stand in for");
        rig.xe.lose_held_runs();
        rig.poll();
        assert_eq!(rig.host.written, [(rig.fence_guest_address(), 9)], "the host wrote the fence");
        assert_eq!(rig.fence(), 9);
        assert!(rig.work.space_failed(rig.space));
        let work = rig.submission(1, 10);
        assert_eq!(rig.start(&work), WorkStart::QueueFailed);
        // Another process of the same guest goes on.
        assert!(!rig.work.space_failed(other));
        let work = rig.submission_in(other, 2, 1);
        let WorkStart::Started { doorbell, .. } = rig.start(&work) else { panic!() };
        assert_eq!(rig.work.ring_doorbell(doorbell).unwrap(), Doorbell::Rung);
    }

    #[test]
    fn a_batch_that_faults_is_found_the_same_way() {
        let mut rig = Rig::new(Stage::Everything);
        let mut dwords = batch::store_qword(0x9_0000_0000, 1).to_vec();
        dwords.push(batch::BATCH_END);
        rig.write_batch(&dwords);
        let work = rig.submission(1, 5);
        let WorkStart::Started { doorbell, .. } = rig.start(&work) else { panic!() };
        rig.work.ring_doorbell(doorbell).unwrap();
        assert_eq!(rig.fence(), 0, "the card wrote no fence");
        rig.poll();
        assert_eq!(rig.fence(), 5);
        assert!(rig.work.space_failed(rig.space));
        assert_eq!(rig.xe.faults(), [0x9_0000_0000]);
    }

    #[test]
    fn a_fence_that_is_not_mapped_is_written_by_the_host_when_the_run_ends() {
        let mut rig = ready(Stage::Everything);
        let mut work = rig.submission(1, 6);
        work.fence_gpu_address = 0x8_0000_0000;
        let WorkStart::Started { doorbell, .. } = rig.start(&work) else { panic!() };
        rig.work.ring_doorbell(doorbell).unwrap();
        let Some(Call::Exec { syncs, .. }) = rig.xe.calls_named("exec").pop() else { panic!() };
        assert!(!syncs.iter().any(|sync| matches!(sync, Sync::UserFence { .. })));
        rig.poll();
        assert_eq!(rig.fence(), 6);
        assert!(!rig.work.space_failed(rig.space), "the run went through: nothing was banned");
    }

    #[test]
    fn runs_are_looked_at_no_more_often_than_every_fifty_milliseconds() {
        let mut rig = ready(Stage::Everything);
        let work = rig.submission(1, 1);
        let WorkStart::Started { doorbell, .. } = rig.start(&work) else { panic!() };
        rig.xe.hold_runs();
        rig.work.ring_doorbell(doorbell).unwrap();
        let now = Instant::now();
        rig.work.poll(&mut rig.host, now);
        rig.work.poll(&mut rig.host, now + Duration::from_millis(10));
        assert_eq!(rig.xe.calls_named("point_reached").len(), 1);
        rig.work.poll(&mut rig.host, now + Duration::from_millis(60));
        assert_eq!(rig.xe.calls_named("point_reached").len(), 2);
    }

    /// A ring as Intel's compute driver submits it: work, then a semaphore wait, and no end.
    fn ring(rig: &mut Rig) {
        let mut dwords = batch::store_qword(rig.data_address() + 0x40, 0xBEEF).to_vec();
        dwords.extend([batch::SEMAPHORE_WAIT_HEADER | 1 << 15 | 1 << 12, 1, 0x4000_1000, 0, 0]);
        rig.write_batch(&dwords);
    }

    #[test]
    fn a_ring_runs_on_a_long_running_queue_and_its_fence_is_for_when_it_ends() {
        let mut rig = Rig::new(Stage::Everything);
        ring(&mut rig);
        let work = rig.submission(1, 3);
        let WorkStart::Started { doorbell, .. } = rig.start(&work) else { panic!() };
        assert_eq!(rig.work.ring_doorbell(doorbell).unwrap(), Doorbell::RungToStay);
        let Some(Call::Exec { queue, syncs, .. }) = rig.xe.calls_named("exec").pop() else { panic!() };
        assert_eq!(
            syncs,
            [Sync::UserFence { address: rig.fence_gpu_address(), value: 3 }],
            "the card writes the fence after the batch, and nothing is signalled"
        );
        assert_eq!(rig.fence(), 0, "a parked ring has not reached it");
        let Some(Call::CreateQueue { vm, .. }) = rig.xe.calls_named("create_queue").pop() else { panic!() };
        assert_eq!(rig.xe.vm_flags(vm), Some(abi::VM_LONG_RUNNING));
        assert_eq!(rig.data_qword(0x40), 0xBEEF, "the work before the wait ran");
        rig.poll();
        assert!(!rig.work.space_failed(rig.space) && rig.host.written.is_empty());
        // xe bans the ring's queue: the host stands in at the fence and the process stops.
        rig.xe.ban(queue);
        rig.poll_later();
        assert_eq!(rig.host.written, [(rig.fence_guest_address(), 3)]);
        assert!(rig.work.space_failed(rig.space));
    }

    #[test]
    fn a_ring_whose_fence_is_not_mapped_runs_with_none() {
        let mut rig = Rig::new(Stage::Everything);
        ring(&mut rig);
        let mut work = rig.submission(1, 3);
        work.fence_gpu_address = 0x8_0000_0000;
        let WorkStart::Started { doorbell, .. } = rig.start(&work) else { panic!() };
        assert_eq!(rig.work.ring_doorbell(doorbell).unwrap(), Doorbell::RungToStay);
        let Some(Call::Exec { syncs, .. }) = rig.xe.calls_named("exec").pop() else { panic!() };
        assert!(syncs.is_empty(), "{syncs:?}");
    }

    #[test]
    fn a_ring_xe_refuses_with_a_fence_is_sent_again_with_none() {
        let mut rig = Rig::new(Stage::Everything);
        ring(&mut rig);
        let work = rig.submission(1, 3);
        let WorkStart::Started { doorbell, .. } = rig.start(&work) else { panic!() };
        rig.xe.fail_call(rig.xe.calls().len(), libc::EOPNOTSUPP);
        assert_eq!(rig.work.ring_doorbell(doorbell).unwrap(), Doorbell::RungToStay);
        let execs = rig.xe.calls_named("exec");
        let [Call::Exec { syncs: first, .. }, Call::Exec { syncs: second, .. }] = &execs[..] else {
            panic!("{execs:?}")
        };
        assert!(!first.is_empty() && second.is_empty());
    }

    #[test]
    fn a_queue_can_carry_a_ring_and_batches_that_end_side_by_side() {
        let mut rig = Rig::new(Stage::Everything);
        ring(&mut rig);
        let work = rig.submission(1, 1);
        let WorkStart::Started { doorbell, .. } = rig.start(&work) else { panic!() };
        rig.work.ring_doorbell(doorbell).unwrap();
        rig.write_batch(&[batch::BATCH_END]);
        let work = rig.submission(1, 2);
        let WorkStart::Started { doorbell, .. } = rig.start(&work) else { panic!() };
        rig.work.ring_doorbell(doorbell).unwrap();
        assert_eq!(rig.fence(), 2);
        let vms: Vec<u32> = rig
            .xe
            .calls_named("create_queue")
            .iter()
            .map(|call| match call {
                Call::CreateQueue { vm, .. } => *vm,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(vms.len(), 2);
        assert_ne!(vms[0], vms[1], "one in each of the process's two address spaces");
        rig.work.close_queue(1);
        assert_eq!(rig.xe.calls_named("destroy_queue").len(), 2);
    }

    /// The node numbers of the render, copy and compute engines.
    const RENDER_NODE: u32 = 0;
    const COPY_NODE: u32 = 2;
    const COMPUTE_NODE: u32 = 5;

    /// How each xe queue made so far was keyed, with its engine class.
    fn keyings(rig: &Rig) -> Vec<(u16, Keying)> {
        rig.xe
            .calls_named("create_queue")
            .iter()
            .map(|call| match call {
                Call::CreateQueue { engine, keying, .. } => (engine.class, *keying),
                _ => unreachable!(),
            })
            .collect()
    }

    /// Sends one batch that ends from each of the three nodes, each as its own queue.
    fn one_on_each_node(rig: &mut Rig) {
        for (queue, node) in [(1, RENDER_NODE), (2, COPY_NODE), (3, COMPUTE_NODE)] {
            let mut work = rig.submission(queue, 1);
            work.node = node;
            let WorkStart::Started { doorbell, .. } = rig.start(&work) else { panic!("node {node}") };
            assert_eq!(rig.work.ring_doorbell(doorbell).unwrap(), Doorbell::Rung, "node {node}");
        }
    }

    #[test]
    fn with_the_keying_decided_every_render_queue_and_no_other_is_keyed_by_binding_table() {
        let mut rig = Rig::keyed(Stage::Everything, Keying::BindingTableAndIndex);
        rig.write_batch(&[batch::BATCH_END]);
        one_on_each_node(&mut rig);
        let table = Keying::BindingTableAndIndex;
        assert_eq!(
            keyings(&rig),
            [
                (Engine::RENDER, table),
                (Engine::COPY, Keying::BindingIndex),
                (Engine::COMPUTE, Keying::BindingIndex)
            ]
        );
        // A ring on the render engine is a render queue like any other; one on compute is not.
        ring(&mut rig);
        for (queue, node) in [(4, RENDER_NODE), (5, COMPUTE_NODE)] {
            let mut work = rig.submission(queue, 1);
            work.node = node;
            let WorkStart::Started { doorbell, .. } = rig.start(&work) else { panic!("node {node}") };
            assert_eq!(rig.work.ring_doorbell(doorbell).unwrap(), Doorbell::RungToStay, "node {node}");
        }
        assert_eq!(keyings(&rig)[3..], [(Engine::RENDER, table), (Engine::COMPUTE, Keying::BindingIndex)]);
        assert!(!rig.work.space_failed(rig.space));
    }

    #[test]
    fn without_it_no_queue_is_keyed_by_binding_table() {
        let mut rig = Rig::new(Stage::Everything);
        rig.write_batch(&[batch::BATCH_END]);
        one_on_each_node(&mut rig);
        ring(&mut rig);
        let mut work = rig.submission(4, 1);
        work.node = RENDER_NODE;
        let WorkStart::Started { doorbell, .. } = rig.start(&work) else { panic!() };
        rig.work.ring_doorbell(doorbell).unwrap();
        let made = keyings(&rig);
        assert_eq!(made.len(), 4);
        assert!(made.iter().all(|(_, keying)| *keying == Keying::BindingIndex), "{made:?}");
    }

    #[test]
    fn a_render_queue_xe_wont_key_stops_and_is_not_made_any_other_way() {
        let mut rig = Rig::keyed(Stage::Everything, Keying::BindingTableAndIndex);
        rig.write_batch(&[batch::BATCH_END]);
        rig.xe.refuse_keying(libc::EINVAL);
        let mut work = rig.submission(1, 1);
        work.node = RENDER_NODE;
        assert_eq!(rig.start(&work), WorkStart::QueueFailed);
        assert_eq!(keyings(&rig), [(Engine::RENDER, Keying::BindingTableAndIndex)], "no plain queue after");
        assert!(rig.xe.calls_named("exec").is_empty());
        // The ring of a render queue is refused the same way.
        ring(&mut rig);
        let mut work = rig.submission(2, 1);
        work.node = RENDER_NODE;
        assert_eq!(rig.start(&work), WorkStart::QueueFailed);
        assert_eq!(keyings(&rig).len(), 2);
        assert!(keyings(&rig).iter().all(|(_, keying)| *keying == Keying::BindingTableAndIndex));
        // The process's other engines go on.
        rig.write_batch(&[batch::BATCH_END]);
        let work = rig.submission(3, 1);
        let WorkStart::Started { doorbell, .. } = rig.start(&work) else { panic!() };
        assert_eq!(rig.work.ring_doorbell(doorbell).unwrap(), Doorbell::Rung);
        assert_eq!(rig.fence(), 1);
        assert!(!rig.work.space_failed(rig.space));
    }

    #[test]
    fn the_header_used_in_these_tests_is_a_compact_one() {
        assert_eq!(Header::read(&compact_header()).verdict, Length::Compact);
    }
}
