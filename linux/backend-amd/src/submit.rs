//! Submitting a command buffer and waiting for it through an eventfd.
//!
//! `DRM_AMDGPU_CS` takes a context, an indirect buffer at a GPU address, and a set of chunks. This
//! backend sends two: the IB chunk and a syncobj-out chunk, so the kernel signals a sync object when
//! the work retires. There is no doorbell: the submission is an ioctl. The daemon's own runs wait
//! for it on an eventfd armed on that sync object. A guest's run isn't waited for: it ends by
//! writing the guest's fence, see `fence_ib`, and its sync object is only looked at afterwards.
//!
//! A buffer with a GPU address in this file's VM is not kept there for a submission by that alone:
//! only buffers made for the VM itself are, and a space's are taken in from another open. amdgpu can
//! move any other buffer out while the work runs, and the card then faults on its addresses. So a
//! submission names the buffers it may use in a `BO_HANDLES` chunk, which makes amdgpu bring them in
//! first and keep them in until the work retires.

use crate::ioctl::{self, call, get32, get64, put32, put64};
use crate::memory::GemHandle;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

// Context op and offsets.
const CTX_OP_ALLOC: u32 = 1;
const CTX_OP_FREE: u32 = 2;
const CTX_OP_QUERY_STATE2: u32 = 4;
const CTX_IN_OP: usize = 0;
const CTX_IN_CTX_ID: usize = 8;
const CTX_OUT_CTX_ID: usize = 0;
const CTX_OUT_FLAGS: usize = 0;
const CTX_REQUEST_SIZE: usize = 16;

/// `SYNC_IOC_FILE_INFO` from the kernel's `sync_file.h`, `_IOWR('>', 4, struct sync_file_info)`, and
/// where that structure keeps the fence's status. Asked for no fences, the kernel fills in only that.
const SYNC_FILE_INFO: u64 = 0xC038_3E04;
const SYNC_FILE_INFO_SIZE: usize = 56;
const SYNC_FILE_INFO_STATUS: usize = 32;

// Chunk ids.
const CHUNK_ID_IB: u32 = 0x01;
const CHUNK_ID_SYNCOBJ_IN: u32 = 0x04;
const CHUNK_ID_SYNCOBJ_OUT: u32 = 0x05;
const CHUNK_ID_BO_HANDLES: u32 = 0x06;

// drm_amdgpu_cs_chunk: id, length in dwords, pointer to the chunk's data.
const CHUNK_ID: usize = 0;
const CHUNK_LENGTH_DW: usize = 4;
const CHUNK_DATA: usize = 8;
const CHUNK_SIZE: usize = 16;

// drm_amdgpu_cs_chunk_ib, 32 bytes.
const IB_VA_START: usize = 8;
const IB_BYTES: usize = 16;
const IB_IP_TYPE: usize = 20;
const IB_IP_INSTANCE: usize = 24;
const IB_RING: usize = 28;
const IB_CHUNK_SIZE: usize = 32;

// drm_amdgpu_bo_list_in, 24 bytes, pointing at drm_amdgpu_bo_list_entry records of 8 bytes: a
// handle and a priority, left at 0.
const BO_LIST_NUMBER: usize = 8;
const BO_LIST_ENTRY_SIZE: usize = 12;
const BO_LIST_ENTRIES: usize = 16;
const BO_LIST_SIZE: usize = 24;
const BO_ENTRY_SIZE: usize = 8;

// drm_amdgpu_bo_list_in again, for the ioctl that makes a list: the operation and the list's handle
// come before the number and the pointer, and the answer is the list's handle.
const LIST_OPERATION: usize = 0;
const LIST_HANDLE: usize = 4;
const LIST_NUMBER: usize = 8;
const LIST_ENTRY_SIZE: usize = 12;
const LIST_ENTRIES: usize = 16;
const LIST_OUT_HANDLE: usize = 0;
const OP_CREATE: u32 = 0;
const OP_DESTROY: u32 = 1;
const OP_UPDATE: u32 = 2;

