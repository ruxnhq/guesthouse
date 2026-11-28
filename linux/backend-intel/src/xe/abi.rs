//! The xe calls that make things on the card, as bytes: each request's size, where its fields lie,
//! and its ioctl number.
//!
//! Everything comes from the kernel's public uAPI headers, `include/uapi/drm/xe_drm.h` and `drm.h`,
//! which are MIT. The sizes and numbers in the tests are the ones this PC's `/usr/include/drm`
//! headers give (kernel 7.3), printed by a C program and written down here. Only xe's commands 0x00
//! to 0x0a are used, as those are in every kernel that has the driver; anything later is probed.
//!
//! A request is a byte array filled by offset, as [`crate::xe_query`] does it. Nothing here calls
//! the kernel: [`super::device`] does.

use crate::protocol::bytes::{put_dword, put_qword, put_word};

const DIRECTION_SHIFT: u32 = 30;
const SIZE_SHIFT: u32 = 16;
const MAGIC_SHIFT: u32 = 8;
const DRM_MAGIC: u64 = b'd' as u64;
/// Where a DRM driver's own commands begin, after the ones every driver shares.
const DRIVER_COMMANDS: u64 = 0x40;

/// Which way a request's bytes travel, as the kernel's `_IOC` packs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Write = 1,
    ReadWrite = 3,
}

const fn number(direction: Direction, command: u64, size: usize) -> u64 {
    (direction as u64) << DIRECTION_SHIFT | (size as u64) << SIZE_SHIFT | DRM_MAGIC << MAGIC_SHIFT | command
}

/// xe's own command numbers, `DRM_XE_*`.
pub mod command {
    pub const GEM_CREATE: u64 = 0x01;
    pub const GEM_MMAP_OFFSET: u64 = 0x02;
    pub const VM_CREATE: u64 = 0x03;
    pub const VM_DESTROY: u64 = 0x04;
    pub const VM_BIND: u64 = 0x05;
    pub const EXEC_QUEUE_CREATE: u64 = 0x06;
    pub const EXEC_QUEUE_DESTROY: u64 = 0x07;
    pub const EXEC_QUEUE_GET_PROPERTY: u64 = 0x08;
    pub const EXEC: u64 = 0x09;
    /// Newer than the rest, so only probed: see [`super::VM_GET_PROPERTY`].
    pub const VM_GET_PROPERTY: u64 = 0x0f;
}

/// The commands every DRM driver shares, from `drm.h`.
mod shared_command {
    pub const GEM_CLOSE: u64 = 0x09;
    pub const PRIME_HANDLE_TO_FD: u64 = 0x2d;
    pub const SYNCOBJ_CREATE: u64 = 0xbf;
    pub const SYNCOBJ_DESTROY: u64 = 0xc0;
    pub const SYNCOBJ_TIMELINE_WAIT: u64 = 0xca;
    /// `DRM_IOCTL_SYNCOBJ_EVENTFD`, from Linux v6.6.
    pub const SYNCOBJ_EVENTFD: u64 = 0xcf;
}

pub const GEM_CREATE_SIZE: usize = 56;
pub const GEM_MMAP_OFFSET_SIZE: usize = 40;
pub const VM_CREATE_SIZE: usize = 32;
pub const VM_DESTROY_SIZE: usize = 24;
pub const VM_BIND_SIZE: usize = 136;
pub const VM_GET_PROPERTY_SIZE: usize = 56;
pub const EXEC_QUEUE_CREATE_SIZE: usize = 48;
pub const EXEC_QUEUE_DESTROY_SIZE: usize = 24;
pub const EXEC_QUEUE_GET_PROPERTY_SIZE: usize = 40;
pub const EXEC_SIZE: usize = 56;
/// `struct drm_xe_sync`, which an exec or a bind points at an array of.
pub const SYNC_SIZE: usize = 48;
/// `struct drm_xe_engine_class_instance`.
pub const ENGINE_INSTANCE_SIZE: usize = 8;
/// `struct drm_xe_ext_set_property`, the extension a queue's creation points at to set a property.
pub const EXT_SET_PROPERTY_SIZE: usize = 48;
/// `struct drm_xe_query_engine_cycles`.
pub const ENGINE_CYCLES_SIZE: usize = 40;
pub const GEM_CLOSE_SIZE: usize = 8;
pub const PRIME_HANDLE_SIZE: usize = 12;
pub const SYNCOBJ_CREATE_SIZE: usize = 8;
pub const SYNCOBJ_DESTROY_SIZE: usize = 8;
pub const SYNCOBJ_TIMELINE_WAIT_SIZE: usize = 48;
/// `struct drm_syncobj_eventfd`: handle, flags, point, fd and padding.
pub const SYNCOBJ_EVENTFD_SIZE: usize = 24;

const fn xe(direction: Direction, command: u64, size: usize) -> u64 {
    number(direction, DRIVER_COMMANDS + command, size)
}

pub const GEM_CREATE: u64 = xe(Direction::ReadWrite, command::GEM_CREATE, GEM_CREATE_SIZE);
pub const GEM_MMAP_OFFSET: u64 = xe(Direction::ReadWrite, command::GEM_MMAP_OFFSET, GEM_MMAP_OFFSET_SIZE);
pub const VM_CREATE: u64 = xe(Direction::ReadWrite, command::VM_CREATE, VM_CREATE_SIZE);
pub const VM_DESTROY: u64 = xe(Direction::Write, command::VM_DESTROY, VM_DESTROY_SIZE);
pub const VM_BIND: u64 = xe(Direction::Write, command::VM_BIND, VM_BIND_SIZE);
pub const VM_GET_PROPERTY: u64 = xe(Direction::ReadWrite, command::VM_GET_PROPERTY, VM_GET_PROPERTY_SIZE);
pub const EXEC_QUEUE_CREATE: u64 =
    xe(Direction::ReadWrite, command::EXEC_QUEUE_CREATE, EXEC_QUEUE_CREATE_SIZE);
