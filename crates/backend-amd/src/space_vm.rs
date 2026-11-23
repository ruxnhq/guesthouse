//! A GPU address space of its own for each space the core makes.
//!
//! amdgpu gives each open render node one VM. Guest processes place their own reservations, and two
//! processes often place them at the same addresses, so each space opens the node again and gets a
//! VM of its own. Memory is made on the backend's first open and taken into a space's open with a
//! dma-buf before it is mapped there. amdgpu won't hand out adopted host pages that way, so a space
//! adopts those pages itself, unless they reached the first open as a dma-buf of the guest's memory
//! file: see [`crate::udmabuf`]. A buffer taken in is kept while any of its mappings in the space
//! are, and let go after the last.

use crate::device::Device;
use crate::fault::{self, Fault};
use crate::fence_ib::{FenceIbs, Writer};
use crate::info::{self, HW_IP_VCN_ENC};
use crate::memory::{self, GemHandle, MapFlags};
use crate::reset::{self, ContextCalls, Contexts};
use crate::run_wait::{self, Patience, Waited};
use crate::share::{RunTimes, Share};
use crate::submit::{self, Context, Ib, Syncobj};
use crate::video_ib::{self, Copied, VideoIb};
use crate::video_runs::{Retired, VideoRuns};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs::File;
use std::io;
use std::os::fd::{OwnedFd, RawFd};
use std::time::Instant;

/// Which buffers a space has taken in, and its mappings of them. Kept apart from the files so the
/// counting can be tested without a card.
#[derive(Debug, Default)]
pub struct Books {
    /// By the buffer's handle on the first open: its handle here, and how many mappings use it.
    taken_in: HashMap<GemHandle, (GemHandle, u32)>,
    /// By address: the buffer's handle on the first open, and the mapping's place in it.
    mappings: BTreeMap<u64, Placed>,
}

/// Where in its buffer a mapping starts, and how long it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Placed {
    buffer: GemHandle,
    offset: u64,
    length: u64,
}

/// A mapping on its way out, and whether the buffer's handle here must be let go after it.
#[derive(Debug, PartialEq, Eq)]
pub struct Unmapping {
    pub handle: GemHandle,
    pub address: u64,
    pub length: u64,
    pub last_use: bool,
}

impl Books {
    /// The handle here for `buffer`, if it was taken in.
    pub fn handle_of(&self, buffer: GemHandle) -> Option<GemHandle> {
        self.taken_in.get(&buffer).map(|&(handle, _)| handle)
    }

    /// Notes `length` bytes of `buffer` from `offset`, mapped at `address` through `handle`.
    pub fn add(&mut self, buffer: GemHandle, handle: GemHandle, address: u64, offset: u64, length: u64) {
        self.taken_in.entry(buffer).or_insert((handle, 0)).1 += 1;
        self.mappings.insert(address, Placed { buffer, offset, length });
    }

    pub fn remove(&mut self, address: u64) -> Option<Unmapping> {
        let Placed { buffer, length, .. } = self.mappings.remove(&address)?;
        let (handle, uses) = self.taken_in.get_mut(&buffer)?;
        let handle = *handle;
        *uses -= 1;
        let last_use = *uses == 0;
        if last_use {
            self.taken_in.remove(&buffer);
        }
        Some(Unmapping { handle, address, length, last_use })
    }

    /// The handles here of every buffer taken in.
    pub fn handles(&self) -> Vec<GemHandle> {
        self.taken_in.values().map(|&(handle, _)| handle).collect()
    }

    /// Every mapping of `buffer` here.
    pub fn addresses_of(&self, buffer: GemHandle) -> Vec<u64> {
        let of_buffer = self.mappings.iter().filter(|(_, placed)| placed.buffer == buffer);
        of_buffer.map(|(&address, _)| address).collect()
    }

    /// The buffer behind an address, by its handle on the first open, and how far into it the
    /// address is.
    pub fn locate(&self, address: u64) -> Option<(GemHandle, u64)> {
        let (&start, placed) = self.mappings.range(..=address).next_back()?;
        (address - start < placed.length).then_some((placed.buffer, placed.offset + (address - start)))
    }

    /// Whether `bytes` from `address` lie inside one mapping here.
    pub fn holds(&self, address: u64, bytes: u64) -> bool {
        let Some((&start, placed)) = self.mappings.range(..=address).next_back() else {
            return false;
        };
        let end = start.saturating_add(placed.length);
        address.checked_add(bytes).is_some_and(|wanted| wanted <= end)
    }

    /// Whether every byte of `bytes` from `address` is mapped here, in one mapping or in several that
    /// follow one another, as a guest may lay two buffers end to end.
    pub fn covers(&self, address: u64, bytes: u64) -> bool {
        let Some(end) = address.checked_add(bytes) else {
            return false;
        };
        let mut reached = address;
        while reached < end {
            let mapped_to = self
                .mappings
                .range(..=reached)
                .next_back()
                .map(|(&start, placed)| start.saturating_add(placed.length));
            match mapped_to {
                Some(mapped_to) if mapped_to > reached => reached = mapped_to,
                _ => return false,
            }
        }
        true
    }
}

