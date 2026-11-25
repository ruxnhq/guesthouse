//! Getting a queue's submissions onto the GPU.

use crate::channel::{
    AREA_SIZE, Channel, ChannelPlace, Engine, LIST_AREA, LOCAL_MEMORY_ALIGN, LOCAL_MEMORY_BYTES, ListChannel,
    ListFull, Seat,
};
use crate::list_repair;
use crate::pushbuffer::{ENTRY_SIZE, Header, RingEntry, ZERO_EXTENDED_BASE, has_no_extended_base};
use crate::queue_layout::SubchannelUse;
use crate::ring_position::has_ring_position_flag;
use crate::ring_request::RingMove;
use crate::rm::Rm;
use crate::{NvidiaBackend, UNFINISHED_ENTRIES_LIMIT, memory_error};
use guesthouse_backend::{HostServices, SpaceId, Work, WorkSource, WorkStart};
use std::borrow::Cow;
use std::time::{Duration, Instant};

// The fields of a submission's private data.
const LIST_OFFSET: usize = 0x0C;
const ENTRIES: usize = 0x14;
const CONTEXT_LIST_ENTRIES: usize = 0x18;
const LIST_ADDRESS: usize = 0x1C;
const RING_END: usize = 0x78;
const RING_ADDRESS: usize = 0x7C;
/// The handle of the ring the queue moves to, in the submission that ends on the old ring.
const MOVE_HANDLE: usize = 0x84;
const SMALLEST_PRIVATE_DATA: usize = 0x84;

/// How much of a guest's ring is read when standing in for the GPU.
const RING_BYTES: usize = 0x10000;

/// A new ring starts with one entry, the rest of the commands the move was among, and the guest's
/// driver counts on from there.
const FIRST_POSITION_ON_NEW_RING: u32 = 1;

/// How long a queue's later work is held for a move's fence. The core calls a queue stuck when a
/// fence takes this long, so a move that hasn't gone by then never will.
const MOVE_TIMEOUT: Duration = Duration::from_secs(5);

/// The guest writes this method to end a copy, which a channel here is refused. The launch method
/// does the same in the same two words.
const COPY_SEMAPHORE_HEADER: u32 = 0x2001_80C1;
const COPY_LAUNCH_HEADER: u32 = 0x2001_80C0;
const COPY_SEMAPHORE_OPERATION_RELEASE: u32 = 1;
const COPY_LAUNCH_RELEASE_ONE_WORD: u32 = 0xC;

/// The host's own semaphore method, and the words of a release up to its payload.
const HOST_SEMAPHORE_METHOD: u32 = 0x5C;
const HOST_SEMAPHORE_WORDS: u32 = 5;

/// What a queue's commands run on.
#[derive(Debug)]
pub(crate) enum QueueChannel {
    /// Making the channel failed once, and it isn't tried twice.
    Unusable,
    Ring {
        channel: Channel,
        ring_address: u64,
    },
    List(ListChannel),
}

impl QueueChannel {
    fn channel(&self) -> Option<&Channel> {
        match self {
            Self::Unusable => None,
            Self::Ring { channel, .. } => Some(channel),
            Self::List(list) => Some(&list.channel),
        }
    }

    pub(crate) fn close<R: Rm>(
        self,
        device: &mut crate::rm::device::Device<R>,
        space: &crate::rm::device::AddressSpace,
    ) -> Result<(), crate::rm::RmError> {
        match self {
            Self::Unusable => Ok(()),
            Self::Ring { channel, .. } => channel.close(device, space),
            Self::List(list) => list.channel.close(device, space),
        }
    }
}

/// A ring the GPU moves to once it has run what the old ring holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingMove {
    ring_address: u64,
    ring_entries: u32,
    /// The move submission's fence. It retires with that submission, whose last method is the move.
    fence: u64,
    reached: u64,
    registered: Instant,
}

/// A queue, as far as the GPU goes.
#[derive(Debug)]
pub(crate) struct LiveQueue {
    pub(crate) space: SpaceId,
    pub(crate) channel: QueueChannel,
    /// Making a channel is tried once.
    channel_tried: bool,
    /// Where the last submission ended.
    position: Option<u32>,
    /// The GPU address just past the commands of the last list.
    list_end: Option<u64>,
    /// A move registered with the GPU, which goes with the next doorbell.
    pending_move: Option<PendingMove>,
    /// Giving the channel shader local memory is tried once.
    local_memory_tried: bool,
}

impl LiveQueue {
    fn new(space: SpaceId) -> Self {
        Self {
            space,
            channel: QueueChannel::Unusable,
            channel_tried: false,
            position: None,
            list_end: None,
            pending_move: None,
            local_memory_tried: false,
        }
    }
}

/// The kinds of context whose driver sets a pool on one channel for all of them: OpenCL and CUDA.
/// Others aren't read, so games pay nothing for it.
const POOL_SHARING_HINTS: [u32; 2] = [3, 5];

fn mirrors_pool(work: &Work<'_>) -> bool {
    POOL_SHARING_HINTS.contains(&work.client_hint)
}

/// The slice for a new ring channel whose context has other queues, if it gets one of its own.
///
/// Queues of one context wait on each other: a Direct3D 12 queue is three of them, and each
/// submission comes to all three. Every channel is a channel group of its own, and one that waits
/// for another keeps the GPU until its slice ends. A context with one queue has nobody to wait
/// for, so it keeps the RM's slice, which serves it better. A slice set for every graphics channel
/// is already on the group, and stays.
fn shared_context_timeslice(engine: Engine, context_queues: u32, everyone: u64, shared: u64) -> Option<u64> {
    (engine == Engine::Graphics && context_queues > 1 && everyone == 0 && shared != 0).then_some(shared)
}

fn word(data: &[u8], at: usize) -> u32 {
    data.get(at..at + 4).map_or(0, |bytes| u32::from_le_bytes(bytes.try_into().expect("four bytes")))
}

fn long(data: &[u8], at: usize) -> u64 {
    data.get(at..at + 8).map_or(0, |bytes| u64::from_le_bytes(bytes.try_into().expect("eight bytes")))
}

fn entry_at(ring: &[u8], index: u32) -> RingEntry {
    let at = index as usize * ENTRY_SIZE % ring.len();
    let bytes = ring.get(at..at + ENTRY_SIZE).and_then(|entry| entry.try_into().ok());
    RingEntry::parse(bytes.unwrap_or([0; ENTRY_SIZE]))
}

/// The commands at a GPU address, if all of them can be read.
fn read_words(host: &dyn HostServices, space: SpaceId, address: u64, count: u32) -> Option<Vec<u32>> {
    let mut bytes = vec![0; count as usize * 4];
    let read = host.read_at_gpu_address(space, address, &mut bytes)?;
    (read == bytes.len()).then(|| {
        let (words, _) = bytes.as_chunks::<4>();
        words.iter().map(|word| u32::from_le_bytes(*word)).collect()
    })
}

/// The commands each ring entry of a listing points at, for those that can be read.
fn command_buffers(host: &dyn HostServices, space: SpaceId, listing: &[u8]) -> Vec<Vec<u32>> {
    let (entries, _) = listing.as_chunks::<ENTRY_SIZE>();
    let read = |entry: &[u8; ENTRY_SIZE]| {
        let entry = RingEntry::parse(*entry);
        let mut bytes = vec![0; entry.length as usize * 4];
        (entry.length != 0 && host.read_guest_span(space, entry.address, &mut bytes) == Some(bytes.len()))
            .then(|| bytes.as_chunks::<4>().0.iter().map(|word| u32::from_le_bytes(*word)).collect())
    };
    entries.iter().filter_map(read).collect()
}

/// The largest power of two entries that fit a mapping, if a channel can count that far.
fn ring_entries_of(mapping_length: u64) -> Option<u32> {
    let entries = mapping_length / ENTRY_SIZE as u64;
    u32::try_from(1u64 << (u64::BITS - 1 - entries.max(1).leading_zeros())).ok()
}

/// A list as a channel of this class can run it. The guest's driver starts some lists with an entry
/// that sets the extended base to zero, whatever card it's on: NVENC through CUDA does, on the
/// encoder's queue and on a copy queue. A channel with no such opcode faults on the entry, and the
/// rest of the list never runs. At zero the base changes nothing, so the entry is left out. Any
/// other entry stays, and so would another base, for the channel to refuse.
fn without_zero_extended_bases(channel_class: u32, listing: &[u8]) -> Cow<'_, [u8]> {
    let (entries, rest) = listing.as_chunks::<ENTRY_SIZE>();
    if !has_no_extended_base(channel_class) || !entries.iter().any(sets_zero_extended_base) {
        return Cow::Borrowed(listing);
    }
    let kept = entries.iter().filter(|entry| !sets_zero_extended_base(entry));
    Cow::Owned(kept.flatten().chain(rest).copied().collect())
}

fn sets_zero_extended_base(entry: &[u8; ENTRY_SIZE]) -> bool {
    u64::from_le_bytes(*entry) == ZERO_EXTENDED_BASE
}

/// Turns a copy queue's semaphore releases into launches, in the guest's own commands. Says whether
/// it could read every entry's commands: an unread one may hold a release a channel here refuses.
fn patch_copy_semaphores(host: &mut dyn HostServices, space: SpaceId, listing: &[u8]) -> bool {
    let (entries, _) = listing.as_chunks::<ENTRY_SIZE>();
    let mut read_them_all = true;
    for entry in entries {
        let entry = RingEntry::parse(*entry);
        if entry.length == 0 {
            continue;
        }
        let mut bytes = vec![0; entry.length as usize * 4];
        if host.read_guest_span(space, entry.address, &mut bytes) != Some(bytes.len()) {
            read_them_all = false;
            continue;
        }
        let (words, _) = bytes.as_chunks::<4>();
        let words: Vec<u32> = words.iter().map(|word| u32::from_le_bytes(*word)).collect();
        let mut index = 0;
        while index < words.len() {
            let header = words[index];
            if header == COPY_SEMAPHORE_HEADER && index + 1 < words.len() {
                if words[index + 1] == COPY_SEMAPHORE_OPERATION_RELEASE {
                    let at = entry.address + index as u64 * 4;
                    host.write_guest_span(space, at, COPY_LAUNCH_HEADER);
                    host.write_guest_span(space, at + 4, COPY_LAUNCH_RELEASE_ONE_WORD);
                } else {
                    log::warn!("a copy semaphore with operation {:#x}", words[index + 1]);
                }
            }
            index += Header::parse(header).stride();
        }
    }
    read_them_all
}