pub const EXEC_QUEUE_DESTROY: u64 =
    xe(Direction::Write, command::EXEC_QUEUE_DESTROY, EXEC_QUEUE_DESTROY_SIZE);
pub const EXEC_QUEUE_GET_PROPERTY: u64 =
    xe(Direction::ReadWrite, command::EXEC_QUEUE_GET_PROPERTY, EXEC_QUEUE_GET_PROPERTY_SIZE);
pub const EXEC: u64 = xe(Direction::Write, command::EXEC, EXEC_SIZE);
pub const GEM_CLOSE: u64 = number(Direction::Write, shared_command::GEM_CLOSE, GEM_CLOSE_SIZE);
pub const PRIME_HANDLE_TO_FD: u64 =
    number(Direction::ReadWrite, shared_command::PRIME_HANDLE_TO_FD, PRIME_HANDLE_SIZE);
pub const SYNCOBJ_CREATE: u64 =
    number(Direction::ReadWrite, shared_command::SYNCOBJ_CREATE, SYNCOBJ_CREATE_SIZE);
pub const SYNCOBJ_DESTROY: u64 =
    number(Direction::ReadWrite, shared_command::SYNCOBJ_DESTROY, SYNCOBJ_DESTROY_SIZE);
pub const SYNCOBJ_TIMELINE_WAIT: u64 =
    number(Direction::ReadWrite, shared_command::SYNCOBJ_TIMELINE_WAIT, SYNCOBJ_TIMELINE_WAIT_SIZE);
pub const SYNCOBJ_EVENTFD: u64 =
    number(Direction::ReadWrite, shared_command::SYNCOBJ_EVENTFD, SYNCOBJ_EVENTFD_SIZE);

/// `DRM_XE_VM_CREATE_FLAG_SCRATCH_PAGE`: unmapped addresses read zero and swallow writes.
pub const VM_SCRATCH_PAGE: u32 = 1 << 0;
/// `DRM_XE_VM_CREATE_FLAG_LR_MODE`: work in this VM may run for as long as it likes. Such a VM
/// takes no sync object signals, only user fences.
pub const VM_LONG_RUNNING: u32 = 1 << 1;

/// `DRM_XE_GEM_CPU_CACHING_*`.
pub const CPU_CACHING_WRITE_BACK: u16 = 1;
pub const CPU_CACHING_WRITE_COMBINED: u16 = 2;

/// `DRM_XE_GEM_CREATE_FLAG_*`.
pub const BUFFER_SCANOUT: u32 = 1 << 1;
pub const BUFFER_NEEDS_VISIBLE_VRAM: u32 = 1 << 2;

/// `DRM_XE_VM_BIND_OP_*`.
pub const BIND_MAP: u32 = 0;
pub const BIND_UNMAP: u32 = 1;
pub const BIND_MAP_USERPTR: u32 = 2;

/// `DRM_XE_SYNC_TYPE_*` and the one flag, `DRM_XE_SYNC_FLAG_SIGNAL`.
pub const SYNC_TIMELINE_SYNCOBJ: u32 = 1;
pub const SYNC_USER_FENCE: u32 = 2;
pub const SYNC_SIGNAL: u32 = 1 << 0;

/// `DRM_XE_EXEC_QUEUE_GET_PROPERTY_BAN`.
pub const QUEUE_PROPERTY_BAN: u32 = 0;

/// `DRM_XE_EXEC_QUEUE_EXTENSION_SET_PROPERTY`, the one extension a queue's creation takes.
pub const QUEUE_EXTENSION_SET_PROPERTY: u32 = 0;
/// `DRM_XE_EXEC_QUEUE_SET_DISABLE_STATE_CACHE_PERF_FIX`: the queue's render colour cache is keyed
/// by binding table and binding index rather than by the index alone. xe takes it for a render
/// queue only, from Linux v7.1.
pub const QUEUE_PROPERTY_CACHE_KEYED_BY_TABLE: u32 = 6;

/// How a render queue's colour cache tells one surface from another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Keying {
    /// By binding index alone, which is how xe makes a queue unless asked otherwise. A binding
    /// that comes to name another surface then needs a render target cache flush.
    #[default]
    BindingIndex,
    /// By binding table and index, [`QUEUE_PROPERTY_CACHE_KEYED_BY_TABLE`]. Render queues only.
    BindingTableAndIndex,
}

/// `DRM_XE_VM_GET_PROPERTY_FAULTS`.
pub const VM_PROPERTY_FAULTS: u32 = 0;
/// `struct xe_vm_fault`, one entry of a VM's fault list.
pub const VM_FAULT_SIZE: usize = 48;

/// `FAULT_ACCESS_TYPE_*`: what the faulting access was doing.
pub const FAULT_ACCESS_TYPE_READ: u8 = 0;
pub const FAULT_ACCESS_TYPE_WRITE: u8 = 1;
pub const FAULT_ACCESS_TYPE_ATOMIC: u8 = 2;
/// `FAULT_TYPE_*`: why it faulted.
pub const FAULT_TYPE_NOT_PRESENT: u8 = 0;
pub const FAULT_TYPE_WRITE_ACCESS: u8 = 1;
pub const FAULT_TYPE_ATOMIC_ACCESS: u8 = 2;
/// `FAULT_LEVEL_*`: the level of the page table where the walk found nothing.
pub const FAULT_LEVEL_PTE: u8 = 0;
pub const FAULT_LEVEL_PDE: u8 = 1;
pub const FAULT_LEVEL_PDP: u8 = 2;
pub const FAULT_LEVEL_PML4: u8 = 3;
pub const FAULT_LEVEL_PML5: u8 = 4;