/// Where a buffer on the backend's first open came from, which says how a space takes it in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Made on the card, and handed over as a dma-buf.
    Made,
    /// Host pages at this address, of this size. A space adopts them again by the address, or takes
    /// them in as it does memory made on the card when the first open has them as a dma-buf.
    HostPages { address: u64, size: u64, as_dma_buf: bool },
}

/// One space's open of the render node, and what it holds.
#[derive(Debug)]
pub struct SpaceVm {
    file: File,
    books: Books,
    /// One amdgpu context for each engine family, made on first use and again after a reset.
    /// Closing the file frees them.
    contexts: Contexts<Context>,
    /// Whether work here has faulted, after which nothing more is run here.
    faults: fault::Watch,
    /// The command buffers that have the card write a fence, made on first use.
    fence_ibs: Option<FenceIbs>,
    /// The daemon's copies of the video commands run here, made on first use.
    video_ib: Option<VideoIb>,
    /// The video runs on the card, and the places in that buffer they hold.
    video_runs: VideoRuns<Syncobj>,
    /// The runs sent without waiting that the kernel hasn't been seen to retire, oldest first.
    on_card: VecDeque<OnCard>,
    /// The fences of runs seen to retire since they were last asked for.
    retired: Vec<GuestFence>,
    /// The video runs seen to retire since they were last asked for.
    video_retired: Vec<Retired>,
    /// When a video run here was last seen to retire.
    video_last_retired: Option<Instant>,
    /// Whether amdgpu refused to post a run's end to the daemon's eventfd, which is said once.
    video_wake_failed: bool,
    /// How each engine family gives way to the card's other users, see [`crate::share`].
    shares: HashMap<u32, Share>,
    /// Whether finished runs are kept for giving way, which only then judges them.
    keeps_finished: bool,
    /// Runs seen to retire since they were last asked for, with their times, while kept.
    finished: Vec<Finished>,
    /// How fast the card's clock runs, to read the times runs note.
    clock_khz: u32,
}

/// Where a run's fence is in guest memory, by guest physical address, and the value it ends with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuestFence {
    pub queue: u32,
    pub address: u64,
    pub value: u64,
}

/// A run on the card: the slot its fence writer is in, what the kernel signals when it retires, and
/// the fence the run writes last.
#[derive(Debug)]
struct OnCard {
    slot: usize,
    ip_type: u32,
    /// The card's clock in nanoseconds once the run was sent, if it could be read.
    sent: Option<u64>,
    retired: Syncobj,
    /// None once its queue has gone, as the fence's page may then be another queue's.
    fence: Option<GuestFence>,
}

/// A run of the space's that the card has been through: its engine family and when it ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Finished {
    pub ip_type: u32,
    pub times: RunTimes,
}

/// How a run on the card ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ran {
    Retired,
    /// amdgpu kept saying try again, so nothing ran, and the work can come again later.
    Refused,
    /// The kernel neither finished nor reset it in time.
    Hung,
    /// This run reached an address with nothing mapped, or amdgpu recorded such a fault for the
    /// space while it ran. Nothing more is run here.
    Faulted(Fault),
    /// Nothing was sent, as work here had already faulted in an earlier run. amdgpu keeps a space's
    /// last fault, so the fault is that earlier run's, not this one's.
    Stopped(Fault),
}

/// How sending the daemon's own work that waits for other runs went.
#[derive(Debug)]
pub enum Sent {
    /// It is on the card, and the kernel signals this when it retires.
    OnCard(Syncobj),
    /// amdgpu kept saying try again, so nothing was sent.
    Refused,
    /// Nothing was sent, as work here had already faulted.
    Stopped(Fault),
}

/// A run's command buffers with the slot's clock notes around them: one first, and one last, after
/// the fence writer, so the fence's cache flush counts as the run's own time.
fn with_clock_notes(ibs: &[Ib], slot: usize) -> Vec<Ib> {
    let [start, end] = FenceIbs::stamp_ibs(slot);
    let mut stamped = Vec::with_capacity(ibs.len() + 2);
    stamped.push(start);
    stamped.extend_from_slice(ibs);
    stamped.push(end);
    stamped
}

/// A time in the card's clock ticks, in nanoseconds.
fn ticks_to_ns(ticks: u64, clock_khz: u32) -> u64 {
    (u128::from(ticks) * 1_000_000 / u128::from(clock_khz.max(1))) as u64
}

impl SpaceVm {
    pub fn open(device: &Device) -> io::Result<Self> {
        Ok(Self {
            file: device.open_again()?,
            books: Books::default(),
            contexts: Contexts::default(),
            faults: fault::Watch::default(),
            fence_ibs: None,
            video_ib: None,
            video_runs: VideoRuns::default(),
            on_card: VecDeque::new(),
            retired: Vec::new(),
            video_retired: Vec::new(),
            video_last_retired: None,
            video_wake_failed: false,
            shares: HashMap::new(),
            keeps_finished: false,
            finished: Vec::new(),
            clock_khz: device.info().counter_freq_khz,
        })
    }

    /// The buffer behind an address here, by its handle on the first open, and the offset into it.
    pub fn locate(&self, address: u64) -> Option<(GemHandle, u64)> {
        self.books.locate(address)
    }

