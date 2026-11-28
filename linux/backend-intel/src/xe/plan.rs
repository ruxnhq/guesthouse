//! The decisions that need no card: where a buffer lives, how a mapping becomes a bind, what an
//! exec waits for and signals, and what the kernel's refusals mean.
//!
//! All of it is plain values in and out, so it is tested without a card, and the self-test and
//! the backend decide the same way.

use super::abi::{self, BindOp, EngineInstance, NewBuffer, Sync};
use crate::xe_query::{Engine, MemoryRegion};
use guesthouse_backend::{MapRequest, UnmapRequest};

/// The page-table entry kinds of an Xe2 card, by index: `XE2_PAT_ENTRIES` in Mesa's
/// `src/intel/dev/intel_device_info.c`. The compressed ones, 9 and 11, are never used here.
pub mod pat {
    /// Cached, for memory only the card works on.
    pub const CARD: u16 = 0;
    /// Cached and kept coherent with the CPU one way. xe wants at least this for host pages, and
    /// it is right for them anyway, as the guest's CPU and the card share them.
    pub const SHARED_WITH_CPU: u16 = 1;
    /// For a buffer a display reads.
    pub const SCANOUT: u16 = 6;

    /// The entries with no coherency with the CPU: `__coh_mode` 0 in the kernel's `xe2_pat_table`
    /// (`drivers/gpu/drm/xe/xe_pat.c`). Past the table's end there is nothing at all.
    const NOT_COHERENT: [u16; 8] = [0, 3, 6, 8, 10, 12, 13, 14];
    const ENTRIES: u16 = 20;

    /// Whether entry `index` keeps the card coherent with the CPU at least one way. xe refuses a
    /// write-back buffer or host pages bound with one that doesn't (`xe_vm_bind_ioctl_validate_bo`,
    /// `vm_bind_ioctl_check_args`).
    pub fn coherent(index: u16) -> bool {
        index < ENTRIES && !NOT_COHERENT.contains(&index)
    }

    /// The entry a buffer is bound with, from how the CPU caches it and what it's for. One the CPU
    /// caches write-back needs coherency, which is right for it anyway, as the CPU reads and writes
    /// it through its caches. A write-combined one is the card's to cache.
    pub fn for_buffer(cpu_caching: u16, usage: super::BufferUse) -> u16 {
        if cpu_caching == super::abi::CPU_CACHING_WRITE_BACK {
            SHARED_WITH_CPU
        } else if usage == super::BufferUse::Scanout {
            SCANOUT
        } else {
            CARD
        }
    }
}

/// The pages host memory is bound in, whatever the card's own page is.
pub const HOST_PAGE: u64 = 0x1000;

/// What a buffer is for, which decides where it lives and how the CPU sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BufferUse {
    /// Video memory for the guest.
    Card,
    /// A picture a display or a compositor reads where it lies.
    Scanout,
    /// A small buffer of the backend's own that the CPU fills, like a batch.
    HostFilled,
}

/// Where buffers can live on a card, from its memory regions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placements {
    /// The system region's placement bit.
    system: u32,
    /// The card's own memory, if it has any: its bit, its page, and whether the CPU sees all of it.
    vram: Option<Vram>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Vram {
    bit: u32,
    page: u64,
    all_cpu_visible: bool,
}

impl Placements {
    /// `None` for a card that names no system region, which no xe card does.
    pub fn of(regions: &[MemoryRegion]) -> Option<Self> {
        let bit = |region: &MemoryRegion| 1u32.checked_shl(region.instance.into());
        let system = regions.iter().find(|region| region.class == MemoryRegion::SYSTEM)?;
        let vram = regions.iter().find(|region| region.class == MemoryRegion::VRAM);
        Some(Self {
            system: bit(system)?,
            vram: match vram {
                Some(region) => Some(Vram {
                    bit: bit(region)?,
                    page: u64::from(region.min_page_size).max(HOST_PAGE),
                    all_cpu_visible: region.cpu_visible >= region.total,
                }),
                None => None,
            },
        })
    }

    pub fn has_vram(&self) -> bool {
        self.vram.is_some()
    }

    /// The page a buffer for this use is made of, which its size and its binds are multiples of.
    pub fn page(&self, usage: BufferUse) -> u64 {
        match (usage, self.vram) {
            (BufferUse::Card | BufferUse::Scanout, Some(vram)) => vram.page,
            _ => HOST_PAGE,
        }
    }