/// `DRM_XE_DEVICE_QUERY_ENGINE_CYCLES`.
pub const QUERY_ENGINE_CYCLES: u32 = 6;

/// `DRM_SYNCOBJ_WAIT_FLAGS_WAIT_FOR_SUBMIT`. Without it a wait on a point nothing has been attached
/// to yet is refused with EINVAL, rather than waited for.
pub const WAIT_FOR_SUBMIT: u32 = 1 << 1;

/// `DRM_CLOEXEC` and `DRM_RDWR`, for an exported buffer's file.
pub const EXPORT_FLAGS: u32 = libc::O_CLOEXEC as u32 | libc::O_RDWR as u32;

/// A buffer to make, `struct drm_xe_gem_create`. It belongs to no VM, so it can be bound in
/// several and exported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewBuffer {
    pub size: u64,
    /// One bit per memory region instance it may live in.
    pub placement: u32,
    pub flags: u32,
    pub cpu_caching: u16,
}

impl NewBuffer {
    const SIZE: usize = 8;
    const PLACEMENT: usize = 16;
    const FLAGS: usize = 20;
    pub const HANDLE: usize = 28;
    const CPU_CACHING: usize = 32;

    pub fn request(&self) -> [u8; GEM_CREATE_SIZE] {
        let mut request = [0; GEM_CREATE_SIZE];
        put_qword(&mut request, Self::SIZE, self.size);
        put_dword(&mut request, Self::PLACEMENT, self.placement);
        put_dword(&mut request, Self::FLAGS, self.flags);
        put_word(&mut request, Self::CPU_CACHING, self.cpu_caching);
        request
    }
}

/// `struct drm_xe_gem_mmap_offset`: the offset to mmap the node at for a buffer.
pub mod mmap_offset {
    use super::*;
    const HANDLE: usize = 8;
    pub const OFFSET: usize = 16;

    pub fn request(handle: u32) -> [u8; GEM_MMAP_OFFSET_SIZE] {
        let mut request = [0; GEM_MMAP_OFFSET_SIZE];
        put_dword(&mut request, HANDLE, handle);
        request
    }
}

/// `struct drm_xe_vm_create` and `struct drm_xe_vm_destroy`.
pub mod vm {
    use super::*;
    const FLAGS: usize = 8;
    pub const CREATED_ID: usize = 12;
    const DESTROYED_ID: usize = 0;

    pub fn create(flags: u32) -> [u8; VM_CREATE_SIZE] {
        let mut request = [0; VM_CREATE_SIZE];
        put_dword(&mut request, FLAGS, flags);
        request
    }

    pub fn destroy(vm: u32) -> [u8; VM_DESTROY_SIZE] {
        let mut request = [0; VM_DESTROY_SIZE];
        put_dword(&mut request, DESTROYED_ID, vm);
        request
    }
}

/// One change to a VM's mappings, `struct drm_xe_vm_bind_op`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BindOp {
    /// [`BIND_MAP`], [`BIND_UNMAP`] or [`BIND_MAP_USERPTR`].
    pub op: u32,
    /// The buffer's handle for a map, and zero otherwise.
    pub buffer: u32,
    /// Bytes into the buffer for a map, and the host address for a userptr.
    pub offset_or_host_address: u64,
    pub length: u64,
    pub address: u64,
    pub pat_index: u16,
}

impl BindOp {
    const BUFFER: usize = 8;
    const PAT_INDEX: usize = 12;
    const OFFSET: usize = 16;
    const LENGTH: usize = 24;
    const ADDRESS: usize = 32;
    const OP: usize = 40;

    fn write(&self, op: &mut [u8]) {
        put_dword(op, Self::BUFFER, self.buffer);
        put_word(op, Self::PAT_INDEX, self.pat_index);
        put_qword(op, Self::OFFSET, self.offset_or_host_address);
        put_qword(op, Self::LENGTH, self.length);
        put_qword(op, Self::ADDRESS, self.address);
        put_dword(op, Self::OP, self.op);
    }
}

/// `struct drm_xe_vm_bind` carrying one op, which lies inside the request.
pub mod bind {
    use super::*;
    const VM: usize = 8;
    const COUNT: usize = 20;
    const OP: usize = 24;
    const SYNC_COUNT: usize = 108;
    const SYNCS: usize = 112;

    /// `syncs` is the address of `sync_count` [`Sync`] records, or zero for none.
    pub fn request(vm: u32, op: &BindOp, syncs: u64, sync_count: u32) -> [u8; VM_BIND_SIZE] {
        let mut request = [0; VM_BIND_SIZE];
        put_dword(&mut request, VM, vm);
        put_dword(&mut request, COUNT, 1);
        op.write(&mut request[OP..]);
        put_dword(&mut request, SYNC_COUNT, sync_count);
        put_qword(&mut request, SYNCS, syncs);
        request
    }
}

/// An engine, `struct drm_xe_engine_class_instance`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EngineInstance {
    pub class: u16,
    pub instance: u16,
    pub gt: u16,
}