/// Does what a submission's host semaphores would have done, when it did not really run.
///
/// The last one in each submission writes the ring position of its last entry. If that never moves,
/// the ring looks full to the guest's driver, which ends in the device failing.
fn release_host_semaphores(host: &mut dyn HostServices, space: SpaceId, ring: &[u8], entries: u32, end: u32) {
    for index in end.wrapping_sub(entries)..end {
        let entry = entry_at(ring, index);
        if entry.length == 0 {
            continue;
        }
        let Some(words) = read_words(host, space, entry.address, entry.length) else { continue };
        let mut at = 0;
        while at < words.len() {
            let header = Header::parse(words[at]);
            if header.mode == 1
                && header.method == HOST_SEMAPHORE_METHOD
                && header.count >= HOST_SEMAPHORE_WORDS
                && at + 5 < words.len()
            {
                let address = u64::from(words[at + 1]) | u64::from(words[at + 2]) << 32;
                if let Some(span) = host.shared_span(space, address)
                    && let Err(error) = host.write_u32(span.guest_address, words[at + 3])
                {
                    log::warn!("a semaphore can't be released: {error}");
                }
            }
            at += header.stride();
        }
    }
}

impl<R: Rm> NvidiaBackend<R> {
    /// What runs this work's commands, on a list channel.
    fn engine_for(&self, work: &Work<'_>) -> Engine {
        match self.video_engine(work) {
            Some(video) => Engine::Video(video),
            None if self.answers().node_is_graphics(work.node) => Engine::Graphics,
            None => Engine::Copy,
        }
    }

    /// Whether this work's commands are copies, which a channel here needs changed.
    fn runs_copies(&self, work: &Work<'_>) -> bool {
        self.engine_for(work) == Engine::Copy
    }

    fn take_area(&mut self, host: &mut dyn HostServices, size: usize) -> Option<u64> {
        host.take_gpu_addresses(size as u64, crate::CHANNEL_ADDRESS_ALIGN).ok()
    }

    /// Where a new graphics channel's shader local memory goes, and the window it is reached
    /// through. Nothing until a queue has set the window, as no value is made up for it.
    fn local_memory_place(&mut self, host: &mut dyn HostServices, engine: Engine) -> Option<(u64, u64)> {
        if engine != Engine::Graphics {
            return None;
        }
        let Some(window) = self.local_window.window() else {
            log::debug!("a graphics channel waits for the shader local window before it has a pool");
            return None;
        };
        match host.take_gpu_addresses(LOCAL_MEMORY_BYTES, LOCAL_MEMORY_ALIGN) {
            Ok(address) => Some((address, window)),
            Err(error) => {
                log::warn!("no address for shader local memory: {error}");
                None
            }
        }
    }

    /// Reads a graphics submission's commands for the shader local window, until a queue sets it.
    fn watch_local_window(&mut self, host: &dyn HostServices, work: &Work<'_>, listing: &[u8]) {
        if self.local_window.window().is_some() {
            return;
        }
        for words in command_buffers(host, work.space, listing) {
            self.local_window.read(work.queue, &words);
        }
    }

    /// Reads a compute context's graphics submission for a pool its driver sets itself.
    fn watch_guest_pool(&mut self, host: &dyn HostServices, work: &Work<'_>, listing: &[u8]) {
        if !mirrors_pool(work) || self.engine_for(work) != Engine::Graphics {
            return;
        }
        for words in command_buffers(host, work.space, listing) {
            self.pool_mirror.read((work.space, work.client_hint), work.queue, &words);
        }
    }

    /// Points a queue's graphics channel at the pool its context's driver set on another channel,
    /// ahead of this submission.
    fn give_guest_pool(&mut self, work: &Work<'_>) {
        if !mirrors_pool(work) {
            return;
        }
        let Some(queue) = self.queues.get_mut(&work.queue) else { return };
        let channel = match &mut queue.channel {
            QueueChannel::Ring { channel, .. } => channel,
            QueueChannel::List(list) => &mut list.channel,
            QueueChannel::Unusable => return,
        };
        if channel.engine() != Engine::Graphics {
            return;
        }
        if let Some(pool) = self.pool_mirror.owed((work.space, work.client_hint), work.queue) {
            log::debug!("queue {:#x} gets its context's own shader local pool {pool:x?}", work.queue);
            channel.add_late_pool_methods(&pool);
        }
    }

    /// Notes which subchannels a watched queue's commands use, and logs the first use of each.
    fn watch_queue(&mut self, host: &dyn HostServices, work: &Work<'_>, listing: &[u8], kind: &str) {
        let Some(watch) = &mut self.queue_watch else { return };
        let found = SubchannelUse::of_listing(host, work.space, listing);
        let Some((new, submission)) = watch.entry(work.queue).or_default().add(found) else { return };
        let place = self.queue_places.get(&work.queue).map_or("nothing".to_owned(), ToString::to_string);
        log::info!(
            "queue {:#x} of context {:#x} on node {} ({kind}, placed by {place}): submission {submission} \
             is the first with subchannels {new:#04b}{}",
            work.queue,
            work.context,
            work.node,
            if found.unread { ", and has commands that can't be read" } else { "" }
        );
    }

    /// The entries a submission on a guest's ring adds, read from where the channel runs them.
    fn ring_listing(host: &dyn HostServices, space: SpaceId, ring: u64, entries: u32, end: u32) -> Vec<u8> {
        let Some(span) = host.shared_span(space, ring) else { return Vec::new() };
        let Some(size) = ring_entries_of(span.mapping_length) else { return Vec::new() };
        let mut listing = vec![0; entries.min(size) as usize * ENTRY_SIZE];
        for (index, entry) in listing.chunks_mut(ENTRY_SIZE).enumerate() {
            let position = end.wrapping_sub(entries).wrapping_add(index as u32) % size;
            host.read_guest(span.guest_address + u64::from(position) * ENTRY_SIZE as u64, entry);
        }
        listing
    }

    /// A pool for a queue's graphics channel that was made before the window was known. It runs
    /// ahead of the queue's next submission. A queue that named its own pool keeps it.
    fn give_late_local_memory(&mut self, host: &mut dyn HostServices, queue_id: u32) {
        let Some(window) = self.local_window.window() else { return };
        if self.local_window.sets_own_pool(queue_id) {
            return;
        }
        let Some(queue) = self.queues.get_mut(&queue_id) else { return };
        let channel = match &mut queue.channel {
            QueueChannel::Ring { channel, .. } => channel,
            QueueChannel::List(list) => &mut list.channel,
            QueueChannel::Unusable => return,
        };
        if channel.engine() != Engine::Graphics || std::mem::replace(&mut queue.local_memory_tried, true) {
            return;
        }
        let Some(space) = self.spaces.get(&queue.space) else { return };
        let address = match host.take_gpu_addresses(LOCAL_MEMORY_BYTES, LOCAL_MEMORY_ALIGN) {
            Ok(address) => address,
            Err(error) => return log::warn!("no address for shader local memory: {error}"),
        };
        match channel.add_late_local_memory(&mut self.device, space, address, window) {
            Ok(()) => {
                log::debug!("queue {queue_id:#x} gets its shader local memory ahead of this submission")
            }
            Err(error) => log::warn!("no shader local memory: {error}"),
        }
    }

    /// The seat a new ring channel takes in its context's group, where the queue's private data
    /// names one. Only graphics channels of a context the guest named, with the setting on.
    fn seat_for(&self, work: &Work<'_>, engine: Engine) -> Option<Seat> {
        if !self.device.context_groups.enabled || engine != Engine::Graphics || work.context == 0 {
            return None;
        }
        self.queue_places.get(&work.queue)?.seat()
    }

    /// The queue's channel on a guest's ring, made the first time. `first` is where this
    /// submission starts, and the entry before it is free for the prelude.
    fn ring_channel(&mut self, host: &mut dyn HostServices, work: &Work<'_>, ring_address: u64, first: u32) {
        let queue = self.queues.entry(work.queue).or_insert_with(|| LiveQueue::new(work.space));
        if std::mem::replace(&mut queue.channel_tried, true) {
            return;
        }
        let Some(span) =
            host.shared_span(work.space, ring_address).filter(|span| span.offset_in_mapping == 0)
        else {
            log::warn!("ring {ring_address:#x} is not at the start of a shared allocation");
            return;
        };
        if first == 0 {
            log::warn!("no free ring entry for the prelude");
            return;
        }
        let Some(area_address) = self.take_area(host, AREA_SIZE) else { return };
        let Some(ring_entries) = ring_entries_of(span.mapping_length) else {
            log::warn!("ring {ring_address:#x} is too long for a channel");
            return;
        };
        let place = ChannelPlace { ring_address, ring_entries, area_address, area_size: AREA_SIZE };
        let engine = self.video_engine(work).map_or(Engine::Graphics, Engine::Video);
        let local_memory = self.local_memory_place(host, engine);
        let Some(space) = self.spaces.get(&work.space) else { return };
        let seat = self.seat_for(work, engine);
        let (device, generation, sizes) = (&mut self.device, self.generation, self.host_branch);
        let seated = seat.and_then(|seat| {
            match Channel::new_in_context(device, generation, sizes, space, place, work.context, seat) {
                Ok(channel) => {
                    log::info!(
                        "queue {:#x} is channel {} of context {:#x}'s group, on {seat:?}",
                        work.queue,
                        device.context_groups.members(work.context),
                        work.context
                    );
                    Some(channel)
                }
                Err(error) => {
                    log::info!(
                        "queue {:#x} can't sit on {seat:?} in context {:#x}'s group ({error}), so it gets a \
                         group of its own",
                        work.queue,
                        work.context
                    );
                    // The context's queues wait across groups after all, so the shared one gets the
                    // slice the others have.
                    let (everyone, shared) =
                        (device.graphics_timeslice_micros, device.shared_context_timeslice_micros);
                    if !device.context_groups.is_split(work.context) && everyone == 0 {
                        Channel::set_context_group_timeslice(device, work.context, shared);
                    }
                    device.context_groups.note_split(work.context);
                    None
                }
            }
        });
        let made = match seated {
            Some(channel) => Ok(channel),
            None => Channel::new(device, generation, sizes, space, place, engine),
        };
        let (everyone, shared) =
            (self.device.graphics_timeslice_micros, self.device.shared_context_timeslice_micros);
        match made {
            Ok(mut channel) => {
                // A channel in its context's group waits inside the group, on the RM's own slice.
                let split = self.device.context_groups.is_split(work.context);
                let shared = if channel.in_context_group() && !split { 0 } else { shared };
                if let Some(micros) = shared_context_timeslice(engine, work.context_queues, everyone, shared)
                {
                    log::info!(
                        "queue {:#x} shares its context with {} more, so its channel gets a {micros} µs slice",
                        work.queue,
                        work.context_queues - 1
                    );
                    channel.set_timeslice(&mut self.device, micros);
                }
                if let Some((address, window)) = local_memory
                    && let Err(error) = channel.add_local_memory(&mut self.device, space, address, window)
                {
                    log::warn!("no shader local memory: {error}");
                }
                let prelude_at = span.guest_address + u64::from(first - 1) * ENTRY_SIZE as u64;
                if let Err(error) = host.write_u64(prelude_at, channel.prelude_entry()) {
                    // A channel with no prelude would fault, so it never gets to run.
                    log::warn!("the prelude has nowhere to go: {error}");
                    if let Err(error) = channel.close(&mut self.device, space) {
                        log::warn!("closing a channel: {}", memory_error(error));
                    }
                } else if let Some(queue) = self.queues.get_mut(&work.queue) {
                    queue.local_memory_tried = local_memory.is_some();
                    queue.channel = QueueChannel::Ring { channel, ring_address };
                    self.give_sked_page(work.queue);
                }
            }
            Err(error) => log::warn!("a channel failed: {error}"),
        }
    }