    /// The buffer to ask xe for. On a card with memory of its own, the guest's video memory and
    /// pictures go there, with the system region named too so xe can move them out under pressure,
    /// and xe wants such buffers write-combined for the CPU. A card whose CPU window is smaller
    /// than its memory is asked to keep them inside the window, as the backend reads them.
    pub fn buffer(&self, size: u64, usage: BufferUse) -> NewBuffer {
        let size = size.max(1).next_multiple_of(self.page(usage));
        let scanout = if usage == BufferUse::Scanout { abi::BUFFER_SCANOUT } else { 0 };
        match (usage, self.vram) {
            (BufferUse::Card | BufferUse::Scanout, Some(vram)) => NewBuffer {
                size,
                placement: vram.bit | self.system,
                flags: scanout | if vram.all_cpu_visible { 0 } else { abi::BUFFER_NEEDS_VISIBLE_VRAM },
                cpu_caching: abi::CPU_CACHING_WRITE_COMBINED,
            },
            (BufferUse::Scanout, None) => NewBuffer {
                size,
                placement: self.system,
                flags: scanout,
                cpu_caching: abi::CPU_CACHING_WRITE_COMBINED,
            },
            _ => {
                NewBuffer { size, placement: self.system, flags: 0, cpu_caching: abi::CPU_CACHING_WRITE_BACK }
            }
        }
    }
}

/// What lies behind a block of memory the backend named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backing {
    /// A buffer xe made, with how the CPU caches it (`abi::CPU_CACHING_*`).
    Buffer { handle: u32, size: u64, page: u64, usage: BufferUse, cpu_caching: u16 },
    /// Host memory, which is the guest's own pages.
    HostPages { address: u64, size: u64 },
}

impl Backing {
    /// What the backing is, in a word, for the log.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Buffer { .. } => "a buffer xe made",
            Self::HostPages { .. } => "the guest's own pages",
        }
    }

    pub fn size(&self) -> u64 {
        match *self {
            Self::Buffer { size, .. } | Self::HostPages { size, .. } => size,
        }
    }

    fn page(&self) -> u64 {
        match *self {
            Self::Buffer { page, .. } => page,
            Self::HostPages { .. } => HOST_PAGE,
        }
    }
}

/// Why a mapping is refused before the kernel sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindRefused {
    /// The address, the offset or the length isn't a multiple of the memory's page.
    Unaligned {
        page: u64,
    },
    /// It reaches past the end of the memory.
    PastTheEnd,
    Empty,
}

impl std::fmt::Display for BindRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unaligned { page } => write!(f, "not a multiple of this memory's {page:#x} page"),
            Self::PastTheEnd => f.write_str("it reaches past the end of the memory"),
            Self::Empty => f.write_str("it maps nothing"),
        }
    }
}

/// The bind that puts `backing` at the guest's own address, so every address in the guest's
/// batches works as it is.
pub fn bind_for(request: &MapRequest, backing: &Backing) -> Result<BindOp, BindRefused> {
    let page = backing.page();
    if request.length == 0 {
        return Err(BindRefused::Empty);
    }
    if [request.address, request.offset, request.length].iter().any(|value| !value.is_multiple_of(page)) {
        return Err(BindRefused::Unaligned { page });
    }
    if request.offset.checked_add(request.length).is_none_or(|end| end > backing.size()) {
        return Err(BindRefused::PastTheEnd);
    }
    Ok(match *backing {
        Backing::Buffer { handle, usage, cpu_caching, .. } => BindOp {
            op: abi::BIND_MAP,
            buffer: handle,
            offset_or_host_address: request.offset,
            length: request.length,
            address: request.address,
            pat_index: pat::for_buffer(cpu_caching, usage),
        },
        Backing::HostPages { address, .. } => BindOp {
            op: abi::BIND_MAP_USERPTR,
            buffer: 0,
            offset_or_host_address: address + request.offset,
            length: request.length,
            address: request.address,
            pat_index: pat::SHARED_WITH_CPU,
        },
    })
}

/// The bind that takes a mapping away again.
pub fn unbind_for(unmap: &UnmapRequest) -> BindOp {
    BindOp {
        op: abi::BIND_UNMAP,
        buffer: 0,
        offset_or_host_address: 0,
        length: unmap.length,
        address: unmap.address,
        pat_index: 0,
    }
}

/// A point on a timeline of sync objects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    pub timeline: u32,
    pub point: u64,
}