    /// Whether `bytes` from `address` lie inside one mapping here.
    pub fn holds(&self, address: u64, bytes: u64) -> bool {
        self.books.holds(address, bytes)
    }

    /// Whether every byte of `bytes` from `address` is mapped here, in one mapping or several.
    pub fn covers(&self, address: u64, bytes: u64) -> bool {
        self.books.covers(address, bytes)
    }

    /// The first command buffer not wholly mapped here. The card would fault reading it, and a fault
    /// costs every program on the card a reset.
    pub fn first_unmapped<'a>(&self, ibs: &'a [Ib]) -> Option<&'a Ib> {
        ibs.iter().find(|ib| !self.books.holds(ib.address, u64::from(ib.bytes)))
    }

    /// The fault that stopped this space, if its work has had one.
    pub fn faulted(&self) -> Option<Fault> {
        self.faults.found()
    }

    /// Runs command buffers already mapped here on the engine family `ip_type`, in order, and waits
    /// for the last to retire, as long as `patience` allows. Nothing is run once work here has
    /// faulted. amdgpu can record a fault a moment after its run retires, so there is a look before
    /// a run as well as after it. `queue` is the guest's queue the work is from, or `None` for the
    /// daemon's own.
    pub fn run(
        &mut self,
        ip_type: u32,
        queue: Option<u32>,
        ibs: &[Ib],
        patience: Patience,
    ) -> io::Result<Ran> {
        only_copies_for_the_video_ring(ip_type, ibs)?;
        if let Some(fault) = self.faults.look(&self.file) {
            return Ok(Ran::Stopped(fault));
        }
        let ran = self.submit_and_wait(ip_type, queue, ibs, patience);
        match (ran, self.faults.look(&self.file)) {
            (Ok(Ran::Retired), Some(fault)) => Ok(Ran::Faulted(fault)),
            (ran, _) => ran,
        }
    }

    /// Copies a submission's video commands into a free place of the daemon's video buffer here,
    /// made at the first: see [`crate::video_ib`] for why the guest's own buffers are never run on
    /// the video ring. Returns the place, or `None` while every place's run is still on the card.
    /// `alignment` is the card's for a new buffer.
    pub fn place_video(&mut self, alignment: u64, copied: &Copied) -> io::Result<Option<usize>> {
        self.take_back_video(Instant::now());
        if self.video_ib.is_none() {
            let made = VideoIb::make(&self.file, alignment)
                .map_err(|error| step("making the buffer for video commands", error))?;
            self.video_ib = Some(made);
        }
        let Some(slot) = self.video_runs.take_slot() else { return Ok(None) };
        self.video_ib.as_mut().expect("made above").write(slot, copied);
        Ok(Some(slot))
    }

    /// Frees a place in the video buffer whose run won't be sent.
    pub fn drop_video_slot(&mut self, slot: usize) {
        self.video_runs.give_back(slot);
    }

    /// Sends the video commands in `slot` to the card's video ring, laid out as `runs`, and doesn't
    /// wait for them. The run keeps its place until the kernel says it has retired, and the host
    /// then writes `fence`, if there is one: see [`Self::take_retired_video`]. A run that amdgpu
    /// refuses for now (`WouldBlock`) or at all keeps its place too, for the caller to send again
    /// or give back.
    pub fn send_video(
        &mut self,
        slot: usize,
        runs: &[Ib],
        queue: u32,
        fence: Option<GuestFence>,
        wake: Option<RawFd>,
        now: Instant,
    ) -> io::Result<()> {
        only_copies_for_the_video_ring(HW_IP_VCN_ENC, runs)?;
        // As in `run`: nothing more is sent to a space whose work has faulted. amdgpu keeps the
        // space's last fault, so it is an earlier run's, and this run is not sent rather than faulted.
        if let Some(fault) = self.faults.look(&self.file) {
            return Err(io::Error::other(format!(
                "work here reached {:#x} earlier, where nothing is mapped (fault status {:#x}), so nothing \
                 more is sent",
                fault.address, fault.status
            )));
        }
        let Some(own) = self.video_ib.as_ref().map(VideoIb::handle) else {
            return Err(io::Error::other("no buffer for video commands was made here"));
        };
        let retired = Syncobj::create(&self.file).map_err(|error| step("making a sync object", error))?;
        // The guest doesn't say which buffers the work uses, so it may use any mapped here.
        let mut buffers = self.books.handles();
        buffers.push(own);
        let submitted = self.contexts.submit(&self.file, HW_IP_VCN_ENC, Some(queue), |context| {
            submit::submit_all(&self.file, context, HW_IP_VCN_ENC, runs, &buffers, &retired).map(|_| ())
        });
        match submitted {
            Ok(()) => {
                // Without it the run is still seen to retire at the next poll.
                if let Some(wake) = wake
                    && let Err(error) = retired.post_to(&self.file, wake)
                    && !std::mem::replace(&mut self.video_wake_failed, true)
                {
                    log::debug!(
                        "amdgpu won't post a video run's end to an eventfd ({error}), so it is polled"
                    );
                }
                self.video_runs.sent(slot, retired, queue, fence, now);
                Ok(())
            }
            Err(error) => {
                retired.destroy(&self.file);
                Err(error)
            }
        }
    }

    /// The video runs the kernel has retired since this was last asked, with the fence the host is
    /// to write for each. The kernel says a run it reset has retired too.
    pub fn take_retired_video(&mut self, now: Instant) -> Vec<Retired> {
        self.take_back_video(now);
        std::mem::take(&mut self.video_retired)
    }

    /// Whether any video run of this space is on the card.
    #[cfg(test)]
    pub fn video_on_card(&self) -> bool {
        self.video_runs.any_on_card()
    }

    /// Looks at the video runs here: whether any is still on the card after those the kernel has
    /// retired are taken off, and when one was last seen to retire. The fences of those taken off
    /// wait for [`Self::take_retired_video`].
    pub fn video_state(&mut self, now: Instant) -> (bool, Option<Instant>) {
        self.take_back_video(now);
        (self.video_runs.any_on_card(), self.video_last_retired)
    }

    /// Frees the places of the video runs the kernel says have retired, and keeps what to say of
    /// them for [`Self::take_retired_video`].
    fn take_back_video(&mut self, now: Instant) {
        if !self.video_runs.any_on_card() {
            return;
        }
        let file = &self.file;
        // A sync object that can't be asked about is one the kernel has no run for.
        let retired = self.video_runs.retire(|signal| signal.signalled(file).unwrap_or(true), now);
        if !retired.is_empty() {
            self.video_last_retired = Some(now);
        }
        for (signal, run) in retired {
            signal.destroy(file);
            self.video_retired.push(run);
        }
    }

    /// Sync files of the guest's runs on the card that write `fence`, if one of them takes it to
    /// `value` or past it. Work that waits for all of them comes after the fence reaches the value.
    /// `None` if no run sent here does, as when the run is still to be sent.
    pub fn sync_files_reaching(&self, fence: u64, value: u64) -> io::Result<Option<Vec<OwnedFd>>> {
        let writes = |run: &&OnCard| run.fence.is_some_and(|written| written.address == fence);
        let reaches = |run: &OnCard| run.fence.is_some_and(|written| written.value >= value);
        if !self.on_card.iter().filter(writes).any(reaches) {
            return Ok(None);
        }
        let files = self.on_card.iter().filter(writes).map(|run| run.retired.export_sync_file(&self.file));
        files.collect::<io::Result<Vec<_>>>().map(Some)
    }

    /// Sends the daemon's own command buffers, already mapped here, to run once every sync file in
    /// `after` has signalled, and doesn't wait for them. Hand what it gives to [`Self::end_own`].
    pub fn send_after(&mut self, ip_type: u32, ibs: &[Ib], after: &[OwnedFd]) -> io::Result<Sent> {
        only_copies_for_the_video_ring(ip_type, ibs)?;
        if let Some(fault) = self.faults.look(&self.file) {
            return Ok(Sent::Stopped(fault));
        }
        let mut waits = Vec::with_capacity(after.len());
        for sync_file in after {
            match Syncobj::from_sync_file(&self.file, sync_file) {
                Ok(wait) => waits.push(wait),
                Err(error) => {
                    waits.iter().for_each(|wait| wait.destroy(&self.file));
                    return Err(step("taking in a run to wait for", error));
                }
            }
        }
        let sent = Syncobj::create(&self.file).map_err(|error| step("making a sync object", error));
        let submitted = sent.and_then(|signal| {
            // As in `submit_and_wait`: any buffer mapped here may be used.
            let buffers = self.books.handles();
            let submitted = self.contexts.submit(&self.file, ip_type, None, |context| {
                submit::submit_after(&self.file, context, ip_type, ibs, &buffers, &waits, &signal).map(|_| ())
            });
            match submitted {
                Ok(()) => Ok(Sent::OnCard(signal)),
                Err(error) => {
                    signal.destroy(&self.file);
                    match error.kind() {
                        io::ErrorKind::WouldBlock => Ok(Sent::Refused),
                        _ => Err(step("submitting", error)),
                    }
                }
            }
        });
        // The kernel holds the fences a submission waits for, so the objects can go now.
        waits.iter().for_each(|wait| wait.destroy(&self.file));
        submitted
    }

    /// An eventfd that becomes readable once `signal`, from [`Self::send_after`], is signalled.
    pub fn arm(&self, signal: &Syncobj) -> io::Result<OwnedFd> {
        signal.arm_eventfd(&self.file).map_err(|error| step("arming the eventfd", error))
    }

    /// How the daemon's own run from [`Self::send_after`] ended, once it retired or was given up on.
    pub fn end_own(&mut self, signal: Syncobj, retired: bool) -> Ran {
        signal.destroy(&self.file);
        match (retired, self.faults.look(&self.file)) {
            (false, _) => Ran::Hung,
            (true, Some(fault)) => Ran::Faulted(fault),
            (true, None) => Ran::Retired,
        }
    }

    fn submit_and_wait(
        &mut self,
        ip_type: u32,
        queue: Option<u32>,
        ibs: &[Ib],
        patience: Patience,
    ) -> io::Result<Ran> {
        let signal = Syncobj::create(&self.file).map_err(|error| step("making a sync object", error))?;
        // The guest doesn't say which buffers the work uses, so it may use any mapped here.
        let buffers = self.books.handles();
        let submitted = self.contexts.submit(&self.file, ip_type, queue, |context| {
            submit::submit_all(&self.file, context, ip_type, ibs, &buffers, &signal).map(|_| ())
        });
        let ran = match submitted {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(Ran::Refused),
            Err(error) => Err(step("submitting", error)),
            Ok(()) => signal
                .arm_eventfd(&self.file)
                .map_err(|error| step("arming the eventfd", error))
                .and_then(|event| run_wait::wait_for_run(&event, patience, ip_type))
                .map(|waited| match waited {
                    Waited::Retired { .. } => Ran::Retired,
                    Waited::Hung => Ran::Hung,
                }),
        };
        signal.destroy(&self.file);
        ran
    }

    /// Writes the command buffer that has the card store `value` at `fence` when a run is through,
    /// to go last in that run. Returns its slot with it, or `None` while every slot's run is still
    /// on the card.
    pub fn fence_writer(
        &mut self,
        alignment: u64,
        writer: Writer,
        fence: u64,
        value: u64,
    ) -> io::Result<Option<(usize, Ib)>> {
        self.take_back_slots(false);
        if self.fence_ibs.is_none() {
            let made =
                FenceIbs::make(&self.file, alignment).map_err(|error| step("making fence writers", error))?;
            self.fence_ibs = Some(made);
        }
        if let Some(written) = self.fence_ibs.as_mut().and_then(|ibs| ibs.write(writer, fence, value)) {
            return Ok(Some(written));
        }
        // Runs on other engines can retire before the oldest one does.
        self.take_back_slots(true);
        Ok(self.fence_ibs.as_mut().and_then(|ibs| ibs.write(writer, fence, value)))
    }

    /// Frees the slot of a fence writer whose run won't be sent.
    pub fn drop_fence_writer(&mut self, slot: usize) {
        if let Some(ibs) = &mut self.fence_ibs {
            ibs.give_back(slot);
        }
    }

    /// Sends command buffers already mapped here to run in order on the engine family `ip_type`,
    /// and doesn't wait for them. The last is the fence writer in `slot`, which writes `fence`.
    pub fn send(&mut self, ip_type: u32, ibs: &[Ib], slot: usize, fence: GuestFence) -> io::Result<()> {
        only_copies_for_the_video_ring(ip_type, ibs)?;
        let Some(fence_ibs) = &self.fence_ibs else {
            return Err(io::Error::other("no fence writers were made here"));
        };
        // As in `run`: nothing more is sent to a space whose work has faulted.
        if let Some(fault) = self.faults.look(&self.file) {
            return Err(io::Error::other(format!(
                "work here reached {:#x} earlier, where nothing is mapped (fault status {:#x}), so nothing \
                 more is sent",
                fault.address, fault.status
            )));
        }
        let retired = Syncobj::create(&self.file).map_err(|error| step("making a sync object", error))?;
        // The guest doesn't say which buffers the work uses, so it may use any mapped here.
        let mut buffers = self.books.handles();
        buffers.push(fence_ibs.handle());
        let stamped = with_clock_notes(ibs, slot);
        let submitted = self.contexts.submit(&self.file, ip_type, Some(fence.queue), |context| {
            submit::submit_all(&self.file, context, ip_type, &stamped, &buffers, &retired).map(|_| ())
        });
        match submitted {
            Ok(_) => {
                let sent = info::read_gpu_clock(&self.file, self.clock_khz).ok();
                self.on_card.push_back(OnCard { slot, ip_type, sent, retired, fence: Some(fence) });
                Ok(())
            }
            Err(error) => {
                retired.destroy(&self.file);
                Err(error)
            }
        }
    }

    /// Has the space keep its finished runs for [`Self::take_finished`], to give way.
    pub fn keep_finished(&mut self) {
        self.keeps_finished = true;
    }

    /// The runs seen to retire since this was last asked, looking at them all, with their times.
    pub fn take_finished(&mut self) -> Vec<Finished> {
        self.take_back_slots(true);
        std::mem::take(&mut self.finished)
    }

    /// When the runs still on the card that the card has been through ended, by engine family: the
    /// card notes a run's end before the kernel retires it.
    pub fn ends_on_card(&self) -> Vec<(u32, u64)> {
        let Some(fence_ibs) = &self.fence_ibs else { return Vec::new() };
        self.on_card
            .iter()
            .filter(|run| run.sent.is_some())
            .filter_map(|run| {
                let (started, ended) = fence_ibs.times(run.slot);
                (started != 0 && ended >= started).then(|| (run.ip_type, ticks_to_ns(ended, self.clock_khz)))
            })
            .collect()
    }

    /// Judges a finished run for giving way, with when the guest's own work last ended before it
    /// started: see [`Share::retired`].
    pub fn judge(&mut self, finished: Finished, own_end: u64) {
        self.shares.entry(finished.ip_type).or_default().retired(finished.times, own_end);
    }

    /// Whether a run may go to the engine family `ip_type` now, or should wait while the space gives
    /// way to others on the card. See [`crate::share`].
    pub fn may_send(&mut self, ip_type: u32) -> bool {
        let Some(share) = self.shares.get(&ip_type) else { return true };
        let Ok(now) = info::read_gpu_clock(&self.file, self.clock_khz) else { return true };
        if !share.contended(now) {
            return true;
        }
        self.take_back_slots(true);
        let on_card = self.on_card.iter().filter(|run| run.ip_type == ip_type).count();
        self.shares.get(&ip_type).is_none_or(|share| share.may_send(now, on_card))
    }

    /// The fences of the runs the kernel has retired since this was last asked, looking at them all.
    pub fn take_retired(&mut self) -> Vec<GuestFence> {
        self.take_back_slots(true);
        std::mem::take(&mut self.retired)
    }

    /// Forgets the fences of a queue that has gone.
    pub fn forget_queue(&mut self, queue: u32) {
        self.contexts.forget_queue(queue);
        self.video_runs.forget_queue(queue);
        self.video_retired.retain(|run| run.queue != queue);
        self.retired.retain(|fence| fence.queue != queue);
        for run in &mut self.on_card {
            run.fence = run.fence.filter(|fence| fence.queue != queue);
        }
    }

    /// Frees the slots of runs the kernel says have retired: the oldest ones, or with `all` any.
    fn take_back_slots(&mut self, all: bool) {
        let Some(fence_ibs) = &mut self.fence_ibs else { return };
        let mut still_on_card = VecDeque::new();
        while let Some(run) = self.on_card.pop_front() {
            // A sync object that can't be asked about is one the kernel has no run for.
            if run.retired.signalled(&self.file).unwrap_or(true) {
                run.retired.destroy(&self.file);
                let (started, ended) = fence_ibs.times(run.slot);
                if let Some(sent) = run.sent.filter(|_| self.keeps_finished) {
                    let ticks = |ticks| ticks_to_ns(ticks, self.clock_khz);
                    let times = RunTimes { sent, started: ticks(started), ended: ticks(ended) };
                    self.finished.push(Finished { ip_type: run.ip_type, times });
                }
                fence_ibs.give_back(run.slot);
                self.retired.extend(run.fence);
            } else if all {
                still_on_card.push_back(run);
            } else {
                self.on_card.push_front(run);
                break;
            }
        }
        still_on_card.append(&mut self.on_card);
        self.on_card = still_on_card;
    }

    /// Maps `length` bytes of `buffer`, a handle on `device`, from `offset`, at `address`.
    #[allow(clippy::too_many_arguments)]
    pub fn map(
        &mut self,
        device: &Device,
        buffer: GemHandle,
        origin: Origin,
        address: u64,
        offset: u64,
        length: u64,
        flags: MapFlags,
    ) -> io::Result<()> {
        let fresh = self.books.handle_of(buffer).is_none();
        let handle = match (self.books.handle_of(buffer), origin) {
            (Some(handle), _) => handle,
            (None, Origin::Made | Origin::HostPages { as_dma_buf: true, .. }) => {
                let shared =
                    memory::export(device, buffer).map_err(|error| step("handing the buffer over", error))?;
                memory::import(&self.file, &shared).map_err(|error| step("taking the buffer in", error))?
            }
            (None, Origin::HostPages { address, size, as_dma_buf: false }) => {
                memory::adopt_userptr(&self.file, address, size, false)
                    .map_err(|error| step("adopting the pages", error))?
            }
        };
        if let Err(error) = memory::map(&self.file, handle, address, offset, length, flags) {
            if fresh {
                memory::close(&self.file, handle);
            }
            return Err(error);
        }
        self.books.add(buffer, handle, address, offset, length);
        Ok(())
    }

    /// Takes back the mapping at `address`, if there is one.
    pub fn unmap(&mut self, address: u64) -> io::Result<()> {
        let Some(unmapping) = self.books.remove(address) else {
            return Ok(());
        };
        let result = memory::unmap(&self.file, unmapping.handle, unmapping.address, unmapping.length);
        if unmapping.last_use {
            memory::close(&self.file, unmapping.handle);
        }
        result
    }

    /// Takes back every mapping of `buffer`, as before it is freed. Returns where they were.
    pub fn forget(&mut self, buffer: GemHandle) -> Vec<u64> {
        let addresses = self.books.addresses_of(buffer);
        for &address in &addresses {
            if let Err(error) = self.unmap(address) {
                log::warn!("GEM_VA unmap at {address:#x}: {error}");
            }
        }
        addresses
    }
}