// drm_amdgpu_cs in/out, 24 bytes.
const CS_CTX_ID: usize = 0;
const CS_BO_LIST: usize = 4;
const CS_NUM_CHUNKS: usize = 8;
const CS_CHUNKS: usize = 16;
const CS_OUT_HANDLE: usize = 0;
const CS_REQUEST_SIZE: usize = 24;

// drm_syncobj_create / _destroy, 8 bytes.
const SYNCOBJ_HANDLE: usize = 0;
const SYNCOBJ_REQUEST_SIZE: usize = 8;

// drm_syncobj_wait, 32 bytes: a pointer to the handles, a time to wait until, how many handles, flags,
// and which one was signalled first. A time of 0 is already past, so the call only looks.
const WAIT_HANDLES: usize = 0;
const WAIT_COUNT: usize = 16;
const WAIT_REQUEST_SIZE: usize = 32;

// drm_syncobj_handle, 16 bytes: handle, flags, fd (s32), pad. With the sync file flag the fd is a
// sync file holding the object's fence, which any open of any device can take in.
const HANDLE_HANDLE: usize = 0;
const HANDLE_FLAGS: usize = 4;
const HANDLE_FD: usize = 8;
const HANDLE_REQUEST_SIZE: usize = 16;
const EXPORT_SYNC_FILE: u32 = 1;
const IMPORT_SYNC_FILE: u32 = 1;

// drm_syncobj_eventfd, 24 bytes: handle, flags, point (u64), fd (s32), pad.
const EVENTFD_HANDLE: usize = 0;
const EVENTFD_FD: usize = 16;
const EVENTFD_REQUEST_SIZE: usize = 24;

/// An amdgpu submission context. Work is submitted against it, and it reports lost queues.
#[derive(Debug)]
pub struct Context {
    id: u32,
}

impl Context {
    pub fn create(file: &impl AsRawFd) -> io::Result<Self> {
        let mut request = [0u8; CTX_REQUEST_SIZE];
        put32(&mut request, CTX_IN_OP, CTX_OP_ALLOC);
        ioctl::drm_ioctl(file, call::ctx(), &mut request)?;
        Ok(Self { id: ioctl::get32(&request, CTX_OUT_CTX_ID) })
    }

    pub fn id(&self) -> u32 {
        self.id
    }

    /// What amdgpu says has happened to the context since it was made: the
    /// `AMDGPU_CTX_QUERY2_FLAGS_*` bits, such as a reset or lost video memory.
    pub fn reset_flags(&self, file: &impl AsRawFd) -> io::Result<u64> {
        let mut request = [0u8; CTX_REQUEST_SIZE];
        put32(&mut request, CTX_IN_OP, CTX_OP_QUERY_STATE2);
        put32(&mut request, CTX_IN_CTX_ID, self.id);
        ioctl::drm_ioctl(file, call::ctx(), &mut request)?;
        Ok(get64(&request, CTX_OUT_FLAGS))
    }

    pub fn destroy(&self, file: &impl AsRawFd) {
        let mut request = [0u8; CTX_REQUEST_SIZE];
        put32(&mut request, CTX_IN_OP, CTX_OP_FREE);
        put32(&mut request, CTX_IN_CTX_ID, self.id);
        let _ = ioctl::drm_ioctl(file, call::ctx(), &mut request);
    }
}

/// A DRM sync object. The kernel signals it when the submission that names it retires.
#[derive(Debug)]
pub struct Syncobj {
    handle: u32,
}

impl Syncobj {
    pub fn create(file: &impl AsRawFd) -> io::Result<Self> {
        let mut request = [0u8; SYNCOBJ_REQUEST_SIZE];
        ioctl::drm_ioctl(file, call::syncobj_create(), &mut request)?;
        Ok(Self { handle: ioctl::get32(&request, SYNCOBJ_HANDLE) })
    }