/// What one exec waits for and signals: the VM's last bind, if it has made any, as a bind may still
/// be on its way when the batch that needs it is sent; the fence the card writes when the batch is
/// through, which the kernel takes one of at most; and the queue's own timeline, by which the
/// backend learns the run is over even when the fence was never written.
pub fn exec_syncs(last_bind: Option<Point>, fence: Option<(u64, u64)>, done: Point) -> Vec<Sync> {
    let mut syncs = Vec::with_capacity(3);
    if let Some(bind) = last_bind {
        syncs.push(Sync::After { timeline: bind.timeline, point: bind.point });
    }
    if let Some((address, value)) = fence {
        syncs.push(Sync::UserFence { address, value });
    }
    syncs.push(Sync::Signal { timeline: done.timeline, point: done.point });
    syncs
}

/// The bytes a user fence must be aligned to, which the kernel checks.
pub const FENCE_ALIGNMENT: u64 = 8;

/// What a refused exec means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecVerdict {
    /// The queue is full for now, or the kernel wants the call again. Nothing changed.
    NotYet,
    /// The queue or its VM was reset or banned: nothing of it will ever run again.
    Lost,
    /// Anything else, which is a fault of the backend's own.
    Failed,
}

pub fn exec_verdict(error: &std::io::Error) -> ExecVerdict {
    match error.raw_os_error() {
        // EWOULDBLOCK is the same number on Linux.
        Some(libc::EAGAIN) => ExecVerdict::NotYet,
        Some(libc::ECANCELED) => ExecVerdict::Lost,
        _ => ExecVerdict::Failed,
    }
}

/// What a refused bind means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindVerdict {
    /// No memory for it right now: the caller may free something and try again.
    NoRoom,
    /// The kernel found the request wrong, which is a fault of the backend's own.
    Wrong,
    Failed,
}

pub fn bind_verdict(error: &std::io::Error) -> BindVerdict {
    match error.raw_os_error() {
        Some(libc::ENOMEM | libc::ENOBUFS) => BindVerdict::NoRoom,
        Some(libc::EINVAL) => BindVerdict::Wrong,
        _ => BindVerdict::Failed,
    }
}

/// One engine of each class the card lists, the lowest instance of each, in class order.
pub fn one_engine_per_class(engines: &[Engine]) -> Vec<EngineInstance> {
    let mut chosen: Vec<EngineInstance> = Vec::new();
    let mut sorted: Vec<&Engine> = engines.iter().collect();
    sorted.sort_by_key(|engine| (engine.class, engine.instance));
    for engine in sorted {
        if chosen.last().is_none_or(|last| last.class != engine.class) {
            chosen.push(EngineInstance { class: engine.class, instance: engine.instance, gt: engine.gt });
        }
    }
    chosen
}