    /// Looks at the ring entries between the last submission and this one. They belong to no
    /// submission, but the GPU fetches them anyway, and after the ring wraps they can still hold
    /// entries from the last lap, which would run old commands again.
    fn clear_ring_gap(
        host: &mut dyn HostServices,
        space: SpaceId,
        ring_address: u64,
        size: u32,
        previous: u32,
        start: u32,
        clear: bool,
    ) {
        let Some(span) = host.shared_span(space, ring_address) else { return };
        let gap = (start.wrapping_sub(previous)) % size;
        for index in 0..gap {
            let at = span.guest_address + u64::from(previous.wrapping_add(index) % size) * ENTRY_SIZE as u64;
            let mut entry = [0; ENTRY_SIZE];
            host.read_guest(at, &mut entry);
            if clear
                && entry != [0; ENTRY_SIZE]
                && let Err(error) = host.write_u64(at, 0)
            {
                log::warn!("a ring entry can't be cleared: {error}");
            }
        }
    }

    /// Puts an entry in the last free place ahead of a submission. The guest leaves empty entries
    /// there, and the GPU runs them on its way to the submission, once the doorbell has rung, so this
    /// comes after everything the guest's CPU wrote for it. False if there was no place for it.
    fn invalidate_in_gap(
        host: &mut dyn HostServices,
        space: SpaceId,
        ring_address: u64,
        size: u32,
        previous: u32,
        start: u32,
        entry: u64,
    ) -> bool {
        if start.wrapping_sub(previous).is_multiple_of(size) {
            return false;
        }
        let Some(span) = host.shared_span(space, ring_address) else { return false };
        let last_free = start.wrapping_sub(1) % size;
        let at = span.guest_address + u64::from(last_free) * ENTRY_SIZE as u64;
        host.write_u64(at, entry).is_ok()
    }

    /// The move submission has retired, so the GPU has run the move. Counting switches to the new
    /// ring now, and not before: a doorbell for the new ring is a small index, and storing it while
    /// the GPU is still on the old ring stops it short of the move.
    ///
    /// A move whose fence can't be read, or hasn't retired by [`MOVE_TIMEOUT`], fails the queue.
    fn settle_move(&mut self, host: &dyn HostServices, queue_id: u32, now: Instant) -> Result<(), WorkStart> {
        let Some(queue) = self.queues.get_mut(&queue_id) else { return Ok(()) };
        let Some(pending) = queue.pending_move else { return Ok(()) };
        let fail = |queue: &mut LiveQueue, why: String| {
            log::warn!("queue {queue_id:#x} gives up its move to ring {:#x}: {why}", pending.ring_address);
            queue.pending_move = None;
            Err(WorkStart::QueueFailed)
        };
        let seen = match host.fence_value(pending.fence) {
            Ok(seen) => seen,
            Err(error) => return fail(queue, format!("its fence can't be read: {error}")),
        };
        if seen < pending.reached {
            if now.saturating_duration_since(pending.registered) > MOVE_TIMEOUT {
                return fail(queue, format!("its fence is at {seen:#x} and not {:#x}", pending.reached));
            }
            return Err(WorkStart::Busy);
        }
        queue.pending_move = None;
        let QueueChannel::Ring { channel, ring_address } = &mut queue.channel else { return Ok(()) };
        Self::finish_ring_move(channel, ring_address, &mut queue.position, pending);
        Ok(())
    }

    fn start_on_ring(
        &mut self,
        host: &mut dyn HostServices,
        work: &Work<'_>,
        ring_address: u64,
        entries: u32,
        end: u32,
    ) -> Result<u32, WorkStart> {
        self.settle_move(host, work.queue, Instant::now())?;
        let first = end.saturating_sub(entries);
        if self.queue_watch.is_some() {
            let ring = self.queue_ring(work.queue).unwrap_or(ring_address);
            let listing = Self::ring_listing(host, work.space, ring, entries, end);
            self.watch_queue(host, work, &listing, "ring");
        }
        let mirrored = self.engine_for(work) == Engine::Graphics && mirrors_pool(work);
        if self.video_engine(work).is_none() && (self.local_window.window().is_none() || mirrored) {
            let ring = self.queue_ring(work.queue).unwrap_or(ring_address);
            let listing = Self::ring_listing(host, work.space, ring, entries, end);
            self.watch_local_window(host, work, &listing);
            self.watch_guest_pool(host, work, &listing);
        }
        self.ring_channel(host, work, ring_address, first);
        self.give_late_local_memory(host, work.queue);
        self.give_guest_pool(work);
        if let Some(old_ring) = self.queue_ring(work.queue).filter(|known| *known != ring_address) {
            // This submission still ends on the old ring. The move goes with its doorbell.
            self.begin_ring_move(host, work, old_ring, ring_address, entries, end)?;
        }
        self.keep_to_bound_subchannels(host, work, entries, end)?;
        let channel_class = self.generation.channel_class;
        let queue = self.queues.get_mut(&work.queue).ok_or(WorkStart::Finished)?;
        let QueueChannel::Ring { channel, ring_address: running } = &mut queue.channel else {
            return Err(WorkStart::Finished);
        };
        if work.may_run && has_no_extended_base(channel_class) {
            let size = channel.ring_entries();
            Self::clear_zero_extended_bases(host, work.space, *running, size, entries, end);
        }
        let mut invalidated = true;
        if let Some(previous) = queue.position {
            let start = end.wrapping_sub(entries);
            let size = channel.ring_entries();
            Self::clear_ring_gap(host, work.space, ring_address, size, previous, start, work.may_run);
            if work.may_run {
                let entry = channel.entry_ahead();
                invalidated =
                    Self::invalidate_in_gap(host, work.space, ring_address, size, previous, start, entry);
                if invalidated {
                    channel.entry_ahead_placed();
                }
            }
        }
        queue.position = Some(end);
        if !invalidated {
            self.invalidate_and_wait();
        }
        Ok(end)
    }

    /// A channel on a compute seat has no object for drawing or copying, and commands for one
    /// would fault on the card, taking the context's whole group with them. So a submission with any
    /// stops its queue here. The entries are read from the ring the channel runs now, which during
    /// a move is still the old one.
    fn keep_to_bound_subchannels(
        &self,
        host: &dyn HostServices,
        work: &Work<'_>,
        entries: u32,
        end: u32,
    ) -> Result<(), WorkStart> {
        let Some(LiveQueue { channel: QueueChannel::Ring { channel, ring_address }, .. }) =
            self.queues.get(&work.queue)
        else {
            return Ok(());
        };
        let Some(missing) = channel.missing_objects().filter(|_| work.may_run) else { return Ok(()) };
        let listing = Self::ring_listing(host, work.space, *ring_address, entries, end);
        let found = SubchannelUse::of_listing(host, work.space, &listing);
        if found.needs(missing) {
            log::warn!(
                "queue {:#x} stops: its channel only computes, and its commands use subchannels {:#04b}{}",
                work.queue,
                found.used,
                if found.unread { " and some can't be read" } else { "" }
            );
            return Err(WorkStart::QueueFailed);
        }
        Ok(())
    }

    /// The ring a queue's channel runs, if it has one.
    fn queue_ring(&self, queue: u32) -> Option<u64> {
        match self.queues.get(&queue)?.channel {
            QueueChannel::Ring { ring_address, .. } => Some(ring_address),
            _ => None,
        }
    }

    /// The submission that names a new ring. The guest's driver asked for the ring and got a handle,
    /// which this submission carries, and put a method naming the handle at the end of its commands.
    /// The entries are still on the old ring, so they run as usual, and the GPU makes the move when
    /// it reaches the method. The new ring's first entry holds the commands that followed it.
    fn begin_ring_move(
        &mut self,
        host: &mut dyn HostServices,
        work: &Work<'_>,
        old_ring: u64,
        ring_address: u64,
        entries: u32,
        end: u32,
    ) -> Result<(), WorkStart> {
        let stop = |why: &str| {
            log::warn!("queue {:#x} moved to ring {ring_address:#x} and stops: {why}", work.queue);
            WorkStart::QueueFailed
        };
        let Some(RingMove { handle, entries: ring_entries }) = self.ring_moves.take(ring_address) else {
            return Err(stop("no ring was asked for there"));
        };
        if word(work.private, MOVE_HANDLE) != handle {
            return Err(stop("the submission names another move"));
        }
        let queue = self.queues.get_mut(&work.queue).ok_or(WorkStart::Finished)?;
        let QueueChannel::Ring { channel, .. } = &queue.channel else {
            return Err(WorkStart::Finished);
        };
        if let Err(error) = channel.register_move(&mut self.device, handle, ring_address, ring_entries) {
            return Err(stop(&format!("the move can't be registered: {error}")));
        }
        queue.pending_move = Some(PendingMove {
            ring_address,
            ring_entries,
            fence: work.fence,
            reached: work.fence_value,
            registered: Instant::now(),
        });
        // The doorbell for the new ring can come before the GPU has moved, and the GPU takes a
        // position below its own as the ring wrapping. What it would run past the move is the old
        // ring's stale entries, so those go. What stays is what can still be unfinished: all of this
        // submission's entries, however many it has, and the limit's worth of earlier ones, since
        // the guest holds its next submission back once more than that is unfinished.
        // Holding the new ring's put until the move retires should stop that doorbell. This stays
        // as a second guard in case one gets through early.
        let old_entries = channel.ring_entries();
        let stale = Self::stale_entries(old_entries, entries);
        Self::clear_entries(host, work.space, old_ring, old_entries, end, stale);
        log::info!(
            "queue {:#x} moves to ring {ring_address:#x} of {ring_entries:#x} entries after {entries} entries ending at {end:#x}",
            work.queue
        );
        Ok(())
    }

