//! Creating, destroying, opening and locking allocations.

use crate::wire::{
    Malformed, U32List, U64List, declared_count, pad_to, put_u32, put_u64, set_u32, set_u64, u8_at, u32_at,
};
use crate::{HEADER_SIZE, Status};

const RUNTIME_DATA_SIZE: usize = 0x20;
const DRIVER_DATA_SIZE: usize = 0x24;
const COUNT: usize = 0x28;
const FLAGS: usize = 0x2C;
const ENTRIES: usize = 0x40;
const ENTRY_SIZE: usize = 12;
const ENTRY_PRIVATE_SIZE: usize = 4;
const HANDLE_SIZE: usize = 4;

/// Request flags that say the guest brings its own memory and will list its pages afterwards.
const GUEST_BACKED: u32 = 0x20020;

/// Request flags: the resource is shared, and by an NT handle rather than a global one.
const CREATE_SHARED: u32 = 0x2;
const NT_SECURITY_SHARING: u32 = 0x40;

/// An entry flag: this allocation is a screen's primary surface.
pub const PRIMARY: u32 = 1;

/// A reply flag. Without it the guest maps locked memory write-combined, as if it were video memory
/// behind a PCI window. Ours is plain RAM, and reading it that way is many times slower.
pub const CACHED_FOR_CPU: u32 = 0x80000;

/// A request to create the allocations of one resource.
#[derive(Debug, Clone, Copy)]
pub struct CreateAllocation<'a> {
    message: &'a [u8],
    pub count: usize,
    pub flags: u32,
    first_private: usize,
}

/// One allocation in a [`CreateAllocation`].
#[derive(Debug, Clone, Copy)]
pub struct AllocationEntry<'a> {
    pub flags: u32,
    /// The size the guest declared, which `private` may fall short of.
    pub private_size: u32,
    pub private: &'a [u8],
}

impl<'a> CreateAllocation<'a> {
    pub fn parse(message: &'a [u8]) -> Result<Self, Malformed> {
        let runtime = u32_at(message, RUNTIME_DATA_SIZE)? as usize;
        let driver = u32_at(message, DRIVER_DATA_SIZE)? as usize;
        let count = declared_count(message, COUNT, CreateAllocationAnswer::ENTRY_SIZE)?;
        let flags = u32_at(message, FLAGS)?;
        let first_private = count
            .checked_mul(ENTRY_SIZE)
            .and_then(|entries| entries.checked_add(ENTRIES + runtime + driver))
            .ok_or(Malformed)?;
        if message.len() < ENTRIES + count * ENTRY_SIZE {
            return Err(Malformed);
        }
        Ok(Self { message, count, flags, first_private })
    }

    pub fn guest_backed(&self) -> bool {
        self.flags & GUEST_BACKED != 0
    }

    /// Shared through a global handle, which another device opens the resource by later.
    pub fn shared_by_global_handle(&self) -> bool {
        self.flags & CREATE_SHARED != 0 && self.flags & NT_SECURITY_SHARING == 0
    }

    /// The entries in order, each with its private data. The data blocks follow the table and each
    /// other, and any that run past the message are cut short.
    pub fn entries(&self) -> impl Iterator<Item = AllocationEntry<'a>> + use<'a> {
        let message = self.message;
        let mut private_at = self.first_private;
        (0..self.count).map(move |index| {
            let entry = ENTRIES + index * ENTRY_SIZE;
            let private_size = u32_at(message, entry + ENTRY_PRIVATE_SIZE).unwrap_or(0);
            let private = crate::wire::clamped(message, private_at, private_size as usize);
            private_at += private_size as usize;
            AllocationEntry { flags: u32_at(message, entry).unwrap_or(0), private_size, private }
        })
    }
}

/// The guest changing where an allocation may live, or how the GPU reaches it. The fields follow
/// the header: the paging queue, the allocation, the segments it may use, the segment preference
/// (several segments packed into one word), flags (bit 0 is "accessed physically"), the paging
/// fence value (an answer, so empty here), and a mask of the properties being changed (bit 0
/// accessed physically, bit 1 supported segments, bit 2 preferred segment).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdateAllocationProperty {
    pub paging_queue: u32,
    pub allocation: u32,
    pub supported_segments: u32,
    pub segment_preference: u32,
    pub flags: u32,
    pub property_mask: u32,
}

