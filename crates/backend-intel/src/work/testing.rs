//! What the tests of [`super`] share: a card that is a stand-in, pages that play the guest's, a
//! host that plays the core, and a space set up with a batch, some data and a fence.

use super::CardWork;
use crate::card;
use crate::protocol::bytes::put_dword;
use crate::protocol::command_header::{COMPACT_SIZE, Header};
use crate::stage::Stage;
use crate::xe::abi::Keying;
use crate::xe::fake::FakeXe;
use crate::xe::plan::BufferUse;
use guesthouse_backend::{
    BackendError, CommandBuffer, GPU_RANGES, GuestSpan, HostServices, MapMemoryType, MapRequest, MemoryError,
    MemoryId, SharedSpan, SpaceId, Work, WorkSource, WorkStart,
};
use std::time::{Duration, Instant};

impl CardWork {
    /// Writes the start of a block through the CPU's view of it.
    pub fn write_for_test(&mut self, memory: MemoryId, bytes: &[u8]) {
        assert!(self.cpu_view(memory).unwrap().write(0, bytes));
    }
}

/// A cap no test reaches unless it means to.
const ROOMY: u64 = 1 << 40;

pub fn work(stage: Stage) -> (CardWork, FakeXe) {
    work_with_cap(stage, ROOMY)
}

pub fn work_with_cap(stage: Stage, cap: u64) -> (CardWork, FakeXe) {
    work_keyed(stage, cap, Keying::BindingIndex)
}

/// Work whose render queues are keyed as `render_keying` says.
pub fn work_keyed(stage: Stage, cap: u64, render_keying: Keying) -> (CardWork, FakeXe) {
    let xe = FakeXe::default();
    let work =
        CardWork::new(Box::new(xe.clone()), &card::tests::battlemage(), stage, cap, render_keying).unwrap();
    (work, xe)
}

pub fn map_request(space: SpaceId, memory: MemoryId, address: u64, offset: u64, length: u64) -> MapRequest {
    MapRequest {
        space,
        memory,
        offset,
        length,
        address,
        small_pages: false,
        gpu_cached: false,
        readable: true,
        writable: true,
        executable: false,
        memory_type: MapMemoryType::Default,
        driver_protection: 0,
    }
}

/// Zeroed pages at a fixed host address, standing in for the guest's.
pub struct Pages(Box<[u64]>);

impl Pages {
    pub fn new(bytes: usize) -> Self {
        Self(vec![0u64; bytes / 8].into_boxed_slice())
    }

    pub fn address(&self) -> u64 {
        self.0.as_ptr() as u64
    }

    pub fn len(&self) -> usize {
        self.0.len() * 8
    }

    pub fn bytes(&self) -> Vec<u8> {
        // SAFETY: the pages are plain memory this value owns. The stand-in card writes them through
        // their address, so they are read the same way.
        (0..self.len()).map(|at| unsafe { (self.address() as *const u8).add(at).read_volatile() }).collect()
    }

    pub fn qword(&self, offset: usize) -> u64 {
        // SAFETY: as in `bytes`, and the offset is checked by the slice of the caller's test.
        assert!(offset + 8 <= self.len());
        unsafe { (self.address() as *const u8).add(offset).cast::<u64>().read_volatile() }
    }

    pub fn write(&self, offset: usize, bytes: &[u8]) {
        assert!(offset + bytes.len() <= self.len());
        // SAFETY: inside the pages, checked above.
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                (self.address() as *mut u8).add(offset),
                bytes.len(),
            )
        };
    }
}

/// The core, as far as work on the card asks it anything: guest pages by GPU address, and fences by
/// guest physical address.
#[derive(Default)]
pub struct FakeHost {
    /// Guest pages mapped in a space: the space, the GPU address, the host address and the length.
    spans: Vec<(SpaceId, u64, u64, usize)>,
    /// Fences: the guest physical address, and where it is in this process.
    fences: Vec<(u64, u64)>,
    /// Every fence the host itself wrote.
    pub written: Vec<(u64, u64)>,
    /// Monitored fences a space's process holds: the space, the GPU address and the fence's guest
    /// physical address.
    monitored: Vec<(SpaceId, u64, u64)>,
    /// GPU addresses handed out so far, from [`FakeHost::ADDRESSES_FROM`] up.
    addresses_taken: u64,
}