impl EngineInstance {
    pub fn bytes(&self) -> [u8; ENGINE_INSTANCE_SIZE] {
        let mut bytes = [0; ENGINE_INSTANCE_SIZE];
        put_word(&mut bytes, 0, self.class);
        put_word(&mut bytes, 2, self.instance);
        put_word(&mut bytes, 4, self.gt);
        bytes
    }
}

/// `struct drm_xe_exec_queue_create`, `_destroy` and `_get_property`.
pub mod queue {
    use super::*;
    const EXTENSIONS: usize = 0;
    const WIDTH: usize = 8;
    const PLACEMENTS: usize = 10;
    const VM: usize = 12;
    pub const CREATED_ID: usize = 20;
    const INSTANCES: usize = 24;
    const DESTROYED_ID: usize = 0;
    const PROPERTY_QUEUE: usize = 8;
    const PROPERTY: usize = 12;
    pub const PROPERTY_VALUE: usize = 16;

    /// Inside `struct drm_xe_ext_set_property`: the base's next extension and name, then the
    /// property and its value.
    const NEXT_EXTENSION: usize = 0;
    const EXTENSION_NAME: usize = 8;
    const EXTENSION_PROPERTY: usize = 16;
    const EXTENSION_VALUE: usize = 24;

    /// A queue of one engine that takes one batch at a time. `instances` is the address of that
    /// engine's [`EngineInstance::bytes`], and `extensions` that of the first extension, or zero.
    pub fn create(vm: u32, instances: u64, extensions: u64) -> [u8; EXEC_QUEUE_CREATE_SIZE] {
        let mut request = [0; EXEC_QUEUE_CREATE_SIZE];
        put_qword(&mut request, EXTENSIONS, extensions);
        put_word(&mut request, WIDTH, BATCHES_PER_EXEC);
        put_word(&mut request, PLACEMENTS, 1);
        put_dword(&mut request, VM, vm);
        put_qword(&mut request, INSTANCES, instances);
        request
    }

    /// The extension that keys a render queue's colour cache by binding table and index, the
    /// last in its chain.
    pub fn keyed_by_table() -> [u8; EXT_SET_PROPERTY_SIZE] {
        let mut extension = [0; EXT_SET_PROPERTY_SIZE];
        put_qword(&mut extension, NEXT_EXTENSION, 0);
        put_dword(&mut extension, EXTENSION_NAME, QUEUE_EXTENSION_SET_PROPERTY);
        put_dword(&mut extension, EXTENSION_PROPERTY, QUEUE_PROPERTY_CACHE_KEYED_BY_TABLE);
        put_qword(&mut extension, EXTENSION_VALUE, 1);
        extension
    }

    pub fn destroy(queue: u32) -> [u8; EXEC_QUEUE_DESTROY_SIZE] {
        let mut request = [0; EXEC_QUEUE_DESTROY_SIZE];
        put_dword(&mut request, DESTROYED_ID, queue);
        request
    }

    pub fn property(queue: u32, property: u32) -> [u8; EXEC_QUEUE_GET_PROPERTY_SIZE] {
        let mut request = [0; EXEC_QUEUE_GET_PROPERTY_SIZE];
        put_dword(&mut request, PROPERTY_QUEUE, queue);
        put_dword(&mut request, PROPERTY, property);
        request
    }
}

/// The batches one exec carries, which xe wants equal to the queue's width.
pub const BATCHES_PER_EXEC: u16 = 1;

/// Something an exec or a bind waits for or signals, `struct drm_xe_sync`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sync {
    /// Wait for this point of a timeline.
    After { timeline: u32, point: u64 },
    /// Signal this point of a timeline when done.
    Signal { timeline: u32, point: u64 },
    /// Have the card write `value` at this GPU address when the batch is through.
    UserFence { address: u64, value: u64 },
}

impl Sync {
    const KIND: usize = 8;
    const FLAGS: usize = 12;
    const HANDLE_OR_ADDRESS: usize = 16;
    const VALUE: usize = 24;

    pub fn bytes(&self) -> [u8; SYNC_SIZE] {
        let mut bytes = [0; SYNC_SIZE];
        let (kind, flags, target, value) = match *self {
            Self::After { timeline, point } => (SYNC_TIMELINE_SYNCOBJ, 0, u64::from(timeline), point),
            Self::Signal { timeline, point } => {
                (SYNC_TIMELINE_SYNCOBJ, SYNC_SIGNAL, u64::from(timeline), point)
            }
            Self::UserFence { address, value } => (SYNC_USER_FENCE, SYNC_SIGNAL, address, value),
        };
        put_dword(&mut bytes, Self::KIND, kind);
        put_dword(&mut bytes, Self::FLAGS, flags);
        put_qword(&mut bytes, Self::HANDLE_OR_ADDRESS, target);
        put_qword(&mut bytes, Self::VALUE, value);
        bytes
    }

    /// The records one after another, as the kernel reads them.
    pub fn array(syncs: &[Sync]) -> Vec<u8> {
        syncs.iter().flat_map(Sync::bytes).collect()
    }
}

/// `struct drm_xe_exec`.
pub mod exec {
    use super::*;
    const QUEUE: usize = 8;
    const SYNC_COUNT: usize = 12;
    const SYNCS: usize = 16;
    const ADDRESS: usize = 24;
    const BATCH_COUNT: usize = 32;

    pub fn request(queue: u32, batch: u64, syncs: u64, sync_count: u32) -> [u8; EXEC_SIZE] {
        let mut request = [0; EXEC_SIZE];
        put_dword(&mut request, QUEUE, queue);
        put_dword(&mut request, SYNC_COUNT, sync_count);
        put_qword(&mut request, SYNCS, syncs);
        put_qword(&mut request, ADDRESS, batch);
        put_word(&mut request, BATCH_COUNT, BATCHES_PER_EXEC);
        request
    }
}