impl UpdateAllocationProperty {
    const PAGING_QUEUE: usize = HEADER_SIZE;
    const ALLOCATION: usize = HEADER_SIZE + 4;
    const SUPPORTED_SEGMENTS: usize = HEADER_SIZE + 8;
    const SEGMENT_PREFERENCE: usize = HEADER_SIZE + 12;
    const FLAGS: usize = HEADER_SIZE + 16;
    const PROPERTY_MASK: usize = HEADER_SIZE + 32;

    pub fn parse(message: &[u8]) -> Result<Self, Malformed> {
        Ok(Self {
            paging_queue: u32_at(message, Self::PAGING_QUEUE)?,
            allocation: u32_at(message, Self::ALLOCATION)?,
            supported_segments: u32_at(message, Self::SUPPORTED_SEGMENTS)?,
            segment_preference: u32_at(message, Self::SEGMENT_PREFERENCE)?,
            flags: u32_at(message, Self::FLAGS)?,
            property_mask: u32_at(message, Self::PROPERTY_MASK)?,
        })
    }

    /// The paging fence value the change reaches, then a status. The change is complete when
    /// it is answered, so the fence value is zero and the guest has nothing to wait for.
    pub fn answer(status: Status, out: &mut Vec<u8>) {
        put_u64(out, 0);
        put_u32(out, status.0);
        put_u32(out, 0);
    }
}

/// The guest offering allocations back, so the host may reuse their memory. The fields follow the
/// header: the device, how many handles, the priority, flags, whether the handles are resources
/// rather than allocations (one byte, padded), then the handles. The answer is a bare status.
#[derive(Debug, Clone, Copy)]
pub struct OfferAllocations<'a> {
    pub device: u32,
    pub priority: u32,
    pub flags: u32,
    pub resources: bool,
    pub handles: U32List<'a>,
}

impl<'a> OfferAllocations<'a> {
    const DEVICE: usize = HEADER_SIZE;
    const COUNT: usize = HEADER_SIZE + 4;
    const PRIORITY: usize = HEADER_SIZE + 8;
    const FLAGS: usize = HEADER_SIZE + 12;
    const RESOURCES: usize = HEADER_SIZE + 16;
    const HANDLES: usize = HEADER_SIZE + 20;

    pub fn parse(message: &'a [u8]) -> Result<Self, Malformed> {
        let count = declared_count(message, Self::COUNT, HANDLE_SIZE)?;
        Ok(Self {
            device: u32_at(message, Self::DEVICE)?,
            priority: u32_at(message, Self::PRIORITY)?,
            flags: u32_at(message, Self::FLAGS)?,
            resources: u8_at(message, Self::RESOURCES)? != 0,
            handles: U32List::at(message, Self::HANDLES, count)?,
        })
    }
}

/// The guest taking offered allocations back. The fields follow the header: the device, the paging
/// queue, how many handles, whether they are resources, whether the guest wants a result for each
/// (two bytes, padded), then the handles.
#[derive(Debug, Clone, Copy)]
pub struct ReclaimAllocations<'a> {
    pub device: u32,
    pub paging_queue: u32,
    pub resources: bool,
    pub write_results: bool,
    pub handles: U32List<'a>,
}

impl<'a> ReclaimAllocations<'a> {
    const DEVICE: usize = HEADER_SIZE;
    const PAGING_QUEUE: usize = HEADER_SIZE + 4;
    const COUNT: usize = HEADER_SIZE + 8;
    const RESOURCES: usize = HEADER_SIZE + 12;
    const WRITE_RESULTS: usize = HEADER_SIZE + 13;
    const HANDLES: usize = HEADER_SIZE + 16;

    pub fn parse(message: &'a [u8]) -> Result<Self, Malformed> {
        let count = declared_count(message, Self::COUNT, HANDLE_SIZE)?;
        Ok(Self {
            device: u32_at(message, Self::DEVICE)?,
            paging_queue: u32_at(message, Self::PAGING_QUEUE)?,
            resources: u8_at(message, Self::RESOURCES)? != 0,
            write_results: u8_at(message, Self::WRITE_RESULTS)? != 0,
            handles: U32List::at(message, Self::HANDLES, count)?,
        })
    }