impl FakeHost {
    /// Where the addresses the host hands out start: high in the guests' range, past anything the
    /// tests map themselves.
    pub const ADDRESSES_FROM: u64 = 0x80_0000_0000;

    pub fn show(&mut self, space: SpaceId, gpu_address: u64, pages: &Pages) {
        self.spans.push((space, gpu_address, pages.address(), pages.len()));
    }

    pub fn hold_monitored(&mut self, space: SpaceId, gpu_address: u64, guest_address: u64) {
        self.monitored.push((space, gpu_address, guest_address));
    }

    /// The process lets go of every monitored fence it held.
    pub fn release_monitored(&mut self) {
        self.monitored.clear();
    }

    pub fn fence_at(&mut self, guest_address: u64, host_address: u64) {
        self.fences.push((guest_address, host_address));
    }

    fn fence(&self, guest_address: u64) -> Result<*mut u64, MemoryError> {
        let found = self.fences.iter().find(|(guest, _)| *guest == guest_address);
        found.map(|(_, host)| *host as *mut u64).ok_or(MemoryError::Outside(guest_address))
    }
}

impl HostServices for FakeHost {
    fn take_gpu_addresses(&mut self, size: u64, align: u64) -> Result<u64, BackendError> {
        let address = (Self::ADDRESSES_FROM + self.addresses_taken).next_multiple_of(align.max(1));
        self.addresses_taken = address + size - Self::ADDRESSES_FROM;
        Ok(address)
    }

    fn read_at_gpu_address(&self, space: SpaceId, address: u64, out: &mut [u8]) -> Option<usize> {
        self.read_guest_span(space, address, out)
    }

    fn shared_span(&self, _space: SpaceId, _address: u64) -> Option<SharedSpan> {
        None
    }

    fn guest_span(&self, _space: SpaceId, _address: u64) -> Option<GuestSpan> {
        None
    }

    fn read_guest_span(&self, space: SpaceId, address: u64, out: &mut [u8]) -> Option<usize> {
        let &(_, start, host, length) = self.spans.iter().find(|(held, start, _, length)| {
            *held == space && (*start..*start + *length as u64).contains(&address)
        })?;
        let offset = (address - start) as usize;
        let count = out.len().min(length - offset);
        // SAFETY: the span is pages a test keeps alive, and the range was cut to its length.
        unsafe { std::ptr::copy_nonoverlapping((host as *const u8).add(offset), out.as_mut_ptr(), count) };
        Some(count)
    }

    fn write_guest_span(&mut self, space: SpaceId, address: u64, value: u32) -> bool {
        let found = self.spans.iter().find(|(held, start, _, length)| {
            *held == space && (*start..*start + *length as u64).contains(&address)
        });
        let Some(&(_, start, host, length)) = found else { return false };
        let offset = (address - start) as usize;
        assert!(offset + 4 <= length);
        // SAFETY: the span is pages a test keeps alive, and the word lies inside it.
        unsafe { (host as *mut u8).add(offset).cast::<u32>().write_unaligned(value) };
        true
    }

    fn write_u32(&mut self, _address: u64, _value: u32) -> Result<(), MemoryError> {
        Err(MemoryError::Outside(_address))
    }

    fn write_u64(&mut self, address: u64, value: u64) -> Result<(), MemoryError> {
        let fence = self.fence(address)?;
        // SAFETY: a fence a test registered, in pages it keeps alive.
        unsafe { fence.write_volatile(value) };
        self.written.push((address, value));
        Ok(())
    }

    fn read_guest(&self, _address: u64, _out: &mut [u8]) -> usize {
        0
    }

    fn fence_value(&self, fence: u64) -> Result<u64, MemoryError> {
        // SAFETY: as in `write_u64`.
        Ok(unsafe { self.fence(fence)?.read_volatile() })
    }

    fn monitored_fence(&self, space: SpaceId, gpu_address: u64) -> Option<u64> {
        let found = self.monitored.iter().find(|(held, gpu, _)| *held == space && *gpu == gpu_address);
        found.map(|&(_, _, guest)| guest)
    }
}