/// `struct drm_syncobj_create`, `_destroy` and `_timeline_wait`, and `struct drm_gem_close`.
pub mod syncobj {
    use super::*;
    pub const HANDLE: usize = 0;
    const WAIT_HANDLES: usize = 0;
    const WAIT_POINTS: usize = 8;
    const WAIT_UNTIL: usize = 16;
    const WAIT_COUNT: usize = 24;
    const WAIT_FLAGS: usize = 28;

    pub fn create() -> [u8; SYNCOBJ_CREATE_SIZE] {
        [0; SYNCOBJ_CREATE_SIZE]
    }

    pub fn destroy(handle: u32) -> [u8; SYNCOBJ_DESTROY_SIZE] {
        let mut request = [0; SYNCOBJ_DESTROY_SIZE];
        put_dword(&mut request, HANDLE, handle);
        request
    }

    /// Has the kernel add one to an eventfd when a point is signalled, once, or at once if it
    /// already is. A point nothing is attached to yet is waited for: with no flags the kernel
    /// looks again each time the sync object takes a fence (`drm_syncobj_eventfd_ioctl` in
    /// `drm_syncobj.c`).
    pub fn eventfd(handle: u32, point: u64, fd: i32) -> [u8; SYNCOBJ_EVENTFD_SIZE] {
        const FLAGS: usize = 4;
        const POINT: usize = 8;
        const FD: usize = 16;
        let mut request = [0; SYNCOBJ_EVENTFD_SIZE];
        put_dword(&mut request, HANDLE, handle);
        put_dword(&mut request, FLAGS, 0);
        put_qword(&mut request, POINT, point);
        put_dword(&mut request, FD, fd as u32);
        request
    }

    /// A wait for one point. `handle` and `point` are the addresses of the two values, and
    /// `until` is a time on the monotonic clock in nanoseconds, with zero for a look that
    /// doesn't wait.
    pub fn wait(handle: u64, point: u64, until: i64) -> [u8; SYNCOBJ_TIMELINE_WAIT_SIZE] {
        let mut request = [0; SYNCOBJ_TIMELINE_WAIT_SIZE];
        put_qword(&mut request, WAIT_HANDLES, handle);
        put_qword(&mut request, WAIT_POINTS, point);
        put_qword(&mut request, WAIT_UNTIL, until as u64);
        put_dword(&mut request, WAIT_COUNT, 1);
        put_dword(&mut request, WAIT_FLAGS, WAIT_FOR_SUBMIT);
        request
    }
}

pub fn close_buffer(handle: u32) -> [u8; GEM_CLOSE_SIZE] {
    let mut request = [0; GEM_CLOSE_SIZE];
    put_dword(&mut request, 0, handle);
    request
}

/// `struct drm_prime_handle`, for handing a buffer out as a file.
pub mod export {
    use super::*;
    const HANDLE: usize = 0;
    const FLAGS: usize = 4;
    pub const FILE: usize = 8;

    pub fn request(handle: u32) -> [u8; PRIME_HANDLE_SIZE] {
        let mut request = [0; PRIME_HANDLE_SIZE];
        put_dword(&mut request, HANDLE, handle);
        put_dword(&mut request, FLAGS, EXPORT_FLAGS);
        request
    }
}

/// `struct drm_xe_query_engine_cycles`: an engine's cycle counter and the CPU's clock, read
/// together.
pub mod engine_cycles {
    use super::*;
    const ENGINE: usize = 0;
    const CLOCK: usize = 8;
    pub const WIDTH: usize = 12;
    pub const CYCLES: usize = 16;
    pub const CPU_TIME: usize = 24;

    pub fn request(engine: EngineInstance) -> [u8; ENGINE_CYCLES_SIZE] {
        let mut request = [0; ENGINE_CYCLES_SIZE];
        request[ENGINE..ENGINE + ENGINE_INSTANCE_SIZE].copy_from_slice(&engine.bytes());
        put_dword(&mut request, CLOCK, libc::CLOCK_MONOTONIC as u32);
        request
    }
}

/// One fault xe saved for a VM, `struct xe_vm_fault`. See [`crate::xe::device::Xe::vm_faults`] for
/// what the list does and does not hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmFault {
    /// The faulting address, canonical, rounded down to `precision`.
    pub address: u64,
    /// How many bytes the address is exact to.
    pub precision: u32,
    pub access: u8,
    pub kind: u8,
    pub level: u8,
}

impl VmFault {
    const PRECISION: usize = 8;
    const ACCESS: usize = 12;
    const KIND: usize = 13;
    const LEVEL: usize = 14;

    /// The entries of an answer, a whole number of `struct xe_vm_fault`.
    pub fn parse(answer: &[u8]) -> Vec<Self> {
        answer
            .as_chunks::<VM_FAULT_SIZE>()
            .0
            .iter()
            .map(|entry| Self {
                address: u64::from_le_bytes(entry[..8].try_into().expect("eight bytes")),
                precision: u32::from_le_bytes(entry[Self::PRECISION..Self::ACCESS].try_into().expect("four")),
                access: entry[Self::ACCESS],
                kind: entry[Self::KIND],
                level: entry[Self::LEVEL],
            })
            .collect()
    }

    /// The bytes of an entry as the kernel writes it. Only the stand-in needs them.
    #[cfg(any(test, feature = "lab-fake-card"))]
    pub fn bytes(&self) -> [u8; VM_FAULT_SIZE] {
        let mut entry = [0; VM_FAULT_SIZE];
        put_qword(&mut entry, 0, self.address);
        put_dword(&mut entry, Self::PRECISION, self.precision);
        entry[Self::ACCESS] = self.access;
        entry[Self::KIND] = self.kind;
        entry[Self::LEVEL] = self.level;
        entry
    }
}