    /// A paging fence value of zero, which the guest has nothing to wait for, a status, and, if
    /// the guest asked, a result of zero ("reclaimed intact") for each handle. Nothing is ever
    /// discarded, as the host never takes the memory an offer gives up.
    pub fn answer(&self, out: &mut Vec<u8>) {
        put_u64(out, 0);
        put_u32(out, Status::SUCCESS.0);
        if self.write_results {
            (0..self.handles.len()).for_each(|_| put_u32(out, 0));
        } else {
            put_u32(out, 0);
        }
    }
}

/// The answer: fixed fields, a table with an entry per allocation, then the private data.
pub struct CreateAllocationAnswer;

impl CreateAllocationAnswer {
    const STATUS: usize = 0;
    const RESOURCE: usize = 4;
    const GLOBAL_SHARE: usize = 8;
    const HEADER: usize = 0x10;
    const ENTRY_SIZE: usize = 0x48;
    const ENTRY_PRIVATE_SIZE: usize = 4;
    const ENTRY_FLAGS: usize = 8;
    const ENTRY_SIZE_BYTES: usize = 0x10;
    const ENTRY_SURFACE: usize = 0x20;
    const PADDING_ALIGN: usize = 8;

    pub fn begin(count: usize, out: &mut Vec<u8>) {
        out.resize(Self::HEADER + count * Self::ENTRY_SIZE, 0);
    }

    pub fn fail(status: Status, out: &mut [u8]) {
        set_u32(out, Self::STATUS, status.0);
    }

    pub fn set_resource(resource: u32, out: &mut [u8]) {
        set_u32(out, Self::RESOURCE, resource);
    }

    /// The handle the guest sends back to open the resource from another device.
    pub fn set_global_share(handle: u32, out: &mut [u8]) {
        set_u32(out, Self::GLOBAL_SHARE, handle);
    }

    pub fn set_entry(index: usize, handle: u32, private_size: u32, flags: u32, size: u64, out: &mut [u8]) {
        let entry = Self::HEADER + index * Self::ENTRY_SIZE;
        set_u32(out, entry, handle);
        set_u32(out, entry + Self::ENTRY_PRIVATE_SIZE, private_size);
        set_u32(out, entry + Self::ENTRY_FLAGS, flags);
        set_u64(out, entry + Self::ENTRY_SIZE_BYTES, size);
    }

    /// Records a surface's shape. The guest keeps this and compares it when it sets a display mode
    /// or flips, so an unset refresh rate would be refused there.
    pub fn set_surface(index: usize, fields: [u32; 7], out: &mut [u8]) {
        let surface = Self::HEADER + index * Self::ENTRY_SIZE + Self::ENTRY_SURFACE;
        for (at, field) in fields.into_iter().enumerate() {
            set_u32(out, surface + at * 4, field);
        }
    }

    pub fn finish(out: &mut Vec<u8>) {
        pad_to(out, out.len().next_multiple_of(Self::PADDING_ALIGN));
    }

    /// The size of an entry in a finished answer.
    pub fn size_of_entry(answer: &[u8], index: usize) -> Result<u64, Malformed> {
        crate::wire::u64_at(answer, Self::HEADER + index * Self::ENTRY_SIZE + Self::ENTRY_SIZE_BYTES)
    }

    pub fn surface_of_entry(answer: &[u8], index: usize) -> Result<[u32; 7], Malformed> {
        let surface = Self::HEADER + index * Self::ENTRY_SIZE + Self::ENTRY_SURFACE;
        let mut fields = [0; 7];
        for (at, field) in fields.iter_mut().enumerate() {
            *field = u32_at(answer, surface + at * 4)?;
        }
        Ok(fields)
    }

    pub fn private_data_start(count: usize) -> usize {
        Self::HEADER + count * Self::ENTRY_SIZE
    }
}

/// A request to destroy a resource, or named allocations, or both.
#[derive(Debug, Clone, Copy)]
pub struct DestroyAllocation<'a> {
    pub resource: u32,
    pub handles: U32List<'a>,
    message: &'a [u8],
    count: usize,
}

impl<'a> DestroyAllocation<'a> {
    const RESOURCE: usize = 0x1C;
    const COUNT: usize = 0x20;
    const HANDLES: usize = 0x28;

    /// Reads the fixed fields only. A resource still shared with someone is released without
    /// looking at the list, so a short one must not fail that case.
    pub fn parse(message: &'a [u8]) -> Result<Self, Malformed> {
        Ok(Self {
            resource: u32_at(message, Self::RESOURCE)?,
            count: u32_at(message, Self::COUNT)? as usize,
            handles: U32List::default(),
            message,
        })
    }