/// A submission a test can change before it is started.
pub struct WorkBuilder {
    pub source: WorkSource,
    pub queue: u32,
    pub space: SpaceId,
    pub node: u32,
    pub private: Vec<u8>,
    pub commands: Option<CommandBuffer>,
    pub fence: u64,
    pub fence_gpu_address: u64,
    pub fence_value: u64,
    pub may_run: bool,
    pub queue_failed: bool,
}

/// Where the header's first dword keeps the context type and the patch-list bit.
const CONTEXT_TYPE_SHIFT: u32 = 16;
const PATCH_LIST_BIT: u32 = 1 << 22;
/// OpenCL's context type, on the compute node, as the lab's guest submits.
const OPENCL: u32 = 5;
const COMPUTE_NODE: u32 = 5;

impl WorkBuilder {
    pub fn context_type(mut self, kind: u32) -> Self {
        put_dword(&mut self.private, 0, kind << CONTEXT_TYPE_SHIFT);
        self
    }

    pub fn patch_list(mut self) -> Self {
        put_dword(&mut self.private, 0, OPENCL << CONTEXT_TYPE_SHIFT | PATCH_LIST_BIT);
        self
    }

    /// The same, sent to a context: its header names a tag and a monitored fence.
    pub fn sent_to_context(mut self, tag: (u64, u32), fence: (u64, u64)) -> Self {
        self.source = WorkSource::Context;
        crate::protocol::command_header::tests::put_marks(&mut self.private, tag, fence);
        self
    }

    pub fn work(&self) -> Work<'_> {
        Work {
            source: self.source,
            queue: self.queue,
            space: self.space,
            node: self.node,
            client_hint: 0,
            context_queues: 1,
            private: &self.private,
            commands: self.commands,
            fence: self.fence,
            fence_gpu_address: self.fence_gpu_address,
            fence_value: self.fence_value,
            may_run: self.may_run,
            queue_failed: self.queue_failed,
        }
    }
}

const BATCH_AT: u64 = 0x1_4000_0000;
const DATA_AT: u64 = 0x400_0000;
const FENCE_AT: u64 = 0x500_0000;
const BATCH_BYTES: u64 = 0x1000;
const DATA_BYTES: usize = 0x2000;
const FENCE_BYTES: usize = 0x1000;
/// The fence's guest physical address, which only the test host resolves.
const FENCE_GUEST_ADDRESS: u64 = 0x7000_0000;
/// Where in the fence block a monitored fence is, next to the queue's.
const MONITORED_OFFSET: u64 = 0x100;

/// A process's space with a batch in video memory, data in guest pages and the fence block mapped,
/// as the core sets one up.
pub struct Rig {
    pub work: CardWork,
    pub xe: FakeXe,
    pub host: FakeHost,
    pub space: SpaceId,
    batch: MemoryId,
    data: (MemoryId, Pages),
    fence: (MemoryId, Pages),
    batch_length: u32,
    now: Instant,
}

impl Rig {
    pub fn new(stage: Stage) -> Self {
        Self::keyed(stage, Keying::BindingIndex)
    }

    /// A rig whose render queues are keyed as `render_keying` says.
    pub fn keyed(stage: Stage, render_keying: Keying) -> Self {
        let (mut work, xe) = work_keyed(stage, ROOMY, render_keying);
        let (batch, _) = work.allocate(BATCH_BYTES, BufferUse::Card).unwrap();
        let data_pages = Pages::new(DATA_BYTES);
        let data = work.adopt(data_pages.address(), DATA_BYTES as u64).unwrap();
        let fence_pages = Pages::new(FENCE_BYTES);
        let fence = work.adopt(fence_pages.address(), FENCE_BYTES as u64).unwrap();
        let mut host = FakeHost::default();
        host.fence_at(FENCE_GUEST_ADDRESS, fence_pages.address());
        host.fence_at(FENCE_GUEST_ADDRESS + MONITORED_OFFSET, fence_pages.address() + MONITORED_OFFSET);
        let mut rig = Self {
            work,
            xe,
            host,
            space: SpaceId(0),
            batch,
            data: (data, data_pages),
            fence: (fence, fence_pages),
            batch_length: 0,
            now: Instant::now(),
        };
        rig.space = rig.second_space();
        rig
    }