    /// How many entries of a ring of `ring_entries` are stale when a submission of `entries` entries
    /// ends it. The submission's own entries and the `UNFINISHED_ENTRIES_LIMIT` before them can still
    /// be unfinished, and only what is older than those is stale.
    fn stale_entries(ring_entries: u32, entries: u32) -> u32 {
        ring_entries.saturating_sub(entries).saturating_sub(UNFINISHED_ENTRIES_LIMIT as u32)
    }

    /// Zeroes `count` entries of a ring from `from`, which makes them do nothing.
    fn clear_entries(
        host: &mut dyn HostServices,
        space: SpaceId,
        ring: u64,
        size: u32,
        from: u32,
        count: u32,
    ) {
        let Some(span) = host.shared_span(space, ring) else { return };
        for index in 0..count {
            let at = span.guest_address + u64::from(from.wrapping_add(index) % size) * ENTRY_SIZE as u64;
            if let Err(error) = host.write_u64(at, 0) {
                log::warn!("a stale ring entry can't be cleared: {error}");
                return;
            }
        }
    }

    /// What [`without_zero_extended_bases`] does for a list, for a submission on a ring the GPU
    /// reads from the guest. An entry can't be left out there, so it's zeroed where it lies, before
    /// the doorbell lets the GPU reach it.
    fn clear_zero_extended_bases(
        host: &mut dyn HostServices,
        space: SpaceId,
        ring: u64,
        size: u32,
        entries: u32,
        end: u32,
    ) {
        let Some(span) = host.shared_span(space, ring) else { return };
        for index in 0..entries.min(size) {
            let position = end.wrapping_sub(entries).wrapping_add(index) % size;
            let at = span.guest_address + u64::from(position) * ENTRY_SIZE as u64;
            let mut entry = [0; ENTRY_SIZE];
            if host.read_guest(at, &mut entry) == ENTRY_SIZE
                && sets_zero_extended_base(&entry)
                && let Err(error) = host.write_u64(at, 0)
            {
                log::warn!("a ring entry that sets the extended base can't be cleared: {error}");
            }
        }
    }

    fn list_channel(&mut self, host: &mut dyn HostServices, work: &Work<'_>) -> bool {
        if self.queues.contains_key(&work.queue) {
            return matches!(self.queues[&work.queue].channel, QueueChannel::List(_));
        }
        let mut queue = LiveQueue::new(work.space);
        let engine = self.engine_for(work);
        let local_memory = self.local_memory_place(host, engine);
        queue.local_memory_tried = local_memory.is_some();
        if let (Some(area_address), Some(space)) =
            (self.take_area(host, LIST_AREA), self.spaces.get(&work.space))
        {
            let place = ChannelPlace {
                ring_address: ListChannel::ring_address(area_address),
                ring_entries: ListChannel::RING_ENTRIES,
                area_address,
                area_size: LIST_AREA,
            };
            match Channel::new(&mut self.device, self.generation, self.host_branch, space, place, engine) {
                Ok(mut channel) => {
                    if let Some((address, window)) = local_memory
                        && let Err(error) = channel.add_local_memory(&mut self.device, space, address, window)
                    {
                        log::warn!("no shader local memory: {error}");
                    }
                    queue.channel = QueueChannel::List(ListChannel::new(channel));
                }
                Err(error) => log::warn!("a channel failed: {error}"),
            }
        }
        let usable = matches!(queue.channel, QueueChannel::List(_));
        self.queues.insert(work.queue, queue);
        self.give_sked_page(work.queue);
        usable
    }

    fn start_on_list(
        &mut self,
        host: &mut dyn HostServices,
        work: &Work<'_>,
        listing: &[u8],
    ) -> Result<u32, WorkStart> {
        if self.engine_for(work) == Engine::Graphics {
            self.watch_local_window(host, work, listing);
            self.watch_guest_pool(host, work, listing);
        }
        self.watch_queue(host, work, listing, "list");
        if !self.list_channel(host, work) {
            return Err(WorkStart::Finished);
        }
        self.give_late_local_memory(host, work.queue);
        self.give_guest_pool(work);
        let listing = without_zero_extended_bases(self.generation.channel_class, listing);
        // A fence that can't be read counts as nothing finished, which only holds work back.
        let finished = host.fence_value(work.fence).unwrap_or(0);
        let queue = self.queues.get_mut(&work.queue).ok_or(WorkStart::Finished)?;
        let QueueChannel::List(list) = &mut queue.channel else { return Err(WorkStart::Finished) };
        list.add(&listing, work.fence_gpu_address, work.fence_value, finished).map_err(|full| match full {
            ListFull::ForNow => WorkStart::Busy,
            ListFull::TooLong => {
                log::warn!("queue {:#x} sent a list that can't fit", work.queue);
                WorkStart::QueueFailed
            }
        })
    }

    /// Runs the entries a ring position stands for. Whatever can't be read or trusted stops the queue,
    /// and nothing of it reaches a channel.
    fn start_from_ring_position(
        &mut self,
        host: &mut dyn HostServices,
        work: &Work<'_>,
    ) -> Result<u32, WorkStart> {
        let stop = |why: &str| {
            log::warn!("queue {:#x} stops: {why}", work.queue);
            WorkStart::QueueFailed
        };
        let ring = self.guest_rings.get(&work.queue).ok_or_else(|| stop("it named no ring"))?;
        let (count, end) = (word(work.private, ENTRIES), word(work.private, RING_END));
        let listing = ring.entries_before(host, work.space, count, end).map_err(stop)?;
        if self.runs_copies(work) && !patch_copy_semaphores(host, work.space, &listing) {
            return Err(stop("a copy's commands can't be read, so they can't be made safe"));
        }
        self.start_on_list(host, work, &listing)
    }

    /// The list of a submission, where the driver left it. No channel could ever take a list longer
    /// than its ring, so the entry count is judged before any room is made for them.
    fn listing(
        host: &dyn HostServices,
        work: &Work<'_>,
        previous_end: Option<u64>,
    ) -> Result<Vec<u8>, WorkStart> {
        let private = work.private;
        let offset = word(private, LIST_OFFSET) as usize;
        let (entries, address) = match work.source {
            WorkSource::HardwareQueue => (word(private, ENTRIES), long(private, LIST_ADDRESS)),
            WorkSource::Context => (word(private, CONTEXT_LIST_ENTRIES), 0),
        };
        if entries > ListChannel::RING_ENTRIES {
            log::warn!("queue {:#x} sent a list of {entries} entries", work.queue);
            return Err(WorkStart::QueueFailed);
        }
        let size = entries as usize * ENTRY_SIZE;
        let listing = if address == 0 {
            let end = offset.saturating_add(size).min(private.len());
            private.get(offset.min(end)..end).unwrap_or(&[]).to_vec()
        } else {
            let mut listing = vec![0; size];
            let read =
                host.read_at_gpu_address(work.space, address, &mut listing).ok_or(WorkStart::Finished)?;
            listing.truncate(read);
            listing
        };
        if work.source != WorkSource::HardwareQueue {
            return Ok(listing);
        }
        list_repair::repaired(listing, entries, previous_end).ok_or(WorkStart::QueueFailed)
    }

    /// Stands in for the GPU when a hardware queue's work did not run.
    fn stand_in(host: &mut dyn HostServices, space: SpaceId, private: &[u8]) {
        if private.len() < SMALLEST_PRIVATE_DATA {
            return;
        }
        let mut ring = vec![0; RING_BYTES];
        let Some(read) = host.read_at_gpu_address(space, long(private, RING_ADDRESS), &mut ring) else {
            return;
        };
        ring.truncate(read);
        let entries = word(private, ENTRIES);
        if !ring.is_empty() && entries <= ListChannel::RING_ENTRIES {
            release_host_semaphores(host, space, &ring, entries, word(private, RING_END));
        }
    }

    pub(crate) fn start_work_on_gpu(&mut self, host: &mut dyn HostServices, work: &Work<'_>) -> WorkStart {
        let private = work.private;
        let outcome = self.prepare(host, work);
        let start = match outcome {
            Ok(doorbell) if work.may_run => {
                return WorkStart::Started { doorbell: u64::from(doorbell), work: word(private, ENTRIES) };
            }
            Ok(_) => WorkStart::Finished,
            Err(start) => start,
        };
        if work.source == WorkSource::HardwareQueue && start != WorkStart::Busy {
            Self::stand_in(host, work.space, private);
        }
        start
    }

    fn prepare(&mut self, host: &mut dyn HostServices, work: &Work<'_>) -> Result<u32, WorkStart> {
        let private = work.private;
        if private.len() < SMALLEST_PRIVATE_DATA || work.queue_failed {
            return Err(WorkStart::Finished);
        }
        let ring_address = long(private, RING_ADDRESS);
        if ring_address != 0 {
            return self.start_on_ring(
                host,
                work,
                ring_address,
                word(private, ENTRIES),
                word(private, RING_END),
            );
        }
        if has_ring_position_flag(private) {
            return self.start_from_ring_position(host, work);
        }
        let previous_end = self.queues.get(&work.queue).and_then(|queue| queue.list_end);
        let listing = Self::listing(host, work, previous_end)?;
        if self.runs_copies(work) {
            patch_copy_semaphores(host, work.space, &listing);
        }
        let started = self.start_on_list(host, work, &listing);
        // Work that has to try again keeps its place, as it'll bring the same list back.
        if started.is_ok()
            && let Some(queue) = self.queues.get_mut(&work.queue)
        {
            queue.list_end = list_repair::end_of(&listing);
        }
        started
    }

    pub(crate) fn ring(&mut self, handle: u32, position: u64) {
        let Some(queue) = self.queues.get_mut(&handle) else { return };
        let position = position as u32;
        match &mut queue.channel {
            // The move itself stays pending until its fence retires. Switching here would let the
            // next doorbell store a new-ring index over the put the GPU still has to reach.
            QueueChannel::Ring { channel, .. } => channel.advance(&mut self.device, position),
            QueueChannel::List(list) => list.channel.advance(&mut self.device, position),
            QueueChannel::Unusable => {}
        }
    }

    /// The move submission's fence has retired, so the GPU has run the method and is on the new ring.
    /// GP_GET in the control block never moves, which is why the fence is the signal instead.
    /// From here the channel counts on the new ring, from the entry the driver left at its start.
    fn finish_ring_move(
        channel: &mut Channel,
        ring_address: &mut u64,
        position: &mut Option<u32>,
        pending: PendingMove,
    ) {
        channel.follow_ring(pending.ring_entries);
        *ring_address = pending.ring_address;
        *position = Some(FIRST_POSITION_ON_NEW_RING);
        log::info!(
            "a channel moved to ring {:#x} of {:#x} entries",
            pending.ring_address,
            pending.ring_entries
        );
    }