/// The engine of a class and instance, if the card has it.
pub fn engine(engines: &[Engine], class: u16, instance: u16) -> Option<EngineInstance> {
    let found = engines.iter().find(|engine| engine.class == class && engine.instance == instance)?;
    Some(EngineInstance { class, instance, gt: found.gt })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card;
    use guesthouse_backend::{MapMemoryType, MemoryId, SpaceId};
    use std::io::Error;

    fn request(address: u64, offset: u64, length: u64) -> MapRequest {
        MapRequest {
            space: SpaceId(1),
            memory: MemoryId(1),
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

    fn battlemage() -> Placements {
        Placements::of(&card::tests::battlemage().regions).unwrap()
    }

    fn lunar_lake() -> Placements {
        Placements::of(&card::tests::lunar_lake().regions).unwrap()
    }

    #[test]
    fn video_memory_goes_to_the_cards_own_memory_and_may_be_moved_out() {
        let buffer = battlemage().buffer(0x1_0000, BufferUse::Card);
        assert_eq!(buffer.placement.count_ones(), 2, "the card's region and the system's");
        assert_eq!(buffer.cpu_caching, abi::CPU_CACHING_WRITE_COMBINED);
        assert_eq!(buffer.flags, 0, "the CPU sees all of this card's memory");
        let batch = battlemage().buffer(0x1_0000, BufferUse::HostFilled);
        assert_eq!((batch.placement.count_ones(), batch.cpu_caching), (1, abi::CPU_CACHING_WRITE_BACK));
        assert_ne!(batch.placement & buffer.placement, 0, "the system region is in both");
    }

    #[test]
    fn a_card_with_no_memory_of_its_own_uses_the_systems() {
        let placements = lunar_lake();
        assert!(!placements.has_vram());
        let buffer = placements.buffer(100, BufferUse::Card);
        assert_eq!((buffer.size, buffer.cpu_caching, buffer.flags), (0x1000, abi::CPU_CACHING_WRITE_BACK, 0));
        let picture = placements.buffer(0x2000, BufferUse::Scanout);
        assert_eq!(picture.placement, buffer.placement);
        assert_eq!(
            (picture.cpu_caching, picture.flags),
            (abi::CPU_CACHING_WRITE_COMBINED, abi::BUFFER_SCANOUT)
        );
    }

    #[test]
    fn a_small_cpu_window_keeps_buffers_inside_it() {
        let mut regions = card::tests::battlemage().regions;
        for region in regions.iter_mut().filter(|region| region.class == MemoryRegion::VRAM) {
            region.cpu_visible = 256 << 20;
        }
        let buffer = Placements::of(&regions).unwrap().buffer(0x1000, BufferUse::Scanout);
        assert_eq!(buffer.flags, abi::BUFFER_SCANOUT | abi::BUFFER_NEEDS_VISIBLE_VRAM);
    }

    #[test]
    fn a_card_with_no_system_region_has_nowhere_to_put_anything() {
        assert_eq!(Placements::of(&[]), None);
    }

    #[test]
    fn video_memory_is_bound_at_the_guests_address_as_cached_card_memory() {
        let backing = Backing::Buffer {
            handle: 9,
            size: 0x1_0000,
            page: 0x1000,
            usage: BufferUse::Card,
            cpu_caching: abi::CPU_CACHING_WRITE_COMBINED,
        };
        let op = bind_for(&request(0x1_4000_0000, 0x2000, 0x4000), &backing).unwrap();
        assert_eq!((op.op, op.buffer, op.pat_index), (abi::BIND_MAP, 9, pat::CARD));
        assert_eq!((op.offset_or_host_address, op.length, op.address), (0x2000, 0x4000, 0x1_4000_0000));
        let picture = Backing::Buffer {
            handle: 9,
            size: 0x1_0000,
            page: 0x1000,
            usage: BufferUse::Scanout,
            cpu_caching: abi::CPU_CACHING_WRITE_COMBINED,
        };
        assert_eq!(bind_for(&request(0x1_4000_0000, 0, 0x1000), &picture).unwrap().pat_index, pat::SCANOUT);
    }

    /// Lunar Lake has no memory of its own, so its video memory is write-back system memory, which
    /// xe binds only with an entry coherent with the CPU.
    #[test]
    fn video_memory_in_system_memory_is_bound_coherent_with_the_cpu() {
        let placements = lunar_lake();
        let made = placements.buffer(0x1_0000, BufferUse::Card);
        let backing = Backing::Buffer {
            handle: 9,
            size: made.size,
            page: placements.page(BufferUse::Card),
            usage: BufferUse::Card,
            cpu_caching: made.cpu_caching,
        };
        let op = bind_for(&request(0x1_4000_0000, 0, 0x1000), &backing).unwrap();
        assert_eq!(op.pat_index, pat::SHARED_WITH_CPU);
        assert!(pat::coherent(op.pat_index));
    }

    #[test]
    fn every_buffer_is_bound_with_an_entry_xe_takes_for_its_caching() {
        for placements in [battlemage(), lunar_lake()] {
            for usage in [BufferUse::Card, BufferUse::Scanout, BufferUse::HostFilled] {
                let made = placements.buffer(0x1000, usage);
                let index = pat::for_buffer(made.cpu_caching, usage);
                let write_back = made.cpu_caching == abi::CPU_CACHING_WRITE_BACK;
                assert!(!write_back || pat::coherent(index), "{usage:?} on {placements:?}");
            }
        }
        assert!(!pat::coherent(pat::CARD) && !pat::coherent(pat::SCANOUT));
        assert!(pat::coherent(pat::SHARED_WITH_CPU));
        assert!(!pat::coherent(20), "past the table");
    }

    #[test]
    fn guest_pages_are_bound_by_their_host_address_and_kept_coherent() {
        let backing = Backing::HostPages { address: 0x7F00_0000_0000, size: 0x20_0000 };
        let op = bind_for(&request(0x255_F13C_0000, 0x3000, 0x1000), &backing).unwrap();
        assert_eq!((op.op, op.buffer, op.pat_index), (abi::BIND_MAP_USERPTR, 0, pat::SHARED_WITH_CPU));
        assert_eq!(op.offset_or_host_address, 0x7F00_0000_3000);
        assert_eq!(op.address, 0x255_F13C_0000, "above the core's own ranges, as the guest placed it");
    }

    #[test]
    fn a_mapping_the_kernel_would_refuse_is_refused_first() {
        let backing = Backing::Buffer {
            handle: 9,
            size: 0x2_0000,
            page: 0x1_0000,
            usage: BufferUse::Card,
            cpu_caching: abi::CPU_CACHING_WRITE_COMBINED,
        };
        let unaligned = BindRefused::Unaligned { page: 0x1_0000 };
        assert_eq!(bind_for(&request(0x1000, 0, 0x1_0000), &backing), Err(unaligned));
        assert_eq!(bind_for(&request(0x1_0000, 0x1000, 0x1_0000), &backing), Err(unaligned));
        assert_eq!(bind_for(&request(0x1_0000, 0, 0x1000), &backing), Err(unaligned));
        assert_eq!(bind_for(&request(0x1_0000, 0x1_0000, 0x2_0000), &backing), Err(BindRefused::PastTheEnd));
        assert_eq!(bind_for(&request(0x1_0000, 0, 0), &backing), Err(BindRefused::Empty));
        assert_eq!(
            bind_for(&request(0x1_0000, !0xFFFF_u64, 0x1_0000), &backing),
            Err(BindRefused::PastTheEnd)
        );
        assert!(bind_for(&request(0x1_0000, 0x1_0000, 0x1_0000), &backing).is_ok());
    }

    #[test]
    fn an_unmap_is_an_unbind_of_the_same_addresses() {
        let unmap = UnmapRequest { space: SpaceId(1), memory: MemoryId(1), address: 0x4000, length: 0x2000 };
        let op = unbind_for(&unmap);
        assert_eq!((op.op, op.address, op.length, op.buffer), (abi::BIND_UNMAP, 0x4000, 0x2000, 0));
    }

    #[test]
    fn an_exec_waits_for_the_last_bind_writes_one_fence_and_signals_its_queue() {
        let done = Point { timeline: 4, point: 7 };
        let syncs = exec_syncs(Some(Point { timeline: 2, point: 5 }), Some((0x1000, 9)), done);
        assert_eq!(
            syncs,
            [
                Sync::After { timeline: 2, point: 5 },
                Sync::UserFence { address: 0x1000, value: 9 },
                Sync::Signal { timeline: 4, point: 7 }
            ]
        );
        // A VM that has bound nothing has no point to wait for, and the kernel refuses a wait on
        // a timeline with nothing attached.
        assert_eq!(exec_syncs(None, None, done), [Sync::Signal { timeline: 4, point: 7 }]);
    }

    #[test]
    fn the_kernels_refusals_of_an_exec_and_a_bind_are_told_apart() {
        let errno = Error::from_raw_os_error;
        assert_eq!(exec_verdict(&errno(libc::EAGAIN)), ExecVerdict::NotYet);
        assert_eq!(exec_verdict(&errno(libc::EWOULDBLOCK)), ExecVerdict::NotYet);
        assert_eq!(exec_verdict(&errno(libc::ECANCELED)), ExecVerdict::Lost);
        assert_eq!(exec_verdict(&errno(libc::EINVAL)), ExecVerdict::Failed);
        assert_eq!(exec_verdict(&Error::other("no errno")), ExecVerdict::Failed);
        assert_eq!(bind_verdict(&errno(libc::ENOMEM)), BindVerdict::NoRoom);
        assert_eq!(bind_verdict(&errno(libc::ENOBUFS)), BindVerdict::NoRoom);
        assert_eq!(bind_verdict(&errno(libc::EINVAL)), BindVerdict::Wrong);
        assert_eq!(bind_verdict(&errno(libc::EFAULT)), BindVerdict::Failed);
    }

    #[test]
    fn each_engine_class_is_tried_once_on_its_first_engine() {
        let engines = card::tests::battlemage().engines;
        let chosen = one_engine_per_class(&engines);
        let classes: Vec<u16> = chosen.iter().map(|engine| engine.class).collect();
        let mut expected: Vec<u16> = engines.iter().map(|engine| engine.class).collect();
        expected.sort_unstable();
        expected.dedup();
        assert_eq!(classes, expected);
        assert!(chosen.iter().all(|engine| engine.instance == 0));
        assert_eq!(engine(&engines, Engine::COMPUTE, 0).map(|engine| engine.class), Some(Engine::COMPUTE));
        assert_eq!(engine(&engines, Engine::COMPUTE, 9), None);
    }
}