    pub fn with_handles(mut self) -> Result<Self, Malformed> {
        self.handles = U32List::at(self.message, Self::HANDLES, self.count)?;
        Ok(self)
    }
}

/// A request to open a resource another process made.
#[derive(Debug, Clone, Copy)]
pub struct OpenResource {
    pub shared: u32,
    pub count: usize,
}

impl OpenResource {
    const SHARED: usize = 0x20;
    const COUNT: usize = 0x24;
    const ANSWER_FIXED_SIZE: usize = 0xF;
    const ANSWER_ALIGN_MASK: usize = !7;

    pub fn parse(message: &[u8]) -> Result<Self, Malformed> {
        let count = declared_count(message, Self::COUNT, HANDLE_SIZE)?;
        Ok(Self { shared: u32_at(message, Self::SHARED)?, count })
    }

    fn answer_size(&self) -> usize {
        (self.count * HANDLE_SIZE + Self::ANSWER_FIXED_SIZE) & Self::ANSWER_ALIGN_MASK
    }

    pub fn refuse(&self, out: &mut Vec<u8>) {
        put_u32(out, 0);
        put_u32(out, Status::INVALID_PARAMETER.0);
        pad_to(out, self.answer_size());
    }

    pub fn answer(&self, resource: u32, handles: &[u32], out: &mut Vec<u8>) {
        put_u32(out, resource);
        put_u32(out, Status::SUCCESS.0);
        handles.iter().for_each(|&handle| put_u32(out, handle));
        pad_to(out, self.answer_size());
    }
}

/// The allocation in a lock request.
pub fn locked_allocation(message: &[u8]) -> Result<u32, Malformed> {
    u32_at(message, 0x1C)
}

/// Answers a lock: a status, padding, and the guest physical address of the memory.
pub fn lock_answer(status: Status, address: u64, out: &mut Vec<u8>) {
    put_u32(out, status.0);
    put_u32(out, 0);
    put_u64(out, address);
}

/// One message's share of a guest-backed allocation's page list.
#[derive(Debug, Clone, Copy)]
pub struct SetGuestPages<'a> {
    pub allocation: u32,
    pub first: usize,
    pub count: usize,
    message: &'a [u8],
}

impl<'a> SetGuestPages<'a> {
    const ALLOCATION: usize = HEADER_SIZE + 4;
    const COUNT: usize = HEADER_SIZE + 8;
    const FIRST: usize = HEADER_SIZE + 12;
    const PAGES: usize = 0x28;

    pub fn parse(message: &'a [u8]) -> Result<Self, Malformed> {
        Ok(Self {
            allocation: u32_at(message, Self::ALLOCATION)?,
            count: u32_at(message, Self::COUNT)? as usize,
            first: u32_at(message, Self::FIRST)? as usize,
            message,
        })
    }