    /// What a list queue has put on its ring, for tests that play the GPU.
    #[cfg(any(test, feature = "fake-rm"))]
    pub fn ring_of_list_queue(&self, handle: u32) -> Option<Vec<RingEntry>> {
        match &self.queues.get(&handle)?.channel {
            QueueChannel::List(list) => Some(list.entries()),
            _ => None,
        }
    }

    pub(crate) fn close(&mut self, handle: u32) {
        self.guest_rings.remove(&handle);
        self.queue_places.remove(&handle);
        if let Some(watch) = &mut self.queue_watch {
            watch.remove(&handle);
        }
        self.local_window.forget(handle);
        self.pool_mirror.forget(handle);
        let Some(queue) = self.queues.remove(&handle) else { return };
        let Some(space) = self.spaces.get(&queue.space) else { return };
        let page_down = self.sked_pages.queue_closing(&mut self.device, queue.space, space, handle);
        if let Err(error) = queue.channel.close(&mut self.device, space) {
            log::warn!("closing queue {handle:#x}: {}", memory_error(error));
        }
        if page_down {
            let next = self.queues.iter().find(|(_, other)| {
                other.space == queue.space
                    && other.channel.channel().and_then(Channel::compute_object).is_some()
            });
            if let Some((&next, _)) = next {
                self.give_sked_page(next);
            }
        }
    }

    /// Maps the queue's address space's SKED page from the queue's compute object, if the space has
    /// none yet. See [`crate::sked_page`].
    fn give_sked_page(&mut self, handle: u32) {
        let Some(queue) = self.queues.get(&handle) else { return };
        let Some(compute) = queue.channel.channel().and_then(Channel::compute_object) else { return };
        let Some(space) = self.spaces.get(&queue.space) else { return };
        self.sked_pages.give(&mut self.device, queue.space, space, handle, compute);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guesthouse_backend::MemoryError;

    /// Where a ring entry's upper word keeps its length.
    const ONE_WORD_SHIFT: u32 = 10;

    /// Reports every list as present, full of one-word entries, and remembers what was asked.
    #[derive(Default)]
    struct ListHost {
        asked: std::cell::Cell<usize>,
        reads: std::cell::Cell<usize>,
        /// Whether addresses are in shared pages.
        ring: bool,
    }

    impl HostServices for ListHost {
        fn take_gpu_addresses(&mut self, _: u64, _: u64) -> Result<u64, guesthouse_backend::BackendError> {
            Ok(0)
        }
        fn read_at_gpu_address(&self, _: SpaceId, _: u64, out: &mut [u8]) -> Option<usize> {
            self.asked.set(out.len());
            self.reads.set(self.reads.get() + 1);
            let one_word = 1u64 << (u32::BITS + ONE_WORD_SHIFT);
            out.chunks_mut(ENTRY_SIZE)
                .for_each(|entry| entry.copy_from_slice(&one_word.to_le_bytes()[..entry.len()]));
            Some(out.len())
        }
        fn shared_span(&self, _: SpaceId, _: u64) -> Option<guesthouse_backend::SharedSpan> {
            self.ring.then_some(guesthouse_backend::SharedSpan {
                guest_address: 0,
                offset_in_mapping: 0,
                mapping_length: 0,
            })
        }
        fn guest_span(&self, _: SpaceId, _: u64) -> Option<guesthouse_backend::GuestSpan> {
            None
        }
        fn read_guest_span(&self, space: SpaceId, address: u64, out: &mut [u8]) -> Option<usize> {
            self.read_at_gpu_address(space, address, out)
        }
        fn write_guest_span(&mut self, _: SpaceId, _: u64, _: u32) -> bool {
            true
        }
        fn write_u32(&mut self, _: u64, _: u32) -> Result<(), MemoryError> {
            Ok(())
        }
        fn write_u64(&mut self, _: u64, _: u64) -> Result<(), MemoryError> {
            Ok(())
        }
        fn read_guest(&self, _: u64, _: &mut [u8]) -> usize {
            0
        }
        fn fence_value(&self, _: u64) -> Result<u64, MemoryError> {
            Ok(0)
        }
    }

    fn private_data(entries: u32) -> Vec<u8> {
        let mut private = vec![0; SMALLEST_PRIVATE_DATA];
        private[ENTRIES..ENTRIES + 4].copy_from_slice(&entries.to_le_bytes());
        private[LIST_ADDRESS..LIST_ADDRESS + 8].copy_from_slice(&0x1000u64.to_le_bytes());
        private
    }

    fn work(private: &[u8]) -> Work<'_> {
        Work {
            source: WorkSource::HardwareQueue,
            queue: 1,
            space: SpaceId(1),
            node: 1,
            client_hint: 0,
            context: 0,
            context_queues: 1,
            private,
            commands: None,
            fence: 0,
            fence_gpu_address: 0,
            fence_value: 1,
            may_run: true,
            queue_failed: false,
        }
    }

    #[test]
    fn only_a_graphics_channel_whose_context_has_other_queues_gets_the_shared_slice() {
        assert_eq!(shared_context_timeslice(Engine::Graphics, 3, 0, 250), Some(250));
        assert_eq!(shared_context_timeslice(Engine::Graphics, 2, 0, 250), Some(250));
        assert_eq!(shared_context_timeslice(Engine::Graphics, 1, 0, 250), None, "a lone queue");
        assert_eq!(shared_context_timeslice(Engine::Copy, 3, 0, 250), None);
        assert_eq!(shared_context_timeslice(Engine::Graphics, 3, 0, 0), None, "switched off");
        assert_eq!(shared_context_timeslice(Engine::Graphics, 3, 4000, 250), None, "everyone's slice wins");
    }

    #[test]
    fn a_ring_gap_can_start_at_the_last_position_there_is() {
        let mut host = ListHost { ring: true, ..ListHost::default() };
        NvidiaBackend::<crate::rm::fake::FakeRm>::clear_ring_gap(
            &mut host,
            SpaceId(1),
            0,
            8,
            u32::MAX,
            3,
            true,
        );
    }

    #[test]
    fn a_list_as_long_as_the_ring_is_read() {
        let host = ListHost::default();
        let private = private_data(ListChannel::RING_ENTRIES);
        // With no list before it there's nothing to mend it from, but all of it was read first.
        let listing = NvidiaBackend::<crate::rm::fake::FakeRm>::listing(&host, &work(&private), None);
        assert_eq!(listing, Err(WorkStart::QueueFailed));
        assert_eq!(host.asked.get(), ListChannel::RING_ENTRIES as usize * ENTRY_SIZE);
    }

    #[test]
    fn a_longer_list_stops_the_queue_before_any_room_is_made_for_it() {
        let host = ListHost::default();
        let private = private_data(ListChannel::RING_ENTRIES + 1);
        let listing = NvidiaBackend::<crate::rm::fake::FakeRm>::listing(&host, &work(&private), None);
        assert_eq!(listing, Err(WorkStart::QueueFailed));
        assert_eq!(host.asked.get(), 0);
    }

    #[test]
    fn a_submission_longer_than_any_ring_is_not_walked_when_standing_in() {
        let mut host = ListHost::default();
        let entries = ListChannel::RING_ENTRIES + 1;
        let mut private = private_data(entries);
        private[RING_END..RING_END + 4].copy_from_slice(&entries.to_le_bytes());
        NvidiaBackend::<crate::rm::fake::FakeRm>::stand_in(&mut host, SpaceId(1), &private);
        assert_eq!(host.reads.get(), 1, "the ring itself, and no entry in it");
    }

    #[test]
    fn a_ring_is_the_largest_power_of_two_that_fits() {
        assert_eq!(ring_entries_of(0x10000), Some(0x2000));
        assert_eq!(ring_entries_of(0x10008), Some(0x2000));
        assert_eq!(ring_entries_of(0xFFF8), Some(0x1000));
    }

    #[test]
    fn a_ring_a_channel_could_not_count_is_not_used() {
        assert_eq!(ring_entries_of(1 << 34), Some(1 << 31));
        assert_eq!(ring_entries_of(1 << 35), None);
    }

    mod extended_base {
        use super::*;
        use crate::fake_guest::{fake_backend, start_on_card};
        use crate::profile::{AMPERE, BLACKWELL, TURING};
        use crate::pushbuffer::ring_entry;
        use crate::rm::fake::FakeRm;
        use crate::rm::fake_card::FAKE_AMPERE;
        use guesthouse_backend::{Backend, GpuRange, SharedSpan};

        const COMMANDS: u64 = 0x82_1F23_0000;
        const GPU_RANGE: GpuRange = GpuRange { start: 0x1_0000_0000, end: 0x2_0000_0000 };
        const RING: u64 = 0x1_0010_0000;
        const RING_ENTRIES: u32 = 8;
        const CHANNEL_AREA: u64 = 0x1_0040_0000;
        /// The pretend cards' graphics node, whose queues run on the guest's ring.
        const GRAPHICS_NODE: u32 = 0;
        /// The same opcode with a base that isn't zero.
        const ANOTHER_EXTENDED_BASE: u64 = ZERO_EXTENDED_BASE | 1 << 8;

        fn listing(entries: &[u64]) -> Vec<u8> {
            entries.iter().flat_map(|entry| entry.to_le_bytes()).collect()
        }

        #[test]
        fn a_zero_extended_base_is_left_out_of_a_list_for_a_channel_that_has_none() {
            let sent = listing(&[ZERO_EXTENDED_BASE, ring_entry(COMMANDS, 27)]);
            for generation in [&AMPERE, &TURING] {
                let kept = without_zero_extended_bases(generation.channel_class, &sent);
                assert_eq!(kept, listing(&[ring_entry(COMMANDS, 27)]), "{}", generation.name);
            }
        }

        #[test]
        fn a_channel_that_has_the_extended_base_gets_the_list_as_it_came() {
            let sent = listing(&[ZERO_EXTENDED_BASE, ring_entry(COMMANDS, 27)]);
            let kept = without_zero_extended_bases(BLACKWELL.channel_class, &sent);
            assert!(matches!(kept, Cow::Borrowed(_)), "and nothing was copied");
            assert_eq!(kept, sent);
        }

        #[test]
        fn a_list_with_no_extended_base_is_not_copied() {
            let sent = listing(&[ring_entry(COMMANDS, 27), ring_entry(COMMANDS + 0x100, 4)]);
            let kept = without_zero_extended_bases(AMPERE.channel_class, &sent);
            assert!(matches!(kept, Cow::Borrowed(_)));
        }