    /// Another process's space, with the same three things mapped at the same addresses.
    pub fn second_space(&mut self) -> SpaceId {
        let space = self.work.create_space(&GPU_RANGES).unwrap();
        self.work.map(&map_request(space, self.batch, BATCH_AT, 0, BATCH_BYTES)).unwrap();
        self.work.map(&map_request(space, self.data.0, DATA_AT, 0, DATA_BYTES as u64)).unwrap();
        self.work.map(&map_request(space, self.fence.0, FENCE_AT, 0, FENCE_BYTES as u64)).unwrap();
        self.host.show(space, DATA_AT, &self.data.1);
        self.host.show(space, FENCE_AT, &self.fence.1);
        space
    }

    pub fn batch_address(&self) -> u64 {
        BATCH_AT
    }

    /// The memory the data block is, mapped at [`Rig::data_address`].
    pub fn data_memory(&self) -> MemoryId {
        self.data.0
    }

    pub fn data_bytes(&self) -> u64 {
        DATA_BYTES as u64
    }

    pub fn data_address(&self) -> u64 {
        DATA_AT
    }

    pub fn fence_gpu_address(&self) -> u64 {
        FENCE_AT
    }

    pub fn fence_guest_address(&self) -> u64 {
        FENCE_GUEST_ADDRESS
    }

    /// Writes the batch into video memory, zeroed first, and makes submissions name its length.
    pub fn write_batch(&mut self, dwords: &[u32]) {
        let view = self.work.cpu_view(self.batch).unwrap();
        assert!(view.write(0, &vec![0u8; BATCH_BYTES as usize]));
        assert!(view.write(0, &crate::xe::batch::bytes(dwords)));
        self.batch_length = dwords.len() as u32 * 4;
    }

    pub fn write_data(&mut self, offset: usize, bytes: &[u8]) {
        self.data.1.write(offset, bytes);
    }

    pub fn data_qword(&self, offset: usize) -> u64 {
        self.data.1.qword(offset)
    }

    /// The fence's value, as the guest would read it.
    pub fn fence(&self) -> u64 {
        self.fence.1.qword(0)
    }

    /// A monitored fence in the fence block, by GPU address and guest physical address. It is the
    /// process's to name once [`FakeHost::hold_monitored`] says so.
    pub fn monitored_addresses(&self) -> (u64, u64) {
        (FENCE_AT + MONITORED_OFFSET, FENCE_GUEST_ADDRESS + MONITORED_OFFSET)
    }

    /// The monitored fence's value, as the guest would read it.
    pub fn monitored(&self) -> u64 {
        self.fence.1.qword(MONITORED_OFFSET as usize)
    }

    /// Sets the monitored fence, as the guest or another run would have.
    pub fn set_monitored(&mut self, value: u64) {
        self.fence.1.write(MONITORED_OFFSET as usize, &value.to_le_bytes());
    }

    /// Reads a dword of video memory the batch buffer holds, at an offset from its start.
    pub fn batch_dword(&mut self, offset: u64) -> u32 {
        let mut bytes = [0; 4];
        assert!(self.work.cpu_view(self.batch).unwrap().read(offset, &mut bytes));
        u32::from_le_bytes(bytes)
    }

    pub fn data_dword(&self, offset: usize) -> u32 {
        let bytes = self.data.1.bytes();
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }

    pub fn submission(&self, queue: u32, value: u64) -> WorkBuilder {
        self.submission_in(self.space, queue, value)
    }

    pub fn submission_in(&self, space: SpaceId, queue: u32, value: u64) -> WorkBuilder {
        let builder = WorkBuilder {
            source: WorkSource::HardwareQueue,
            queue,
            space,
            node: COMPUTE_NODE,
            private: vec![0; COMPACT_SIZE],
            commands: Some(CommandBuffer { address: BATCH_AT, length: self.batch_length }),
            fence: FENCE_GUEST_ADDRESS,
            fence_gpu_address: FENCE_AT,
            fence_value: value,
            may_run: true,
            queue_failed: false,
        };
        builder.context_type(OPENCL)
    }

    pub fn start(&mut self, builder: &WorkBuilder) -> WorkStart {
        let work = builder.work();
        self.work.start(&mut self.host, &work, &Header::read(work.private))
    }

    /// Looks at the runs, at a time far enough on that the look isn't skipped.
    pub fn poll(&mut self) {
        self.now += Duration::from_millis(100);
        self.work.poll(&mut self.host, self.now);
    }

    pub fn poll_later(&mut self) {
        self.poll();
    }
}