    pub fn handle(&self) -> u32 {
        self.handle
    }

    /// Another name for the same object, to ask about it. It is never destroyed through this name.
    pub fn alias(&self) -> Self {
        Self { handle: self.handle }
    }

    /// A handle no kernel made, for a test.
    #[cfg(test)]
    pub fn made_up(handle: u32) -> Self {
        Self { handle }
    }

    /// Arms an eventfd that becomes readable when this sync object is signalled. The eventfd is the
    /// event source the backend trait does not have today.
    pub fn arm_eventfd(&self, file: &impl AsRawFd) -> io::Result<OwnedFd> {
        // SAFETY: eventfd returns a fresh descriptor or -1.
        let raw = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the descriptor is fresh and owned here.
        let event = unsafe { OwnedFd::from_raw_fd(raw) };
        let mut request = [0u8; EVENTFD_REQUEST_SIZE];
        put32(&mut request, EVENTFD_HANDLE, self.handle);
        put32(&mut request, EVENTFD_FD, event.as_raw_fd() as u32);
        ioctl::drm_ioctl(file, call::syncobj_eventfd(), &mut request)?;
        Ok(event)
    }

    /// Has the kernel add one to the eventfd `event` when this sync object is signalled. Several
    /// sync objects can post to the same eventfd.
    pub fn post_to(&self, file: &impl AsRawFd, event: RawFd) -> io::Result<()> {
        let mut request = [0u8; EVENTFD_REQUEST_SIZE];
        put32(&mut request, EVENTFD_HANDLE, self.handle);
        put32(&mut request, EVENTFD_FD, event as u32);
        ioctl::drm_ioctl(file, call::syncobj_eventfd(), &mut request)
    }

    /// A sync file of the fence it holds now, which a submission on another open can wait for.
    pub fn export_sync_file(&self, file: &impl AsRawFd) -> io::Result<OwnedFd> {
        let mut request = [0u8; HANDLE_REQUEST_SIZE];
        put32(&mut request, HANDLE_HANDLE, self.handle);
        put32(&mut request, HANDLE_FLAGS, EXPORT_SYNC_FILE);
        put32(&mut request, HANDLE_FD, u32::MAX);
        ioctl::drm_ioctl(file, call::syncobj_handle_to_fd(), &mut request)?;
        let raw = ioctl::get32(&request, HANDLE_FD) as RawFd;
        if raw < 0 {
            return Err(io::Error::other("the kernel gave no sync file"));
        }
        // SAFETY: the kernel made the descriptor for this call, and nothing else owns it.
        Ok(unsafe { OwnedFd::from_raw_fd(raw) })
    }

    /// A new sync object on `file` holding the fence of a sync file.
    pub fn from_sync_file(file: &impl AsRawFd, sync_file: &impl AsRawFd) -> io::Result<Self> {
        let made = Self::create(file)?;
        let mut request = [0u8; HANDLE_REQUEST_SIZE];
        put32(&mut request, HANDLE_HANDLE, made.handle);
        put32(&mut request, HANDLE_FLAGS, IMPORT_SYNC_FILE);
        put32(&mut request, HANDLE_FD, sync_file.as_raw_fd() as u32);
        if let Err(error) = ioctl::drm_ioctl(file, call::syncobj_fd_to_handle(), &mut request) {
            made.destroy(file);
            return Err(error);
        }
        Ok(made)
    }

    /// How the run behind the fence it holds ended, as the kernel tells it in a sync file's status: 1
    /// for a run that finished, 0 for one still going, and the negative error number the kernel ended
    /// it with otherwise.
    pub fn end_status(&self, file: &impl AsRawFd) -> io::Result<i32> {
        let sync_file = self.export_sync_file(file)?;
        let mut request = [0u8; SYNC_FILE_INFO_SIZE];
        ioctl::drm_ioctl(&sync_file, SYNC_FILE_INFO, &mut request)?;
        Ok(get32(&request, SYNC_FILE_INFO_STATUS) as i32)
    }