        #[test]
        fn any_other_entry_without_commands_stays_in_a_list() {
            let odd = [ANOTHER_EXTENDED_BASE, ZERO_EXTENDED_BASE | 1 << 63, 0, 5 << 32];
            let sent = listing(&[&odd[..], &[ZERO_EXTENDED_BASE, ring_entry(COMMANDS, 27)]].concat());
            let kept = without_zero_extended_bases(AMPERE.channel_class, &sent);
            assert_eq!(kept, listing(&[&odd[..], &[ring_entry(COMMANDS, 27)]].concat()));
        }

        #[test]
        fn the_end_of_a_list_that_is_not_a_whole_entry_stays() {
            let mut sent = listing(&[ZERO_EXTENDED_BASE, ring_entry(COMMANDS, 27)]);
            sent.extend_from_slice(&[7, 7, 7]);
            let kept = without_zero_extended_bases(AMPERE.channel_class, &sent);
            assert_eq!(kept[..ENTRY_SIZE], ring_entry(COMMANDS, 27).to_le_bytes());
            assert_eq!(kept[ENTRY_SIZE..], [7, 7, 7]);
        }

        /// Plays a guest whose ring is in shared pages at its own address.
        struct RingHost {
            ring: Vec<u64>,
        }

        impl RingHost {
            fn index(address: u64) -> usize {
                ((address - RING) / ENTRY_SIZE as u64) as usize
            }
        }

        impl HostServices for RingHost {
            fn take_gpu_addresses(
                &mut self,
                _: u64,
                _: u64,
            ) -> Result<u64, guesthouse_backend::BackendError> {
                Ok(CHANNEL_AREA)
            }
            fn read_at_gpu_address(&self, _: SpaceId, _: u64, _: &mut [u8]) -> Option<usize> {
                None
            }
            fn shared_span(&self, _: SpaceId, address: u64) -> Option<SharedSpan> {
                let mapping_length = self.ring.len() as u64 * ENTRY_SIZE as u64;
                (address == RING).then_some(SharedSpan {
                    guest_address: RING,
                    offset_in_mapping: 0,
                    mapping_length,
                })
            }
            fn guest_span(&self, _: SpaceId, _: u64) -> Option<guesthouse_backend::GuestSpan> {
                None
            }
            fn read_guest_span(&self, _: SpaceId, _: u64, _: &mut [u8]) -> Option<usize> {
                None
            }
            fn write_guest_span(&mut self, _: SpaceId, _: u64, _: u32) -> bool {
                true
            }
            fn write_u32(&mut self, _: u64, _: u32) -> Result<(), MemoryError> {
                Ok(())
            }
            fn write_u64(&mut self, address: u64, value: u64) -> Result<(), MemoryError> {
                self.ring[Self::index(address)] = value;
                Ok(())
            }
            fn read_guest(&self, address: u64, out: &mut [u8]) -> usize {
                out.copy_from_slice(&self.ring[Self::index(address)].to_le_bytes());
                out.len()
            }
            fn fence_value(&self, _: u64) -> Result<u64, MemoryError> {
                Ok(0)
            }
        }

        fn cleared(ring: [u64; RING_ENTRIES as usize], entries: u32, end: u32) -> Vec<u64> {
            let mut host = RingHost { ring: ring.to_vec() };
            NvidiaBackend::<FakeRm>::clear_zero_extended_bases(
                &mut host,
                SpaceId(1),
                RING,
                RING_ENTRIES,
                entries,
                end,
            );
            host.ring
        }

        #[test]
        fn a_zero_extended_base_on_a_guests_ring_is_zeroed_where_it_lies() {
            let work = ring_entry(COMMANDS, 27);
            let ring = [0, 0, ZERO_EXTENDED_BASE, work, 0, 0, 0, 0];
            assert_eq!(cleared(ring, 2, 4), [0, 0, 0, work, 0, 0, 0, 0]);
        }

        #[test]
        fn only_this_submissions_entries_on_a_ring_are_looked_at() {
            let work = ring_entry(COMMANDS, 27);
            let ring = [0, ZERO_EXTENDED_BASE, work, work, ZERO_EXTENDED_BASE, 0, 0, 0];
            assert_eq!(cleared(ring, 2, 4), ring);
        }

        #[test]
        fn a_submission_that_wraps_the_ring_is_cleared_on_both_sides() {
            let work = ring_entry(COMMANDS, 27);
            let ring = [ZERO_EXTENDED_BASE, work, 0, 0, 0, 0, work, ZERO_EXTENDED_BASE];
            assert_eq!(cleared(ring, 4, 2), [0, work, 0, 0, 0, 0, work, 0]);
        }

        #[test]
        fn another_base_stays_on_a_ring_for_the_channel_to_refuse() {
            let ring = [ANOTHER_EXTENDED_BASE, ring_entry(COMMANDS, 27), 0, 0, 0, 0, 0, 0];
            assert_eq!(cleared(ring, 2, 2), ring);
        }

        /// The guest's ring after a first submission of a zero extended base and one entry of
        /// commands, at positions 1 and 2. The prelude takes the place before them.
        fn ring_after_a_submission(mut backend: NvidiaBackend<FakeRm>) -> Vec<u64> {
            let ring = vec![0, ZERO_EXTENDED_BASE, ring_entry(COMMANDS, 27), 0, 0, 0, 0, 0];
            let mut host = RingHost { ring };
            let space = backend.create_address_space(&[GPU_RANGE]).unwrap();
            let mut private = vec![0; SMALLEST_PRIVATE_DATA];
            private[ENTRIES..ENTRIES + 4].copy_from_slice(&2u32.to_le_bytes());
            private[RING_END..RING_END + 4].copy_from_slice(&3u32.to_le_bytes());
            private[RING_ADDRESS..RING_ADDRESS + 8].copy_from_slice(&RING.to_le_bytes());
            let started =
                backend.start_work(&mut host, &Work { space, node: GRAPHICS_NODE, ..work(&private) });
            assert_eq!(started, WorkStart::Started { doorbell: 3, work: 2 });
            host.ring
        }

        #[test]
        fn a_submission_on_a_ring_has_lost_its_zero_extended_base_on_ampere_when_it_starts() {
            let backend = start_on_card(FakeRm::new(), &FAKE_AMPERE, FAKE_AMPERE.device as u16);
            let ring = ring_after_a_submission(backend.map_err(|refusal| refusal.0).unwrap());
            assert_eq!(ring[1..3], [0, ring_entry(COMMANDS, 27)]);
        }

        #[test]
        fn a_submission_on_a_ring_keeps_its_extended_base_on_blackwell() {
            let ring = ring_after_a_submission(fake_backend(FakeRm::new()));
            assert_eq!(ring[1..3], [ZERO_EXTENDED_BASE, ring_entry(COMMANDS, 27)]);
        }
    }

    mod moves {
        use super::*;
        use crate::fake_guest::fake_backend;
        use crate::rm::classes::CONTROL_DEFERRED_API;
        use crate::rm::fake::{Call, FakeRm};
        use guesthouse_backend::{Backend, GpuRange, MemoryId, SharedSpan};
        use std::cell::Cell;

        const QUEUE: u32 = 1;
        /// The pretend card's graphics node, whose channels can move.
        const GRAPHICS_NODE: u32 = 0;
        const GPU_RANGE: GpuRange = GpuRange { start: 0x1_0000_0000, end: 0x2_0000_0000 };
        const OLD_RING: u64 = 0x1_0010_0000;
        const OLD_RING_BYTES: u64 = 0x1000;
        const NEW_RING: u64 = 0x1_0020_0000;
        const NEW_RING_BYTES: u64 = 0x8_0000;
        const NEW_RING_ENTRIES: u32 = (NEW_RING_BYTES / ENTRY_SIZE as u64) as u32;
        const CHANNEL_AREA: u64 = 0x1_0040_0000;
        const MOVE: u32 = 0xC1D0_4242;
        const FENCE: u64 = 0x9000;
        /// The fence value the move submission signals.
        const MOVED: u64 = 2;
        /// Long enough for the guest's driver's move submission, with its move handle.
        const MOVE_PRIVATE_DATA: usize = 0x118;

        /// Plays a guest whose rings are in shared pages at their own addresses, and keeps every
        /// 64-bit write.
        struct MoveHost {
            fence: Cell<Result<u64, MemoryError>>,
            writes: Vec<(u64, u64)>,
        }

        impl MoveHost {
            fn new() -> Self {
                Self { fence: Cell::new(Ok(0)), writes: Vec::new() }
            }
        }

        impl HostServices for MoveHost {
            fn take_gpu_addresses(
                &mut self,
                _: u64,
                _: u64,
            ) -> Result<u64, guesthouse_backend::BackendError> {
                Ok(CHANNEL_AREA)
            }
            fn read_at_gpu_address(&self, _: SpaceId, _: u64, _: &mut [u8]) -> Option<usize> {
                None
            }
            fn shared_span(&self, _: SpaceId, address: u64) -> Option<SharedSpan> {
                let length = match address {
                    OLD_RING => OLD_RING_BYTES,
                    NEW_RING => NEW_RING_BYTES,
                    _ => return None,
                };
                Some(SharedSpan { guest_address: address, offset_in_mapping: 0, mapping_length: length })
            }
            fn guest_span(&self, _: SpaceId, _: u64) -> Option<guesthouse_backend::GuestSpan> {
                None
            }
            fn read_guest_span(&self, _: SpaceId, _: u64, _: &mut [u8]) -> Option<usize> {
                None
            }
            fn write_guest_span(&mut self, _: SpaceId, _: u64, _: u32) -> bool {
                true
            }
            fn write_u32(&mut self, _: u64, _: u32) -> Result<(), MemoryError> {
                Ok(())
            }
            fn write_u64(&mut self, address: u64, value: u64) -> Result<(), MemoryError> {
                self.writes.push((address, value));
                Ok(())
            }
            fn read_guest(&self, _: u64, _: &mut [u8]) -> usize {
                0
            }
            fn fence_value(&self, _: u64) -> Result<u64, MemoryError> {
                let value = self.fence.replace(Ok(0));
                self.fence.set(value);
                value
            }
        }

        fn submission(ring: u64, entries: u32, end: u32, handle: u32) -> Vec<u8> {
            let mut private = vec![0; MOVE_PRIVATE_DATA];
            private[ENTRIES..ENTRIES + 4].copy_from_slice(&entries.to_le_bytes());
            private[RING_END..RING_END + 4].copy_from_slice(&end.to_le_bytes());
            private[RING_ADDRESS..RING_ADDRESS + 8].copy_from_slice(&ring.to_le_bytes());
            private[MOVE_HANDLE..MOVE_HANDLE + 4].copy_from_slice(&handle.to_le_bytes());
            private
        }

        fn ring_work(space: SpaceId, private: &[u8], fence_value: u64) -> Work<'_> {
            Work { space, queue: QUEUE, node: GRAPHICS_NODE, fence: FENCE, fence_value, ..work(private) }
        }