    pub fn pages(&self) -> Result<U64List<'a>, Malformed> {
        U64List::at(self.message, Self::PAGES, self.count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handles_message(header_fields: &[u8], handles: &[u32]) -> Vec<u8> {
        let mut message = vec![0; HEADER_SIZE];
        message.extend_from_slice(header_fields);
        handles.iter().for_each(|&handle| put_u32(&mut message, handle));
        message
    }

    #[test]
    fn an_offer_names_its_handles_after_the_flags() {
        let mut fields = vec![];
        [7u32, 2, 2, 1, 1].iter().for_each(|&word| put_u32(&mut fields, word));
        let message = handles_message(&fields, &[0x10, 0x20]);
        let offer = OfferAllocations::parse(&message).unwrap();
        assert_eq!((offer.device, offer.priority, offer.flags, offer.resources), (7, 2, 1, true));
        assert_eq!(offer.handles.iter().collect::<Vec<_>>(), [0x10, 0x20]);
        assert!(OfferAllocations::parse(&message[..message.len() - 4]).is_err());
    }

    #[test]
    fn a_reclaim_is_answered_with_no_fence_and_nothing_discarded() {
        let mut fields = vec![];
        [7u32, 9, 3, 0x0100].iter().for_each(|&word| put_u32(&mut fields, word));
        let message = handles_message(&fields, &[0x10, 0x20, 0x30]);
        let reclaim = ReclaimAllocations::parse(&message).unwrap();
        assert_eq!((reclaim.device, reclaim.paging_queue), (7, 9));
        assert!(!reclaim.resources && reclaim.write_results);
        let mut out = Vec::new();
        reclaim.answer(&mut out);
        assert_eq!(out, [0; 24]);
        let mut quiet = message.clone();
        quiet[HEADER_SIZE + 13] = 0;
        let mut out = Vec::new();
        ReclaimAllocations::parse(&quiet).unwrap().answer(&mut out);
        assert_eq!(out, [0; 16]);
        assert!(ReclaimAllocations::parse(&message[..message.len() - 4]).is_err());
    }

    #[test]
    fn an_allocation_property_update_is_read_and_answered_without_a_fence() {
        let mut message = vec![0; 64];
        message[HEADER_SIZE..HEADER_SIZE + 4].copy_from_slice(&0x4003_7700u32.to_le_bytes());
        message[HEADER_SIZE + 4..HEADER_SIZE + 8].copy_from_slice(&0x4003_9080u32.to_le_bytes());
        message[HEADER_SIZE + 8..HEADER_SIZE + 12].copy_from_slice(&0x8000_0000u32.to_le_bytes());
        message[HEADER_SIZE + 32] = 6;
        let update = UpdateAllocationProperty::parse(&message).unwrap();
        assert_eq!((update.paging_queue, update.allocation), (0x4003_7700, 0x4003_9080));
        assert_eq!((update.supported_segments, update.property_mask), (0x8000_0000, 6));
        assert!(UpdateAllocationProperty::parse(&message[..58]).is_err());
        let mut out = Vec::new();
        UpdateAllocationProperty::answer(Status::SUCCESS, &mut out);
        assert_eq!(out, [0; 16]);
    }
    use crate::wire::u64_at;

    fn create_message(guest_backed: bool, blocks: &[&[u8]], runtime: &[u8]) -> Vec<u8> {
        let mut message = vec![0; ENTRIES];
        set_u32(&mut message, RUNTIME_DATA_SIZE, runtime.len() as u32);
        set_u32(&mut message, COUNT, blocks.len() as u32);
        set_u32(&mut message, FLAGS, if guest_backed { 0x20 } else { 0 });
        for (index, block) in blocks.iter().enumerate() {
            message.extend_from_slice(&(index as u32).to_le_bytes());
            message.extend_from_slice(&(block.len() as u32).to_le_bytes());
            message.extend_from_slice(&[0; 4]);
        }
        message.extend_from_slice(runtime);
        blocks.iter().for_each(|block| message.extend_from_slice(block));
        message
    }

    #[test]
    fn private_blocks_follow_the_runtime_data() {
        let message = create_message(false, &[b"first", b"second!"], b"runtime");
        let request = CreateAllocation::parse(&message).unwrap();
        let blocks: Vec<_> = request.entries().map(|entry| entry.private.to_vec()).collect();
        assert_eq!(blocks, [b"first".to_vec(), b"second!".to_vec()]);
        assert!(!request.guest_backed());
    }

    #[test]
    fn a_count_the_message_cannot_hold_is_refused() {
        let mut message = create_message(true, &[b"x"], b"");
        set_u32(&mut message, COUNT, 1000);
        assert!(CreateAllocation::parse(&message).is_err());
    }

    #[test]
    fn answer_is_padded_to_eight_bytes() {
        let mut out = Vec::new();
        CreateAllocationAnswer::begin(1, &mut out);
        CreateAllocationAnswer::set_entry(0, 0x40, 3, PRIMARY, 0x1000, &mut out);
        out.extend_from_slice(b"abc");
        CreateAllocationAnswer::finish(&mut out);
        assert_eq!(out.len(), 0x10 + 0x48 + 8);
        assert_eq!(u64_at(&out, 0x20), Ok(0x1000));
        assert_eq!(CreateAllocationAnswer::size_of_entry(&out, 0), Ok(0x1000));
    }

    #[test]
    fn open_resource_answers_have_room_for_every_handle() {
        let request = OpenResource { shared: 1, count: 3 };
        let mut out = Vec::new();
        request.answer(9, &[1, 2, 3], &mut out);
        assert_eq!(out.len(), 24);
        let mut out = Vec::new();
        request.refuse(&mut out);
        assert_eq!(out.len(), 24);
        assert_eq!(u32_at(&out, 4), Ok(Status::INVALID_PARAMETER.0));
    }
}