    /// Whether the kernel has signalled it, without waiting. A submission must have named it.
    pub fn signalled(&self, file: &impl AsRawFd) -> io::Result<bool> {
        let handles = [self.handle];
        let mut request = [0u8; WAIT_REQUEST_SIZE];
        put64(&mut request, WAIT_HANDLES, handles.as_ptr() as u64);
        put32(&mut request, WAIT_COUNT, 1);
        match ioctl::drm_ioctl(file, call::syncobj_wait(), &mut request) {
            Ok(()) => Ok(true),
            Err(error) if error.raw_os_error() == Some(libc::ETIME) => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub fn destroy(&self, file: &impl AsRawFd) {
        let mut request = [0u8; SYNCOBJ_REQUEST_SIZE];
        put32(&mut request, SYNCOBJ_HANDLE, self.handle);
        let _ = ioctl::drm_ioctl(file, call::syncobj_destroy(), &mut request);
    }
}

/// Waits for an armed eventfd to become readable, up to a timeout. `true` means the work retired.
pub fn wait_eventfd(event: &impl AsRawFd, timeout_ms: i32) -> io::Result<bool> {
    let mut poll = libc::pollfd { fd: event.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    let ready = loop {
        // SAFETY: a single live pollfd for the duration of the call.
        let ready = unsafe { libc::poll(&mut poll, 1, timeout_ms) };
        if ready >= 0 {
            break ready;
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    };
    if ready == 0 {
        return Ok(false);
    }
    let mut counter = [0u8; 8];
    // SAFETY: reading the 8-byte counter an eventfd yields.
    let _ = unsafe { libc::read(event.as_raw_fd(), counter.as_mut_ptr().cast(), counter.len()) };
    Ok(true)
}

/// One indirect buffer: where it is in the file's address space and how many bytes it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ib {
    pub address: u64,
    pub bytes: u32,
}

/// Makes the list of buffers a submission may touch, or replaces the one `existing` names. Making a
/// list only looks the handles up and takes a reference on each buffer; every submission that names it
/// still has the kernel lock and validate each buffer in it. What a submission saves is the lookup
/// and the building of the list from the handles it would otherwise carry.
pub fn make_bo_list(file: &impl AsRawFd, buffers: &[GemHandle], existing: Option<u64>) -> io::Result<u64> {
    let entries = bo_entries(buffers);
    let mut request = [0u8; 24];
    put32(&mut request, LIST_OPERATION, if existing.is_some() { OP_UPDATE } else { OP_CREATE });
    put32(&mut request, LIST_HANDLE, existing.unwrap_or(0) as u32);
    put32(&mut request, LIST_NUMBER, buffers.len() as u32);
    put32(&mut request, LIST_ENTRY_SIZE, BO_ENTRY_SIZE as u32);
    put64(&mut request, LIST_ENTRIES, entries.as_ptr() as u64);
    ioctl::drm_ioctl(file, ioctl::call::bo_list(), &mut request)?;
    let handle = get32(&request, LIST_OUT_HANDLE);
    if handle == 0 {
        return Err(io::Error::other("the kernel gave no handle for the buffer list"));
    }
    Ok(u64::from(handle))
}

/// Lets the kernel free the list `handle` names, and with it its hold on every buffer in it. A list
/// that is merely forgotten keeps those buffers referenced until the file closes.
pub fn destroy_bo_list(file: &impl AsRawFd, handle: u64) -> io::Result<()> {
    let mut request = [0u8; 24];
    put32(&mut request, LIST_OPERATION, OP_DESTROY);
    put32(&mut request, LIST_HANDLE, handle as u32);
    ioctl::drm_ioctl(file, ioctl::call::bo_list(), &mut request)
}

/// Submits one IB and asks the kernel to signal `signal` when it retires. The IB is already at
/// `ib_va` in this file's address space, `ib_bytes` long, for the engine family `ip_type`. No
/// buffers are named, which suits a probe whose buffers are small and just made.
pub fn submit(
    file: &impl AsRawFd,
    context: &Context,
    ip_type: u32,
    ib_va: u64,
    ib_bytes: u32,
    signal: &Syncobj,
) -> io::Result<u64> {
    submit_all(file, context, ip_type, &[Ib { address: ib_va, bytes: ib_bytes }], &[], None, signal)
}

/// Submits IBs to run in order on one engine, and asks the kernel to signal `signal` when the last
/// retires. `buffers`, handles on `file`, are kept in place until then, or `list` names the list
/// already made of them.
pub fn submit_all(
    file: &impl AsRawFd,
    context: &Context,
    ip_type: u32,
    ibs: &[Ib],
    buffers: &[GemHandle],
    list: Option<u64>,
    signal: &Syncobj,
) -> io::Result<u64> {
    submit_after(file, context, ip_type, ibs, buffers, list, &[], signal)
}

/// As [`submit_all`], with the IBs held on the card until every sync object in `after`, on `file`,
/// has been signalled.
#[allow(clippy::too_many_arguments)]
pub fn submit_after(
    file: &impl AsRawFd,
    context: &Context,
    ip_type: u32,
    ibs: &[Ib],
    buffers: &[GemHandle],
    list: Option<u64>,
    after: &[Syncobj],
    signal: &Syncobj,
) -> io::Result<u64> {
    let mut ib_chunks = vec![[0u8; IB_CHUNK_SIZE]; ibs.len()];
    for (chunk, ib) in ib_chunks.iter_mut().zip(ibs) {
        put64(chunk, IB_VA_START, ib.address);
        put32(chunk, IB_BYTES, ib.bytes);
        put32(chunk, IB_IP_TYPE, ip_type);
        put32(chunk, IB_IP_INSTANCE, 0);
        put32(chunk, IB_RING, 0);
    }
    let mut sem_chunk = [0u8; 4];
    put32(&mut sem_chunk, 0, signal.handle());

    let entries = bo_entries(buffers);
    let mut bo_list = [0u8; BO_LIST_SIZE];
    put32(&mut bo_list, BO_LIST_NUMBER, buffers.len() as u32);
    put32(&mut bo_list, BO_LIST_ENTRY_SIZE, BO_ENTRY_SIZE as u32);
    put64(&mut bo_list, BO_LIST_ENTRIES, entries.as_ptr() as u64);

    let mut chunks = vec![[0u8; CHUNK_SIZE]; ibs.len() + 1];
    for (slot, ib_chunk) in chunks.iter_mut().zip(&ib_chunks) {
        write_chunk(slot, CHUNK_ID_IB, IB_CHUNK_SIZE / 4, ib_chunk.as_ptr() as u64);
    }
    write_chunk(&mut chunks[ibs.len()], CHUNK_ID_SYNCOBJ_OUT, 1, sem_chunk.as_ptr() as u64);
    // drm_amdgpu_cs_chunk_sem is one u32 handle per object.
    let waits: Vec<u32> = after.iter().map(Syncobj::handle).collect();
    if !waits.is_empty() {
        let mut slot = [0u8; CHUNK_SIZE];
        write_chunk(&mut slot, CHUNK_ID_SYNCOBJ_IN, waits.len(), waits.as_ptr() as u64);
        chunks.push(slot);
    }
    // The list is named by its handle when one was made, so its handles are only sent the once.
    if list.is_none() && !buffers.is_empty() {
        let mut slot = [0u8; CHUNK_SIZE];
        write_chunk(&mut slot, CHUNK_ID_BO_HANDLES, BO_LIST_SIZE / 4, bo_list.as_ptr() as u64);
        chunks.push(slot);
    }
    let chunk_pointers: Vec<u64> = chunks.iter().map(|chunk| chunk.as_ptr() as u64).collect();

    let mut request = [0u8; CS_REQUEST_SIZE];
    put32(&mut request, CS_CTX_ID, context.id());
    put32(&mut request, CS_BO_LIST, list.unwrap_or(0) as u32);
    put32(&mut request, CS_NUM_CHUNKS, chunk_pointers.len() as u32);
    put64(&mut request, CS_CHUNKS, chunk_pointers.as_ptr() as u64);
    ioctl::drm_ioctl(file, call::cs(), &mut request)?;
    Ok(get64(&request, CS_OUT_HANDLE))
}

/// The list entries for `buffers`, each at the default priority.
fn bo_entries(buffers: &[GemHandle]) -> Vec<u8> {
    let mut entries = vec![0u8; buffers.len() * BO_ENTRY_SIZE];
    for (entry, buffer) in entries.as_chunks_mut::<BO_ENTRY_SIZE>().0.iter_mut().zip(buffers) {
        put32(entry, 0, buffer.0);
    }
    entries
}

fn write_chunk(slot: &mut [u8], id: u32, length_dw: usize, data: u64) {
    put32(slot, CHUNK_ID, id);
    put32(slot, CHUNK_LENGTH_DW, length_dw as u32);
    put64(slot, CHUNK_DATA, data);
}

// SDMA packet building. A write-linear packet copies immediate dwords from the IB into memory at a
// GPU address, which is the smallest thing that proves the engine ran and wrote where it was told.
const SDMA_OPCODE_WRITE_LINEAR: u32 = 0x00000002;
const SDMA_OPCODE_NOP: u32 = 0x00000000;
const SDMA_IB_ALIGNMENT_DWORDS: usize = 8;

/// Builds an SDMA IB that writes `data` to `dst_va`, padded with NOPs to the engine's alignment.
pub fn sdma_write_ib(dst_va: u64, data: &[u32]) -> Vec<u32> {
    let mut ib = Vec::with_capacity(data.len() + 4 + SDMA_IB_ALIGNMENT_DWORDS);
    ib.push(SDMA_OPCODE_WRITE_LINEAR);
    ib.push(dst_va as u32);
    ib.push((dst_va >> 32) as u32);
    ib.push(data.len() as u32 - 1);
    ib.extend_from_slice(data);
    while ib.len() % SDMA_IB_ALIGNMENT_DWORDS != 0 {
        ib.push(SDMA_OPCODE_NOP);
    }
    ib
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sync_file_question_follows_the_kernels_encoding() {
        let read_write = 3 << 30;
        assert_eq!(
            SYNC_FILE_INFO,
            read_write | (SYNC_FILE_INFO_SIZE as u64) << 16 | u64::from(b'>') << 8 | 4
        );
    }

    #[test]
    fn a_write_packet_carries_its_address_and_payload() {
        let ib = sdma_write_ib(0x1_2345_6000, &[0xCAFE_D00D]);
        assert_eq!(ib[0], SDMA_OPCODE_WRITE_LINEAR);
        assert_eq!(ib[1], 0x2345_6000);
        assert_eq!(ib[2], 0x1);
        assert_eq!(ib[3], 0, "count is dwords minus one");
        assert_eq!(ib[4], 0xCAFE_D00D);
        assert_eq!(ib.len() % SDMA_IB_ALIGNMENT_DWORDS, 0, "padded to the engine's alignment");
    }

    #[test]
    fn each_buffer_is_listed_by_its_handle_at_the_default_priority() {
        let entries = bo_entries(&[GemHandle(3), GemHandle(9)]);
        assert_eq!(entries, [3, 0, 0, 0, 0, 0, 0, 0, 9, 0, 0, 0, 0, 0, 0, 0]);
    }
}