        /// A backend with a queue running on the old ring up to entry 3, and the new ring asked for.
        fn on_old_ring(host: &mut MoveHost) -> (NvidiaBackend<FakeRm>, SpaceId) {
            let mut backend = fake_backend(FakeRm::new());
            let space = backend.create_address_space(&[GPU_RANGE]).unwrap();
            let first = submission(OLD_RING, 2, 3, 0);
            let started = backend.start_work(host, &ring_work(space, &first, 1));
            assert_eq!(started, WorkStart::Started { doorbell: 3, work: 2 });
            backend.ring_moves.answered(MemoryId(0x77), MOVE, NEW_RING_BYTES);
            backend.ring_moves.mapped(MemoryId(0x77), NEW_RING);
            (backend, space)
        }

        /// Adds the submission that names the new ring and still ends on the old one, at 6.
        fn moving(host: &mut MoveHost) -> (NvidiaBackend<FakeRm>, SpaceId) {
            let (mut backend, space) = on_old_ring(host);
            let private = submission(NEW_RING, 3, 6, MOVE);
            let started = backend.start_work(host, &ring_work(space, &private, MOVED));
            assert_eq!(started, WorkStart::Started { doorbell: 6, work: 3 });
            backend.ring(QUEUE, 6);
            (backend, space)
        }

        fn queue(backend: &NvidiaBackend<FakeRm>) -> &LiveQueue {
            &backend.queues[&QUEUE]
        }

        #[test]
        fn the_submission_naming_a_new_ring_registers_the_move_and_runs_on_the_old_one() {
            let mut host = MoveHost::new();
            let (backend, _) = moving(&mut host);
            let registered = backend.device.rm.calls.iter().any(|call| {
                matches!(call, Call::Control { command, params, .. }
                    if *command == CONTROL_DEFERRED_API && word(params, 0) == MOVE)
            });
            assert!(registered, "the move went to the deferred API object");
            let pending = queue(&backend).pending_move.expect("a move is pending");
            assert_eq!((pending.ring_address, pending.ring_entries), (NEW_RING, NEW_RING_ENTRIES));
            assert_eq!((pending.fence, pending.reached), (FENCE, MOVED));
            assert_eq!(backend.queue_ring(QUEUE), Some(OLD_RING));
            assert_eq!(queue(&backend).position, Some(6));
        }

        #[test]
        fn later_work_waits_for_the_move_and_stores_no_doorbell() {
            let mut host = MoveHost::new();
            let (mut backend, space) = moving(&mut host);
            host.fence.set(Ok(MOVED - 1));
            let doorbells = backend.device.rm.doorbells().len();
            let next = submission(NEW_RING, 2, 3, 0);
            let started = backend.start_work(&mut host, &ring_work(space, &next, MOVED + 1));
            assert_eq!(started, WorkStart::Busy);
            assert_eq!(backend.device.rm.doorbells().len(), doorbells);
            assert_eq!(backend.queue_ring(QUEUE), Some(OLD_RING));
            assert_eq!(queue(&backend).position, Some(6));
            assert!(queue(&backend).pending_move.is_some());
        }

        #[test]
        fn once_the_move_retires_the_queue_counts_on_the_new_ring_from_one() {
            let mut host = MoveHost::new();
            let (mut backend, _) = moving(&mut host);
            host.fence.set(Ok(MOVED));
            assert_eq!(backend.settle_move(&host, QUEUE, Instant::now()), Ok(()));
            assert_eq!(queue(&backend).position, Some(FIRST_POSITION_ON_NEW_RING));
            assert_eq!(backend.queue_ring(QUEUE), Some(NEW_RING));
            let QueueChannel::Ring { channel, .. } = &queue(&backend).channel else {
                panic!("a ring channel")
            };
            assert_eq!(channel.ring_entries(), NEW_RING_ENTRIES);
            assert_eq!(queue(&backend).pending_move, None);
        }

        #[test]
        fn the_next_submission_after_the_move_starts_on_the_new_ring() {
            let mut host = MoveHost::new();
            let (mut backend, space) = moving(&mut host);
            host.fence.set(Ok(MOVED));
            let next = submission(NEW_RING, 2, 3, 0);
            let started = backend.start_work(&mut host, &ring_work(space, &next, MOVED + 1));
            assert_eq!(started, WorkStart::Started { doorbell: 3, work: 2 });
            assert_eq!(queue(&backend).position, Some(3));
            assert_eq!(backend.queue_ring(QUEUE), Some(NEW_RING));
        }

        #[test]
        fn a_move_that_never_retires_fails_the_queue_after_its_time() {
            let mut host = MoveHost::new();
            let (mut backend, _) = moving(&mut host);
            let soon = Instant::now() + MOVE_TIMEOUT / 2;
            assert_eq!(backend.settle_move(&host, QUEUE, soon), Err(WorkStart::Busy));
            let late = Instant::now() + MOVE_TIMEOUT + Duration::from_secs(1);
            assert_eq!(backend.settle_move(&host, QUEUE, late), Err(WorkStart::QueueFailed));
            assert_eq!(queue(&backend).pending_move, None);
        }

        #[test]
        fn a_fence_that_cant_be_read_fails_the_move_at_once() {
            let mut host = MoveHost::new();
            let (mut backend, _) = moving(&mut host);
            host.fence.set(Err(MemoryError::Outside(FENCE)));
            assert_eq!(backend.settle_move(&host, QUEUE, Instant::now()), Err(WorkStart::QueueFailed));
        }

        #[test]
        fn a_move_naming_another_handle_fails_the_queue() {
            let mut host = MoveHost::new();
            let (mut backend, space) = on_old_ring(&mut host);
            let private = submission(NEW_RING, 3, 6, MOVE + 1);
            let started = backend.start_work(&mut host, &ring_work(space, &private, MOVED));
            assert_eq!(started, WorkStart::QueueFailed);
            assert_eq!(queue(&backend).pending_move, None);
        }

        #[test]
        fn a_move_to_a_ring_nobody_asked_for_fails_the_queue() {
            let mut host = MoveHost::new();
            let mut backend = fake_backend(FakeRm::new());
            let space = backend.create_address_space(&[GPU_RANGE]).unwrap();
            let first = submission(OLD_RING, 2, 3, 0);
            backend.start_work(&mut host, &ring_work(space, &first, 1));
            let private = submission(NEW_RING, 3, 6, MOVE);
            let started = backend.start_work(&mut host, &ring_work(space, &private, MOVED));
            assert_eq!(started, WorkStart::QueueFailed);
        }

        #[test]
        fn a_short_submission_leaves_the_rest_of_the_ring_stale() {
            let kept = |entries: u32| 8192 - entries - UNFINISHED_ENTRIES_LIMIT as u32;
            assert_eq!(NvidiaBackend::<FakeRm>::stale_entries(8192, 3), kept(3));
            assert_eq!(NvidiaBackend::<FakeRm>::stale_entries(8192, 100), kept(100));
        }

        #[test]
        fn a_submission_longer_than_what_is_left_leaves_nothing_stale() {
            assert_eq!(NvidiaBackend::<FakeRm>::stale_entries(8192, 8043), 0);
            assert_eq!(NvidiaBackend::<FakeRm>::stale_entries(8192, 4192), 0);
            assert_eq!(NvidiaBackend::<FakeRm>::stale_entries(8192, 4191), 1);
        }

        #[test]
        fn the_stale_count_never_underflows() {
            assert_eq!(NvidiaBackend::<FakeRm>::stale_entries(8192, 9000), 0);
            assert_eq!(NvidiaBackend::<FakeRm>::stale_entries(512, 3), 0);
            assert_eq!(NvidiaBackend::<FakeRm>::stale_entries(0, 0), 0);
        }