impl std::fmt::Display for VmFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let access = match self.access {
            FAULT_ACCESS_TYPE_READ => "read",
            FAULT_ACCESS_TYPE_WRITE => "write",
            FAULT_ACCESS_TYPE_ATOMIC => "atomic",
            other => return write!(f, "{:#x} (access {other}, unnamed)", self.address),
        };
        let kind = match self.kind {
            FAULT_TYPE_NOT_PRESENT => "nothing mapped",
            FAULT_TYPE_WRITE_ACCESS => "write to a read-only page",
            FAULT_TYPE_ATOMIC_ACCESS => "atomic on a page that allows none",
            _ => "an unnamed fault type",
        };
        let level = match self.level {
            FAULT_LEVEL_PTE => "page",
            FAULT_LEVEL_PDE => "directory",
            FAULT_LEVEL_PDP => "directory pointer",
            FAULT_LEVEL_PML4 => "level 4",
            FAULT_LEVEL_PML5 => "level 5",
            _ => "an unnamed level",
        };
        write!(
            f,
            "{:#x} (exact to {:#x} bytes) on a {access}: {kind}, found at the {level} level",
            self.address, self.precision
        )
    }
}

/// `struct drm_xe_vm_get_property`.
pub mod vm_property {
    use super::*;
    const VM: usize = 8;
    const PROPERTY: usize = 12;
    const SIZE: usize = 16;
    const DATA: usize = 24;
    /// A VM id no file has. A kernel with the call answers ENOENT for it, and one without says the
    /// call itself is unknown, which is how Mesa tells them apart.
    pub const NO_SUCH_VM: u32 = u32::MAX;

    pub fn probe() -> [u8; VM_GET_PROPERTY_SIZE] {
        let mut request = [0; VM_GET_PROPERTY_SIZE];
        put_dword(&mut request, VM, NO_SUCH_VM);
        request
    }

    /// Asks for a VM's fault list. With `size` zero the kernel says in `size` how many bytes the
    /// list takes; otherwise it copies that many to `data`.
    pub fn faults(vm: u32, size: u32, data: u64) -> [u8; VM_GET_PROPERTY_SIZE] {
        let mut request = [0; VM_GET_PROPERTY_SIZE];
        put_dword(&mut request, VM, vm);
        put_dword(&mut request, PROPERTY, VM_PROPERTY_FAULTS);
        put_dword(&mut request, SIZE, size);
        put_qword(&mut request, DATA, data);
        request
    }