impl ContextCalls for File {
    type Context = Context;

    fn make(&self) -> io::Result<Context> {
        Context::create(self)
    }

    fn spoiled(&self, context: &Context, error: &io::Error) -> bool {
        let flags = context.reset_flags(self).ok();
        let spoiled = reset::spoiled_by_reset(error, flags);
        if spoiled {
            let flag = |bit| flags.map_or("unknown".to_string(), |flags| (flags & bit != 0).to_string());
            log::error!(
                "amdgpu refused work after a reset ({error}). Video memory lost: {}, this space's \
                 work hung the card: {}. The space's contexts are made again, and guest queues from \
                 before the reset fail",
                flag(reset::FLAG_VRAM_LOST),
                flag(reset::FLAG_GUILTY)
            );
        }
        spoiled
    }

    fn free(&self, context: Context) {
        context.destroy(self);
    }
}

/// `error`, saying which step it came from.
fn step(doing: &str, error: io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("{doing}: {error}"))
}

/// Why a command buffer a guest placed isn't sent to the video ring.
const NOT_A_COPY: &str = "the video ring is only given the daemon's copy of a guest's commands";

/// Refuses work for the video ring unless every command buffer lies in the daemon's own video
/// buffer, where no guest mapping can be. amdgpu maps a video command buffer with the CPU, and for
/// one that is guest pages that is a fault in the host's kernel: see [`crate::video_ib`]. Every
/// submission here passes this, so no caller can send the ring a guest's buffer by mistake.
fn only_copies_for_the_video_ring(ip_type: u32, ibs: &[Ib]) -> io::Result<()> {
    if ip_type == HW_IP_VCN_ENC && !ibs.iter().all(video_ib::holds) {
        return Err(io::Error::other(NOT_A_COPY));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUFFER: GemHandle = GemHandle(7);
    const HERE: GemHandle = GemHandle(3);
    const AT: u64 = 0x40_0000;
    const SIZE: u64 = 0x1_0000;

    /// A space with no card behind its file: every call to the kernel on it fails, so a test sees
    /// what is refused before the kernel is asked.
    #[test]
    fn a_fence_no_run_here_writes_has_nothing_to_wait_on() {
        let space = space_without_a_card();
        assert!(space.sync_files_reaching(0x1000, 1).unwrap().is_none());
    }

    fn space_without_a_card() -> SpaceVm {
        SpaceVm {
            file: File::open("/dev/null").unwrap(),
            books: Books::default(),
            contexts: Contexts::default(),
            faults: fault::Watch::default(),
            fence_ibs: None,
            video_ib: None,
            video_runs: VideoRuns::default(),
            on_card: VecDeque::new(),
            retired: Vec::new(),
            video_retired: Vec::new(),
            video_last_retired: None,
            video_wake_failed: false,
            shares: HashMap::new(),
            keeps_finished: false,
            finished: Vec::new(),
            clock_khz: 100_000,
        }
    }

    /// A command buffer where a guest places its own, and one inside the daemon's video buffer.
    const GUESTS: Ib = Ib { address: 0x4_2000_1000, bytes: 0x40 };
    const COPY: Ib = Ib { address: video_ib::ADDRESS + 256, bytes: 0x40 };

    #[test]
    fn a_run_notes_the_clock_first_and_again_after_its_fence() {
        let fence_writer = Ib { address: crate::fence_ib::ADDRESS, bytes: 32 };
        let [start, end] = FenceIbs::stamp_ibs(3);
        let stamped = with_clock_notes(&[GUESTS, COPY, fence_writer], 3);
        assert_eq!(stamped, [start, GUESTS, COPY, fence_writer, end]);
        assert_ne!(start, end);
    }

    #[test]
    fn the_cards_ticks_become_nanoseconds_at_its_own_rate() {
        assert_eq!(ticks_to_ns(100_000, 100_000), 1_000_000);
        assert_eq!(ticks_to_ns(27_000, 27_000), 1_000_000);
    }

    #[test]
    fn the_video_ring_takes_only_command_buffers_in_the_daemons_video_buffer() {
        use crate::info::{HW_IP_COMPUTE, HW_IP_DMA, HW_IP_GFX};
        let refused =
            |ibs: &[Ib]| only_copies_for_the_video_ring(HW_IP_VCN_ENC, ibs).unwrap_err().to_string();
        assert_eq!(refused(&[GUESTS]), NOT_A_COPY);
        assert_eq!(refused(&[COPY, GUESTS]), NOT_A_COPY, "one guest buffer among copies");
        let end = video_ib::ADDRESS + (video_ib::SLOTS * video_ib::BYTES) as u64;
        let past_the_end = Ib { address: end - 4, bytes: 8 };
        assert_eq!(refused(&[past_the_end]), NOT_A_COPY);
        assert!(only_copies_for_the_video_ring(HW_IP_VCN_ENC, &[COPY, COPY]).is_ok());
        // The other engines run a guest's buffers where they lie, as before.
        for ip_type in [HW_IP_GFX, HW_IP_COMPUTE, HW_IP_DMA] {
            assert!(only_copies_for_the_video_ring(ip_type, &[GUESTS]).is_ok());
        }
    }

    #[test]
    fn no_way_of_sending_work_gives_the_video_ring_a_guests_buffer() {
        let mut space = space_without_a_card();
        let waited = space.run(HW_IP_VCN_ENC, Some(1), &[GUESTS], Patience::RUN);
        assert_eq!(waited.unwrap_err().to_string(), NOT_A_COPY);
        let fence = GuestFence { queue: 1, address: 0x1000, value: 1 };
        let sent = space.send(HW_IP_VCN_ENC, &[GUESTS, COPY], 0, fence);
        assert_eq!(sent.unwrap_err().to_string(), NOT_A_COPY);
        let sent = space.send_video(0, &[COPY, GUESTS], 1, Some(fence), None, Instant::now());
        assert_eq!(sent.unwrap_err().to_string(), NOT_A_COPY);
        // Refused before the kernel was asked anything, so nothing is on the card.
        assert!(!space.video_on_card());
    }

    #[test]
    fn a_space_that_faulted_before_sends_nothing_and_says_the_fault_was_earlier() {
        use crate::info::HW_IP_DMA;
        let earlier = Fault { address: 0x1000, status: 0xf4_3a51 };
        let mut space = space_without_a_card();
        space.faults = fault::Watch::having_found(earlier);
        // The file has no card behind it, so a run that got as far as the kernel would fail.
        let ran = space.run(HW_IP_DMA, Some(1), &[GUESTS], Patience::RUN).unwrap();
        assert_eq!(ran, Ran::Stopped(earlier));
        // Video runs aren't waited for: one is refused as not sent, with the earlier fault.
        let sent = space.send_video(0, &[COPY], 1, None, None, Instant::now());
        assert_eq!(
            sent.unwrap_err().to_string(),
            "work here reached 0x1000 earlier, where nothing is mapped (fault status 0xf43a51), so nothing \
             more is sent"
        );
        assert!(!space.video_on_card());
    }

    #[test]
    fn video_commands_are_sent_only_from_a_buffer_that_was_made() {
        let mut copied = Copied::default();
        copied.place(8, 256).unwrap().copy_from_slice(&[0x8, 0x1]);
        let mut space = space_without_a_card();
        let failed = space.place_video(0x1000, &copied).unwrap_err().to_string();
        assert!(failed.starts_with("making the buffer for video commands: "), "{failed}");
        let sent = space.send_video(0, &copied.runs_in(0), 1, None, None, Instant::now());
        assert_eq!(sent.unwrap_err().to_string(), "no buffer for video commands was made here");
        assert!(!space.video_on_card());
        assert!(space.take_retired_video(Instant::now()).is_empty());
    }

    #[test]
    fn a_buffer_mapped_twice_is_let_go_after_its_second_mapping() {
        let mut books = Books::default();
        books.add(BUFFER, HERE, AT, 0, SIZE);
        books.add(BUFFER, HERE, AT + SIZE, SIZE, SIZE);
        assert_eq!(books.handle_of(BUFFER), Some(HERE));
        let first = books.remove(AT).unwrap();
        assert_eq!(first, Unmapping { handle: HERE, address: AT, length: SIZE, last_use: false });
        assert!(books.remove(AT + SIZE).unwrap().last_use);
        assert_eq!(books.handle_of(BUFFER), None);
    }

    #[test]
    fn an_address_with_nothing_mapped_has_nothing_to_take_back() {
        let mut books = Books::default();
        assert_eq!(books.remove(AT), None);
    }

    #[test]
    fn a_buffers_mappings_are_found_by_the_buffer() {
        let mut books = Books::default();
        books.add(BUFFER, HERE, AT, 0, SIZE);
        books.add(GemHandle(8), GemHandle(4), AT + SIZE, 0, SIZE);
        assert_eq!(books.addresses_of(BUFFER), vec![AT]);
    }

    #[test]
    fn a_buffer_mapped_twice_is_listed_once() {
        let mut books = Books::default();
        books.add(BUFFER, HERE, AT, 0, SIZE);
        books.add(BUFFER, HERE, AT + SIZE, SIZE, SIZE);
        books.add(GemHandle(8), GemHandle(4), AT + 2 * SIZE, 0, SIZE);
        let mut handles = books.handles();
        handles.sort_by_key(|handle| handle.0);
        assert_eq!(handles, vec![HERE, GemHandle(4)]);
    }

    #[test]
    fn an_address_is_found_in_its_buffer_by_the_mapping_s_offset() {
        let mut books = Books::default();
        books.add(BUFFER, HERE, AT, 0x3000, SIZE);
        assert_eq!(books.locate(AT), Some((BUFFER, 0x3000)));
        assert_eq!(books.locate(AT + 0x10), Some((BUFFER, 0x3010)));
        assert_eq!(books.locate(AT + SIZE), None);
        assert_eq!(books.locate(AT - 1), None);
    }

    #[test]
    fn a_range_is_covered_across_mappings_that_follow_one_another() {
        let mut books = Books::default();
        books.add(BUFFER, HERE, AT, 0, SIZE);
        books.add(GemHandle(8), GemHandle(4), AT + SIZE, 0, SIZE);
        books.add(GemHandle(9), GemHandle(5), AT + 3 * SIZE, 0, SIZE);
        assert!(books.covers(AT, SIZE));
        assert!(books.covers(AT + 0x100, 2 * SIZE - 0x100));
        assert!(books.covers(AT, 0));
        assert!(!books.covers(AT, 2 * SIZE + 1));
        assert!(!books.covers(AT + SIZE, 3 * SIZE));
        assert!(!books.covers(AT - 1, 2));
        assert!(!books.covers(AT + 2 * SIZE, 1));
        assert!(!books.covers(u64::MAX - 1, 4));
    }

    #[test]
    fn a_range_is_held_only_inside_one_mapping() {
        let mut books = Books::default();
        books.add(BUFFER, HERE, AT, 0, SIZE);
        assert!(books.holds(AT, SIZE));
        assert!(books.holds(AT + 0x100, 0x100));
        assert!(!books.holds(AT + 0x100, SIZE));
        assert!(!books.holds(AT - 0x100, 0x200));
        assert!(!books.holds(AT + SIZE, 4));
        assert!(!books.holds(u64::MAX - 1, 4));
    }
}