        #[test]
        fn clearing_entries_wraps_at_the_end_of_the_ring() {
            let mut host = MoveHost::new();
            NvidiaBackend::<FakeRm>::clear_entries(&mut host, SpaceId(1), OLD_RING, 8, 6, 4);
            let at = |index: u64| OLD_RING + index * ENTRY_SIZE as u64;
            assert_eq!(host.writes, [(at(6), 0), (at(7), 0), (at(0), 0), (at(1), 0)]);
        }
    }

    mod context_groups {
        use super::*;
        use crate::fake_guest::fake_backend;
        use crate::pushbuffer::{increasing, ring_entry};
        use crate::rm::classes::{CHANNEL_GROUP, CONTROL_SET_TIMESLICE};
        use crate::rm::fake::{Call, FakeRm};
        use guesthouse_backend::{Backend, GpuRange, SharedSpan};
        use std::collections::HashMap;

        const GPU_RANGE: GpuRange = GpuRange { start: 0x1_0000_0000, end: 0x2_0000_0000 };
        const GRAPHICS_NODE: u32 = 0;
        const RINGS: u64 = 0x1_0010_0000;
        const RING_BYTES: u64 = 0x40;
        const AREAS: u64 = 0x1_0040_0000;
        const COMMANDS: u64 = 0x1_0080_0000;
        const CONTEXT: u32 = 0x4000_0100;
        const DRAWS: u32 = 0x100;
        const COMPUTES: u32 = 0x601;

        /// Plays a guest with a ring for each queue, in shared pages, and commands it can read.
        #[derive(Default)]
        struct Guest {
            rings: HashMap<u64, [u64; 8]>,
            commands: HashMap<u64, Vec<u32>>,
            areas: u64,
        }

        impl HostServices for Guest {
            fn take_gpu_addresses(
                &mut self,
                _: u64,
                _: u64,
            ) -> Result<u64, guesthouse_backend::BackendError> {
                self.areas += 1;
                Ok(AREAS + self.areas * 0x100_0000)
            }
            fn read_at_gpu_address(&self, _: SpaceId, _: u64, _: &mut [u8]) -> Option<usize> {
                None
            }
            fn shared_span(&self, _: SpaceId, address: u64) -> Option<SharedSpan> {
                self.rings.contains_key(&address).then_some(SharedSpan {
                    guest_address: address,
                    offset_in_mapping: 0,
                    mapping_length: RING_BYTES,
                })
            }
            fn guest_span(&self, _: SpaceId, _: u64) -> Option<guesthouse_backend::GuestSpan> {
                None
            }
            fn read_guest_span(&self, _: SpaceId, address: u64, out: &mut [u8]) -> Option<usize> {
                let words = self.commands.get(&address)?;
                let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
                let read = out.len().min(bytes.len());
                out[..read].copy_from_slice(&bytes[..read]);
                Some(read)
            }
            fn write_guest_span(&mut self, _: SpaceId, _: u64, _: u32) -> bool {
                true
            }
            fn write_u32(&mut self, _: u64, _: u32) -> Result<(), MemoryError> {
                Ok(())
            }
            fn write_u64(&mut self, address: u64, value: u64) -> Result<(), MemoryError> {
                let ring = address & !(RING_BYTES - 1);
                let entries = self.rings.get_mut(&ring).ok_or(MemoryError::Outside(address))?;
                entries[((address - ring) / ENTRY_SIZE as u64) as usize] = value;
                Ok(())
            }
            fn read_guest(&self, address: u64, out: &mut [u8]) -> usize {
                let ring = address & !(RING_BYTES - 1);
                let Some(entries) = self.rings.get(&ring) else { return 0 };
                out.copy_from_slice(&entries[((address - ring) / ENTRY_SIZE as u64) as usize].to_le_bytes());
                out.len()
            }
            fn fence_value(&self, _: u64) -> Result<u64, MemoryError> {
                Ok(0)
            }
        }

        struct Lab {
            backend: NvidiaBackend<FakeRm>,
            guest: Guest,
            space: SpaceId,
        }

        impl Lab {
            fn new(grouped: bool) -> Self {
                let mut backend = fake_backend(FakeRm::new());
                backend.set_context_groups(grouped);
                backend.set_shared_context_timeslice(250);
                let space = backend.create_address_space(&[GPU_RANGE]).unwrap();
                Self { backend, guest: Guest::default(), space }
            }

            /// A hardware queue whose private data carries this placing word.
            fn queue(&mut self, queue: u32, place: Option<u32>) {
                let mut data = vec![0; 0xB4];
                if let Some(place) = place {
                    data[..4].copy_from_slice(&0x4E56_4441u32.to_le_bytes());
                    data[6] = 1;
                    data[8] = 0xB4;
                    data[0xC..0x10].copy_from_slice(&0x4E56_4458u32.to_le_bytes());
                    data[0x10..0x14].copy_from_slice(&place.to_le_bytes());
                }
                self.backend.finish_queue_data(queue, &mut data, queue).unwrap();
            }

            /// The queue's next submission: one entry running `words`, at the ring's next place.
            fn submit(&mut self, queue: u32, context: u32, context_queues: u32, words: &[u32]) -> WorkStart {
                let ring = RINGS + u64::from(queue) * 0x1000;
                let commands = COMMANDS + u64::from(queue) * 0x1000;
                self.guest.commands.insert(commands, words.to_vec());
                let entries = self.guest.rings.entry(ring).or_default();
                let at = entries.iter().skip(1).position(|entry| *entry == 0).unwrap() + 1;
                entries[at] = ring_entry(commands, words.len() as u32);
                let mut private = vec![0; SMALLEST_PRIVATE_DATA];
                private[ENTRIES..ENTRIES + 4].copy_from_slice(&1u32.to_le_bytes());
                private[RING_END..RING_END + 4].copy_from_slice(&(at as u32 + 1).to_le_bytes());
                private[RING_ADDRESS..RING_ADDRESS + 8].copy_from_slice(&ring.to_le_bytes());
                let work = Work {
                    queue,
                    space: self.space,
                    node: GRAPHICS_NODE,
                    context,
                    context_queues,
                    ..work(&private)
                };
                self.backend.start_work(&mut self.guest, &work)
            }

            fn channel(&self, queue: u32) -> &Channel {
                self.backend.queues[&queue].channel.channel().expect("the queue has a channel")
            }

            fn groups_made(&self) -> usize {
                let calls = self.backend.device.rm.calls.iter();
                calls.filter(|call| matches!(call, Call::Alloc { class: CHANNEL_GROUP, .. })).count()
            }

            fn slices(&self) -> usize {
                let calls = self.backend.device.rm.calls.iter();
                calls
                    .filter(|call| matches!(call, Call::Control { command: CONTROL_SET_TIMESLICE, .. }))
                    .count()
            }
        }

        fn compute_work() -> [u32; 2] {
            [increasing(1, 0x2B4, 1), 0]
        }

        fn drawing_work() -> [u32; 2] {
            [increasing(0, 0x200, 1), 0]
        }

        fn copy_work() -> [u32; 2] {
            [increasing(4, 0x300, 1), 0]
        }

        #[test]
        fn the_queues_of_a_context_share_one_group_on_the_seats_their_data_names() {
            let mut lab = Lab::new(true);
            let before = lab.groups_made();
            lab.queue(1, Some(DRAWS));
            lab.queue(2, Some(COMPUTES));
            lab.queue(3, Some(COMPUTES + 1));
            assert!(matches!(lab.submit(1, CONTEXT, 3, &drawing_work()), WorkStart::Started { .. }));
            assert!(matches!(lab.submit(2, CONTEXT, 3, &compute_work()), WorkStart::Started { .. }));
            assert!(matches!(lab.submit(3, CONTEXT, 3, &compute_work()), WorkStart::Started { .. }));
            assert_eq!(lab.groups_made() - before, 1);
            assert_eq!(lab.backend.device.context_groups.members(CONTEXT), 3);
            assert_eq!(lab.channel(1).missing_objects(), None);
            assert!(lab.channel(2).missing_objects().is_some() && lab.channel(3).missing_objects().is_some());
            assert_eq!(lab.slices(), 0, "they wait inside the group, on the RM's own slice");
        }

        #[test]
        fn another_contexts_queue_gets_another_group() {
            let mut lab = Lab::new(true);
            let before = lab.groups_made();
            lab.queue(1, Some(DRAWS));
            lab.queue(2, Some(DRAWS));
            lab.submit(1, CONTEXT, 1, &drawing_work());
            lab.submit(2, CONTEXT + 0x40, 1, &drawing_work());
            assert_eq!(lab.groups_made() - before, 2);
            assert_eq!(lab.backend.device.context_groups.len(), 2);
        }

        #[test]
        fn with_the_setting_off_every_queue_has_a_group_of_its_own_and_the_shared_slice() {
            let mut lab = Lab::new(false);
            let before = lab.groups_made();
            lab.queue(1, Some(DRAWS));
            lab.queue(2, Some(COMPUTES));
            assert!(matches!(lab.submit(1, CONTEXT, 2, &drawing_work()), WorkStart::Started { .. }));
            assert!(matches!(lab.submit(2, CONTEXT, 2, &compute_work()), WorkStart::Started { .. }));
            assert_eq!(lab.groups_made() - before, 2);
            assert!(lab.backend.device.context_groups.is_empty());
            assert!(!lab.channel(1).in_context_group() && lab.channel(2).missing_objects().is_none());
            assert_eq!(lab.slices(), 2);
        }

        #[test]
        fn a_queue_whose_data_names_no_seat_keeps_a_group_of_its_own() {
            let mut lab = Lab::new(true);
            lab.queue(1, Some(DRAWS));
            lab.queue(2, None);
            lab.queue(3, Some(0x200));
            lab.queue(4, Some(0x101));
            for queue in 1..=4 {
                let started = lab.submit(queue, CONTEXT, 4, &compute_work());
                assert!(matches!(started, WorkStart::Started { .. }), "queue {queue}");
            }
            assert_eq!(lab.backend.device.context_groups.members(CONTEXT), 1);
            assert!((2..=4).all(|queue| !lab.channel(queue).in_context_group()));
            assert_eq!(lab.slices(), 3, "the ones alone still get the shared slice");
        }

        #[test]
        fn a_queue_with_no_context_is_not_seated() {
            let mut lab = Lab::new(true);
            lab.queue(1, Some(DRAWS));
            lab.submit(1, 0, 1, &drawing_work());
            assert!(lab.backend.device.context_groups.is_empty());
        }

        #[test]
        fn a_seat_the_rm_refuses_falls_back_to_a_group_of_its_own() {
            let mut lab = Lab::new(true);
            lab.queue(1, Some(DRAWS));
            lab.queue(2, Some(COMPUTES));
            lab.submit(1, CONTEXT, 2, &drawing_work());
            let before = lab.groups_made();
            // As GSP on a branch that takes no compute beside a group's drawing: the class is
            // refused on the seat, once.
            lab.backend.device.rm.refuse_class_once = Some(lab.backend.generation.compute_class);
            assert!(matches!(lab.submit(2, CONTEXT, 2, &compute_work()), WorkStart::Started { .. }));
            assert_eq!(lab.groups_made() - before, 1, "a group of its own");
            assert_eq!(lab.backend.device.context_groups.members(CONTEXT), 1);
            assert!(!lab.channel(2).in_context_group());
            assert_eq!(lab.channel(2).missing_objects(), None, "with every class, as before");
            assert_eq!(lab.slices(), 2, "and the shared slice, as the group it left now has too");
            // A queue seated later in that context waits across groups as well.
            lab.queue(3, Some(COMPUTES + 1));
            assert!(matches!(lab.submit(3, CONTEXT, 3, &compute_work()), WorkStart::Started { .. }));
            assert!(lab.channel(3).in_context_group());
            assert_eq!(lab.slices(), 3);
        }

        #[test]
        fn a_compute_seat_never_gets_commands_for_an_object_it_has_not_got() {
            let mut lab = Lab::new(true);
            lab.queue(1, Some(DRAWS));
            lab.queue(2, Some(COMPUTES));
            lab.submit(1, CONTEXT, 2, &drawing_work());
            assert!(matches!(lab.submit(2, CONTEXT, 2, &compute_work()), WorkStart::Started { .. }));
            let doorbells = lab.backend.device.rm.doorbells().len();
            // A subchannel no graphics channel has an object on is as it always was.
            let software = [increasing(6, 0x100, 1), 0, increasing(7, 0x100, 1), 0];
            assert!(matches!(lab.submit(2, CONTEXT, 2, &software), WorkStart::Started { .. }));
            assert_eq!(lab.submit(2, CONTEXT, 2, &copy_work()), WorkStart::QueueFailed);
            assert_eq!(lab.submit(2, CONTEXT, 2, &drawing_work()), WorkStart::QueueFailed);
            assert_eq!(lab.backend.device.rm.doorbells().len(), doorbells);
            // The drawing seat takes anything, as a channel alone does.
            assert!(matches!(lab.submit(1, CONTEXT, 2, &copy_work()), WorkStart::Started { .. }));
        }

        #[test]
        fn closing_the_queues_empties_the_group() {
            let mut lab = Lab::new(true);
            lab.queue(1, Some(DRAWS));
            lab.queue(2, Some(COMPUTES));
            lab.submit(1, CONTEXT, 2, &drawing_work());
            lab.submit(2, CONTEXT, 2, &compute_work());
            lab.backend.close(1);
            assert_eq!(lab.backend.device.context_groups.members(CONTEXT), 1);
            lab.backend.close(2);
            assert!(lab.backend.device.context_groups.is_empty());
        }
    }
}