    /// The size the kernel wrote into a request.
    pub fn size_of_answer(request: &[u8]) -> u32 {
        crate::protocol::bytes::dword(request, SIZE).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::bytes::{dword, qword, word};

    /// Each number as this PC's headers define its `DRM_IOCTL_*`, printed from C.
    #[test]
    fn the_ioctl_numbers_are_the_headers() {
        assert_eq!(GEM_CREATE, 0xC038_6441);
        assert_eq!(GEM_MMAP_OFFSET, 0xC028_6442);
        assert_eq!(VM_CREATE, 0xC020_6443);
        assert_eq!(VM_DESTROY, 0x4018_6444);
        assert_eq!(VM_BIND, 0x4088_6445);
        assert_eq!(EXEC_QUEUE_CREATE, 0xC030_6446);
        assert_eq!(EXEC_QUEUE_DESTROY, 0x4018_6447);
        assert_eq!(EXEC_QUEUE_GET_PROPERTY, 0xC028_6448);
        assert_eq!(EXEC, 0x4038_6449);
        assert_eq!(VM_GET_PROPERTY, 0xC038_644F);
        assert_eq!(GEM_CLOSE, 0x4008_6409);
        assert_eq!(PRIME_HANDLE_TO_FD, 0xC00C_642D);
        assert_eq!(SYNCOBJ_CREATE, 0xC008_64BF);
        assert_eq!(SYNCOBJ_DESTROY, 0xC008_64C0);
        assert_eq!(SYNCOBJ_TIMELINE_WAIT, 0xC030_64CA);
        assert_eq!(SYNCOBJ_EVENTFD, 0xC018_64CF);
    }

    /// The flag values the same C program printed.
    #[test]
    fn the_flags_are_the_headers() {
        assert_eq!((VM_SCRATCH_PAGE, VM_LONG_RUNNING), (0x1, 0x2));
        assert_eq!((BUFFER_SCANOUT, BUFFER_NEEDS_VISIBLE_VRAM), (0x2, 0x4));
        assert_eq!((SYNC_SIGNAL, WAIT_FOR_SUBMIT), (0x1, 0x2));
        assert_eq!(EXPORT_FLAGS, 0x80002, "DRM_CLOEXEC | DRM_RDWR");
    }

    /// `sizeof` of each struct, printed from C against the same headers. The numbers above carry
    /// most of them; these two are only pointed at.
    #[test]
    fn the_records_a_request_points_at_have_the_headers_sizes() {
        assert_eq!(Sync::UserFence { address: 0, value: 0 }.bytes().len(), 48);
        assert_eq!(EngineInstance { class: 0, instance: 0, gt: 0 }.bytes().len(), 8);
        assert_eq!(engine_cycles::request(EngineInstance { class: 0, instance: 0, gt: 0 }).len(), 40);
        assert_eq!(queue::keyed_by_table().len(), 48);
    }

    /// The command numbers, read from the header itself where a PC has it. A build image may not.
    #[test]
    fn the_command_numbers_are_the_ones_in_this_pcs_header() {
        let Ok(header) = std::fs::read_to_string("/usr/include/drm/xe_drm.h") else { return };
        let defined = |name: &str| {
            let line = header.lines().find(|line| {
                let mut words = line.split_whitespace();
                words.next() == Some("#define") && words.next() == Some(name)
            })?;
            let value = line.split_whitespace().nth(2)?.trim_start_matches("0x");
            u64::from_str_radix(value, 16).ok()
        };
        for (name, ours) in [
            ("DRM_XE_GEM_CREATE", command::GEM_CREATE),
            ("DRM_XE_GEM_MMAP_OFFSET", command::GEM_MMAP_OFFSET),
            ("DRM_XE_VM_CREATE", command::VM_CREATE),
            ("DRM_XE_VM_DESTROY", command::VM_DESTROY),
            ("DRM_XE_VM_BIND", command::VM_BIND),
            ("DRM_XE_EXEC_QUEUE_CREATE", command::EXEC_QUEUE_CREATE),
            ("DRM_XE_EXEC_QUEUE_DESTROY", command::EXEC_QUEUE_DESTROY),
            ("DRM_XE_EXEC_QUEUE_GET_PROPERTY", command::EXEC_QUEUE_GET_PROPERTY),
            ("DRM_XE_EXEC", command::EXEC),
            ("DRM_XE_VM_BIND_OP_MAP", u64::from(BIND_MAP)),
            ("DRM_XE_VM_BIND_OP_UNMAP", u64::from(BIND_UNMAP)),
            ("DRM_XE_VM_BIND_OP_MAP_USERPTR", u64::from(BIND_MAP_USERPTR)),
            ("DRM_XE_SYNC_TYPE_TIMELINE_SYNCOBJ", u64::from(SYNC_TIMELINE_SYNCOBJ)),
            ("DRM_XE_SYNC_TYPE_USER_FENCE", u64::from(SYNC_USER_FENCE)),
            ("DRM_XE_GEM_CPU_CACHING_WB", u64::from(CPU_CACHING_WRITE_BACK)),
            ("DRM_XE_GEM_CPU_CACHING_WC", u64::from(CPU_CACHING_WRITE_COMBINED)),
            ("DRM_XE_DEVICE_QUERY_ENGINE_CYCLES", u64::from(QUERY_ENGINE_CYCLES)),
            ("DRM_XE_EXEC_QUEUE_GET_PROPERTY_BAN", u64::from(QUEUE_PROPERTY_BAN)),
            ("DRM_XE_EXEC_QUEUE_EXTENSION_SET_PROPERTY", u64::from(QUEUE_EXTENSION_SET_PROPERTY)),
            (
                "DRM_XE_EXEC_QUEUE_SET_DISABLE_STATE_CACHE_PERF_FIX",
                u64::from(QUEUE_PROPERTY_CACHE_KEYED_BY_TABLE),
            ),
        ] {
            // A header too old to name one is no reason to fail: the numbers above are pinned.
            if let Some(theirs) = defined(name) {
                assert_eq!(theirs, ours, "{name}");
            }
        }
    }

    #[test]
    fn a_buffer_request_carries_its_size_placement_and_caching_and_no_vm() {
        let buffer = NewBuffer {
            size: 0x1_0000,
            placement: 0b11,
            flags: BUFFER_SCANOUT,
            cpu_caching: CPU_CACHING_WRITE_COMBINED,
        };
        let request = buffer.request();
        assert_eq!(qword(&request, 8), Some(0x1_0000));
        assert_eq!((dword(&request, 16), dword(&request, 20)), (Some(0b11), Some(2)));
        assert_eq!(dword(&request, 24), Some(0), "vm_id: bound in any VM, and exportable");
        assert_eq!(word(&request, 32), Some(2));
    }

    #[test]
    fn a_bind_request_carries_one_op_inside_it() {
        let op = BindOp {
            op: BIND_MAP_USERPTR,
            buffer: 0,
            offset_or_host_address: 0x7f00_0000_1000,
            length: 0x20_0000,
            address: 0x400_0000,
            pat_index: 1,
        };
        let request = bind::request(7, &op, 0xABCD_0000, 1);
        assert_eq!((dword(&request, 8), dword(&request, 20)), (Some(7), Some(1)));
        // The op starts at 24: obj +8, pat_index +12, obj_offset or userptr +16, range +24,
        // addr +32, op +40.
        assert_eq!((dword(&request, 32), word(&request, 36)), (Some(0), Some(1)));
        assert_eq!(qword(&request, 40), Some(0x7f00_0000_1000));
        assert_eq!((qword(&request, 48), qword(&request, 56)), (Some(0x20_0000), Some(0x400_0000)));
        assert_eq!(dword(&request, 64), Some(2));
        assert_eq!((dword(&request, 108), qword(&request, 112)), (Some(1), Some(0xABCD_0000)));
    }

    #[test]
    fn a_user_fence_is_a_signal_at_a_gpu_address() {
        let fence = Sync::UserFence { address: 0x7FFF_FFE0_0000, value: 9 }.bytes();
        assert_eq!((dword(&fence, 8), dword(&fence, 12)), (Some(2), Some(1)));
        assert_eq!((qword(&fence, 16), qword(&fence, 24)), (Some(0x7FFF_FFE0_0000), Some(9)));
        let after = Sync::After { timeline: 5, point: 3 }.bytes();
        assert_eq!((dword(&after, 8), dword(&after, 12)), (Some(1), Some(0)), "a wait, not a signal");
        assert_eq!((qword(&after, 16), qword(&after, 24)), (Some(5), Some(3)));
        let signal = Sync::Signal { timeline: 5, point: 4 };
        assert_eq!(dword(&signal.bytes(), 12), Some(1));
        assert_eq!(Sync::array(&[Sync::After { timeline: 5, point: 3 }, signal]).len(), 2 * SYNC_SIZE);
    }

    #[test]
    fn an_exec_names_its_queue_batch_and_syncs() {
        let request = exec::request(3, 0x1_4000_0000, 0x5000, 3);
        assert_eq!((dword(&request, 8), dword(&request, 12)), (Some(3), Some(3)));
        assert_eq!((qword(&request, 16), qword(&request, 24)), (Some(0x5000), Some(0x1_4000_0000)));
        assert_eq!(word(&request, 32), Some(1), "one batch, as the queue is one engine wide");
    }

    #[test]
    fn a_queue_is_one_engine_wide_in_one_vm() {
        let request = queue::create(9, 0x6000, 0);
        assert_eq!((word(&request, 8), word(&request, 10)), (Some(1), Some(1)));
        assert_eq!((dword(&request, 12), qword(&request, 24)), (Some(9), Some(0x6000)));
        assert_eq!(qword(&request, 0), Some(0), "no extension");
        assert_eq!(EngineInstance { class: 4, instance: 0, gt: 1 }.bytes(), [4, 0, 0, 0, 1, 0, 0, 0]);
        let property = queue::property(6, QUEUE_PROPERTY_BAN);
        assert_eq!((dword(&property, 8), dword(&property, 12)), (Some(6), Some(0)));
    }

    /// Sizes and offsets as a C program printed them against this PC's `xe_drm.h` (Linux v7.1's
    /// headers): `struct drm_xe_ext_set_property` is 48 bytes, its base's `next_extension` at 0,
    /// `name` at 8, `pad` at 12, then `property` at 16, `pad` at 20, `value` at 24 and `reserved`
    /// at 32; `extensions` is at 0 of `struct drm_xe_exec_queue_create`. The property is 6, the
    /// extension 0.
    #[test]
    fn a_queue_keyed_by_binding_table_points_at_one_set_property_extension() {
        let extension = queue::keyed_by_table();
        assert_eq!(qword(&extension, 0), Some(0), "the last extension");
        assert_eq!((dword(&extension, 8), dword(&extension, 12)), (Some(0), Some(0)), "set property");
        assert_eq!((dword(&extension, 16), dword(&extension, 20)), (Some(6), Some(0)));
        assert_eq!(qword(&extension, 24), Some(1), "turned on");
        assert_eq!((qword(&extension, 32), qword(&extension, 40)), (Some(0), Some(0)), "reserved");
        let request = queue::create(9, 0x6000, 0x7000);
        assert_eq!(qword(&request, 0), Some(0x7000));
        assert_eq!((dword(&request, 12), qword(&request, 24)), (Some(9), Some(0x6000)));
    }

    /// Sizes and offsets as `static_assert`s in a C file against `xe_drm.h` of Linux v7.2 held.
    #[test]
    fn a_fault_list_is_asked_for_and_read_as_the_header_lays_it_out() {
        assert_eq!(VM_FAULT_SIZE, 48);
        assert_eq!(VM_PROPERTY_FAULTS, 0);
        let ask = vm_property::faults(7, 96, 0x5000);
        assert_eq!((dword(&ask, 8), dword(&ask, 12), dword(&ask, 16)), (Some(7), Some(0), Some(96)));
        assert_eq!((qword(&ask, 24), ask.len()), (Some(0x5000), VM_GET_PROPERTY_SIZE));
        assert_eq!(vm_property::size_of_answer(&ask), 96);
        let fault = VmFault {
            address: 0x9_0000_1000,
            precision: 4096,
            access: FAULT_ACCESS_TYPE_WRITE,
            kind: FAULT_TYPE_NOT_PRESENT,
            level: FAULT_LEVEL_PDE,
        };
        let bytes = fault.bytes();
        assert_eq!((qword(&bytes, 0), dword(&bytes, 8)), (Some(0x9_0000_1000), Some(4096)));
        assert_eq!((bytes[12], bytes[13], bytes[14], bytes[15]), (1, 0, 1, 0), "the pad stays zero");
        let mut two = bytes.to_vec();
        two.extend(fault.bytes());
        two.extend([0; 5]);
        assert_eq!(VmFault::parse(&two), [fault, fault], "a part of an entry is ignored");
        assert!(VmFault::parse(&[]).is_empty());
    }

    #[test]
    fn a_fault_reads_in_words_and_an_unnamed_value_does_not_panic() {
        let fault = VmFault { address: 0x1000, precision: 4096, access: 2, kind: 1, level: 4 };
        let said = fault.to_string();
        for part in ["0x1000", "0x1000 bytes", "atomic", "read-only", "level 5"] {
            assert!(said.contains(part), "{said}");
        }
        assert!(VmFault { access: 9, ..fault }.to_string().contains("unnamed"));
        assert!(VmFault { kind: 9, level: 9, ..fault }.to_string().contains("unnamed fault type"));
    }

    #[test]
    fn a_wait_waits_for_a_point_nothing_has_reached_yet() {
        let request = syncobj::wait(0x100, 0x200, 0);
        assert_eq!((qword(&request, 0), qword(&request, 8)), (Some(0x100), Some(0x200)));
        assert_eq!((dword(&request, 24), dword(&request, 28)), (Some(1), Some(WAIT_FOR_SUBMIT)));
    }
}
