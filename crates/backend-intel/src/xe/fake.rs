//! A stand-in for xe, for tests: it records every call, can fail any of them, keeps the kernel's
//! rules that a wrong plan would break, and runs the few instructions [`super::batch`] writes, so
//! a test can see a fence written and a value stored without a card.
//!
//! What it does on a fault or a hang is what the design expects of xe, not something seen on a
//! card: the queue is banned, its timeline still signals, and the user fence is left unwritten.
//!
//! A buffer's bytes are host memory in a memfd, so a map, an export and a bind all reach the same
//! bytes. With the `lab-fake-card` feature it is also the lab's card: see [`FakeXe::lab`].

// A lab build uses only part of what the tests do.
#![cfg_attr(not(test), allow(dead_code))]

use super::abi::{self, BindOp, EngineInstance, Keying, NewBuffer, Sync, VmFault};
use super::batch;
use super::device::{CpuMap, EngineCycles, Xe};
use super::plan::pat;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// A call the stand-in was given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    CreateVm { flags: u32 },
    DestroyVm(u32),
    CreateBuffer(NewBuffer),
    CloseBuffer(u32),
    MapBuffer(u32),
    ExportBuffer(u32),
    Bind { vm: u32, op: BindOp, signal: Option<Sync> },
    CreateQueue { vm: u32, engine: EngineInstance, keying: Keying },
    DestroyQueue(u32),
    QueueBanned(u32),
    Exec { queue: u32, batch: u64, syncs: Vec<Sync> },
    CreateSyncobj,
    DestroySyncobj(u32),
    PointReached { syncobj: u32, point: u64 },
    EngineCycles(EngineInstance),
    NamesFaults,
    VmFaults(u32),
}

impl Call {
    /// The method's name, for failing calls by kind.
    pub fn name(&self) -> &'static str {
        match self {
            Self::CreateVm { .. } => "create_vm",
            Self::DestroyVm(_) => "destroy_vm",
            Self::CreateBuffer(_) => "create_buffer",
            Self::CloseBuffer(_) => "close_buffer",
            Self::MapBuffer(_) => "map_buffer",
            Self::ExportBuffer(_) => "export_buffer",
            Self::Bind { .. } => "bind",
            Self::CreateQueue { .. } => "create_queue",
            Self::DestroyQueue(_) => "destroy_queue",
            Self::QueueBanned(_) => "queue_banned",
            Self::Exec { .. } => "exec",
            Self::CreateSyncobj => "create_syncobj",
            Self::DestroySyncobj(_) => "destroy_syncobj",
            Self::PointReached { .. } => "point_reached",
            Self::EngineCycles(_) => "engine_cycles",
            Self::NamesFaults => "names_faults",
            Self::VmFaults(_) => "vm_faults",
        }
    }
}

/// A buffer's bytes: a memfd, mapped once here for the stand-in's own reads and writes.
#[derive(Debug)]
struct Bytes {
    file: OwnedFd,
    start: *mut u8,
    length: usize,
    /// Counts up as buffers are made, so the lab's card can tell the newest apart.
    #[cfg_attr(not(feature = "lab-fake-card"), allow(dead_code))]
    serial: u64,
}

static MADE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl Bytes {
    fn new(length: usize) -> io::Result<Self> {
        // SAFETY: a fresh memfd with a constant name; the result is checked.
        let fd = unsafe { libc::memfd_create(c"guesthouse-fake-xe".as_ptr(), libc::MFD_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the descriptor was just made and is owned by nothing else.
        let file = unsafe { OwnedFd::from_raw_fd(fd) };
        let size = libc::off_t::try_from(length).map_err(|_| errno(libc::EINVAL))?;
        // SAFETY: plain calls on the descriptor made above.
        if unsafe { libc::ftruncate(file.as_raw_fd(), size) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let start = map(&file, length)?;
        let serial = MADE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(Self { file, start, length, serial })
    }
}

// SAFETY: plain shared memory and a descriptor, with no thread affinity; writes through it are
// the stand-in's under its lock, or a map's, as with real buffers.
unsafe impl Send for Bytes {}
// SAFETY: as above; `&Bytes` gives out only the address and the descriptor.
unsafe impl std::marker::Sync for Bytes {}

impl Drop for Bytes {
    fn drop(&mut self) {
        // SAFETY: the mapping was made in `new` with this start and length.
        unsafe { libc::munmap(self.start.cast(), self.length) };
    }
}

/// A shared mapping of all of a memfd.
fn map(file: &OwnedFd, length: usize) -> io::Result<*mut u8> {
    // SAFETY: a fresh shared mapping of the memfd; nothing else in the process is touched.
    let start = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            length,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            file.as_raw_fd(),
            0,
        )
    };
    if start == libc::MAP_FAILED { Err(io::Error::last_os_error()) } else { Ok(start.cast()) }
}

/// What is bound at a range of a VM's addresses.
#[derive(Debug, Clone)]
struct Bound {
    length: u64,
    /// Where the first byte is in this process.
    host: *mut u8,
    /// The buffer it is in, kept while bound, as xe keeps a bound buffer. `None` for host pages.
    buffer: Option<Arc<Bytes>>,
}

#[derive(Debug, Default)]
struct Vm {
    flags: u32,
    bound: BTreeMap<u64, Bound>,
}

impl Vm {
    /// The host address of `length` bytes at a GPU address, if one binding holds them all.
    fn host(&self, address: u64, length: u64) -> Option<*mut u8> {
        let (&start, bound) = self.bound.range(..=address).next_back()?;
        let end = address.checked_add(length)?;
        // SAFETY: the offset is inside the binding, which was checked against its memory when made.
        (end <= start + bound.length).then(|| unsafe { bound.host.add((address - start) as usize) })
    }

    /// Takes a range out of the bindings, keeping what lies to either side of it.
    fn clear(&mut self, address: u64, length: u64) {
        let end = address.saturating_add(length);
        let touched: Vec<u64> = self
            .bound
            .range(..end)
            .filter(|(start, bound)| **start + bound.length > address)
            .map(|(start, _)| *start)
            .collect();
        for start in touched {
            let bound = self.bound.remove(&start).expect("just listed");
            if start < address {
                let left = Bound { length: address - start, ..bound.clone() };
                self.bound.insert(start, left);
            }
            if start + bound.length > end {
                // SAFETY: still inside the same binding.
                let host = unsafe { bound.host.add((end - start) as usize) };
                self.bound
                    .insert(end, Bound { length: start + bound.length - end, host, buffer: bound.buffer });
            }
        }
    }
}

#[derive(Debug)]
struct Queue {
    vm: u32,
    banned: bool,
    keying: Keying,
}

/// A timeline of points, each attached when work that will signal it is taken, and reached when
/// that work ends.
#[derive(Debug, Default)]
struct Timeline {
    attached: u64,
    reached: u64,
}

/// A run taken and not yet let through, while a test holds runs back.
#[derive(Debug)]
struct Held {
    queue: u32,
    batch: u64,
    syncs: Vec<Sync>,
}

/// How a batch came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Done,
    /// It touched an address nothing is bound at.
    Fault(u64),
    /// It never ended.
    Hang,
    /// It parked in a semaphore wait, in a VM where work may run as long as it likes.
    Parked,
}

/// The most instructions a batch may run before it counts as hung.
const MOST_INSTRUCTIONS: usize = 4096;
/// The cycles the stand-in's engine clock gains between two reads, and the nanoseconds its CPU
/// clock does. Their ratio is a 19.2 MHz counter.
const CYCLES_PER_READ: u64 = 19_200;
const NANOSECONDS_PER_READ: u64 = 1_000_000;

#[derive(Debug, Default)]
struct State {
    calls: Vec<Call>,
    next_id: u32,
    vms: HashMap<u32, Vm>,
    buffers: HashMap<u32, Arc<Bytes>>,
    /// How the CPU caches each buffer, which decides the page-table entries it may be bound with.
    cpu_caching: HashMap<u32, u16>,
    queues: HashMap<u32, Queue>,
    timelines: HashMap<u32, Timeline>,
    fail_at: HashMap<usize, i32>,
    fail_named: HashMap<&'static str, i32>,
    hold_runs: bool,
    /// Every run faults at its batch: see [`FakeXe::fault_runs`].
    fault_runs: bool,
    held: Vec<Held>,
    faults: Vec<u64>,
    /// What xe would list for each VM, as it does when the card has page fault support.
    vm_faults: HashMap<u32, Vec<VmFault>>,
    clock_reads: u64,
    names_faults: bool,
    /// Calls taken, recorded or not.
    entered: usize,
    /// The lab's card: see [`FakeXe::lab`].
    lab: Option<Lab>,
    registers: Registers,
    /// How xe answers a queue keyed by binding table: see [`FakeXe::without_keying`] and
    /// [`FakeXe::refuse_keying`].
    keying_refused: Option<i32>,
    /// Takes the property and sets nothing for it: see [`FakeXe::ignore_keying`].
    keying_ignored: bool,
}

/// The register whose bit [`STATE_CACHE_PERF_FIX_OFF`] keys the render colour cache by binding
/// table and index. xe sets it at the start of a queue keyed so, and on no other queue.
pub const COMMON_SLICE_CHICKEN3: u32 = 0x7304;
pub const STATE_CACHE_PERF_FIX_OFF: u32 = 1 << 13;

/// The registers a batch may set and read. On a card a batch reaches only the registers the kernel
/// lets user work reach, and any other is neither set nor read, so the memory a read names keeps
/// what it held.
#[derive(Debug, Default)]
struct Registers {
    reachable: HashSet<u32>,
    values: HashMap<u32, u32>,
}

/// What the lab's card does differently, and what it has noticed.
#[derive(Debug, Default)]
struct Lab {
    /// From [`FakeXe::stop_running`] on, batches are finished without running them.
    runs_nothing: bool,
    /// Batches and user fences at addresses nothing was bound at: faults on a card.
    unbound_batches: u64,
    unbound_fences: u64,
    /// The display driver's slot stamps: see [`stamps`].
    #[cfg(feature = "lab-fake-card")]
    stamps: stamps::Stamps,
}

#[cfg(feature = "lab-fake-card")]
mod stamps;

/// How many of each kind of unbound address the lab's card names in the log.
#[cfg(feature = "lab-fake-card")]
const LAB_NAMED: u64 = 20;

// SAFETY: the raw pointers are into buffers the state owns, or into host memory a test lent and
// keeps alive; nothing here is tied to a thread.
unsafe impl Send for State {}

fn errno(code: i32) -> io::Error {
    io::Error::from_raw_os_error(code)
}

impl State {
    /// Records a call, and fails it if a test asked for that.
    fn enter(&mut self, call: Call) -> io::Result<()> {
        let index = self.entered;
        self.entered += 1;
        let name = call.name();
        if self.lab.is_none() {
            self.calls.push(call);
        }
        match self.fail_at.get(&index).or_else(|| self.fail_named.get(name)) {
            Some(&code) => Err(errno(code)),
            None => Ok(()),
        }
    }

    fn new_id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id
    }

    fn read_dword(vm: &Vm, address: u64) -> Result<u32, Outcome> {
        let host = vm.host(address, 4).ok_or(Outcome::Fault(address))?;
        // SAFETY: `host` resolved to four bytes inside a binding.
        Ok(u32::from_le(unsafe { host.cast::<u32>().read_unaligned() }))
    }

    fn read_qword(vm: &Vm, address: u64) -> Result<u64, Outcome> {
        Ok(u64::from(Self::read_dword(vm, address)?) | u64::from(Self::read_dword(vm, address + 4)?) << 32)
    }

    fn store_dword(vm: &Vm, address: u64, value: u32) -> Result<(), Outcome> {
        let host = vm.host(address, 4).ok_or(Outcome::Fault(address))?;
        // SAFETY: `host` resolved to four bytes inside a binding.
        unsafe { host.cast::<u32>().write_unaligned(value.to_le()) };
        Ok(())
    }

    fn store(vm: &Vm, address: u64, value: u64) -> Result<(), Outcome> {
        let host = vm.host(address, 8).ok_or(Outcome::Fault(address))?;
        // SAFETY: `host` resolved to eight bytes inside a binding.
        unsafe { host.cast::<u64>().write_unaligned(value.to_le()) };
        Ok(())
    }

    fn copy(vm: &Vm, source: u64, target: u64, bytes: u64) -> Result<(), Outcome> {
        let from = vm.host(source, bytes).ok_or(Outcome::Fault(source))?;
        let to = vm.host(target, bytes).ok_or(Outcome::Fault(target))?;
        // SAFETY: both resolved to `bytes` bytes inside bindings, which may be the same memory.
        unsafe { std::ptr::copy(from, to, bytes as usize) };
        Ok(())
    }

    /// Runs one `XY_FAST_COPY_BLT`, pixel by pixel, each end laid out as [`surface_offset`] says.
    /// What the PRM forbids, or leaves undefined, a card would do something unknown with, so the
    /// stand-in faults at the instruction instead: a reserved bit set, a length other than 8, a
    /// Tile Y type bit clear, a depth other than 32 bits, a base off a cache line, a pitch that
    /// isn't whole tiles or OWords, a first pixel off an OWord, an empty or inside-out rectangle.
    /// A bad address faults where it points, as a run that reads past its binding does.
    fn fast_copy(vm: &Vm, dwords: &[u32; batch::FAST_COPY_DWORDS]) -> Result<(), ()> {
        const DWORD0_RESERVED: u32 = 0x000F_9F00;
        const DWORD1_RESERVED: u32 = 0x380F_0000;
        let header = dwords[0];
        if header & DWORD0_RESERVED != 0 || header & 0xFF != 8 {
            return Err(());
        }
        let tile4_bits = batch::FAST_COPY_SOURCE_TILE4 | batch::FAST_COPY_TARGET_TILE4;
        if dwords[1] & tile4_bits != tile4_bits
            || dwords[1] & DWORD1_RESERVED != 0
            || dwords[1] & batch::FAST_COPY_DEPTH_MASK != batch::FAST_COPY_DEPTH_32
            || dwords[7] & 0xFF0F_0000 != 0
        {
            return Err(());
        }
        let tiling = |shift: u32| match header >> shift & 3 {
            0 => batch::Tiling::Linear,
            1 => batch::Tiling::TileX,
            2 => batch::Tiling::Tile4,
            _ => batch::Tiling::Tile64,
        };
        let (source_tiling, target_tiling) =
            (tiling(batch::FAST_COPY_SOURCE_TILING_SHIFT), tiling(batch::FAST_COPY_TARGET_TILING_SHIFT));
        let pitch = |tiling: batch::Tiling, field: u32| {
            let field = u64::from(field & 0xFFFF);
            if tiling == batch::Tiling::Linear { field } else { field * 4 }
        };
        let (source_pitch, target_pitch) = (pitch(source_tiling, dwords[7]), pitch(target_tiling, dwords[1]));
        let corner = |dword: u32| (u64::from(dword & 0xFFFF), u64::from(dword >> 16));
        let ((x1, y1), (x2, y2), (source_x, source_y)) =
            (corner(dwords[2]), corner(dwords[3]), corner(dwords[6]));
        let address = |low: u32, high: u32| u64::from(high) << 32 | u64::from(low);
        let (target, source) = (address(dwords[4], dwords[5]), address(dwords[8], dwords[9]));
        let pixel = u64::from(batch::FAST_COPY_PIXEL_BYTES);
        let oword = u64::from(batch::OWORD);
        let side_breaks_rules = |base: u64, pitch: u64, tiling: batch::Tiling, x: u64| {
            !base.is_multiple_of(batch::CACHE_LINE)
                || pitch == 0
                || !pitch.is_multiple_of(tiling.width().into())
                || !(x * pixel).is_multiple_of(oword)
        };
        if x2 <= x1
            || y2 <= y1
            || [x2, y2].iter().any(|&end| end > u64::from(batch::FAST_COPY_COORDINATE_MOST))
            || side_breaks_rules(target, target_pitch, target_tiling, x1)
            || side_breaks_rules(source, source_pitch, source_tiling, source_x)
        {
            return Err(());
        }
        for row in 0..y2 - y1 {
            for column in 0..x2 - x1 {
                let from = source
                    + surface_offset(
                        source_tiling,
                        source_pitch,
                        (source_x + column) * pixel,
                        source_y + row,
                    );
                let to =
                    target + surface_offset(target_tiling, target_pitch, (x1 + column) * pixel, y1 + row);
                Self::copy(vm, from, to, pixel).map_err(|_| ())?;
            }
        }
        Ok(())
    }

    /// Runs the instructions this backend writes. Anything else ends the batch quietly.
    fn run(vm: &Vm, registers: &mut Registers, batch: u64) -> Outcome {
        const MEM_COPY_HEADER_MASK: u32 = 0xFFC0_0000;
        const MEM_COPY: u32 = 0x5680_0000;
        const PAGES: u32 = 1 << 19;
        const MATRIX: u32 = 1 << 17;
        let mut at = batch;
        for _ in 0..MOST_INSTRUCTIONS {
            let step = (|| -> Result<Option<u64>, Outcome> {
                let header = Self::read_dword(vm, at)?;
                if header == batch::BATCH_END {
                    return Ok(None);
                }
                if header == batch::NOOP {
                    return Ok(Some(at + 4));
                }
                if header == batch::STORE_QWORD_HEADER {
                    Self::store(vm, Self::read_qword(vm, at + 4)?, Self::read_qword(vm, at + 12)?)?;
                    return Ok(Some(at + 4 * batch::STORE_QWORD_DWORDS as u64));
                }
                if header == batch::LOAD_REGISTER_HEADER {
                    let register = Self::read_dword(vm, at + 4)?;
                    if registers.reachable.contains(&register) {
                        registers.values.insert(register, Self::read_dword(vm, at + 8)?);
                    }
                    return Ok(Some(at + 4 * batch::LOAD_REGISTER_DWORDS as u64));
                }
                if header == batch::STORE_REGISTER_HEADER {
                    let register = Self::read_dword(vm, at + 4)?;
                    if registers.reachable.contains(&register) {
                        let value = registers.values.get(&register).copied().unwrap_or(0);
                        Self::store_dword(vm, Self::read_qword(vm, at + 8)?, value)?;
                    }
                    return Ok(Some(at + 4 * batch::STORE_REGISTER_DWORDS as u64));
                }
                if header == batch::BATCH_START_HEADER {
                    return Ok(Some(Self::read_qword(vm, at + 4)?));
                }
                if header & !0xFFFF == batch::SEMAPHORE_WAIT_HEADER & !0xFFFF {
                    return Err(Outcome::Parked);
                }
                if header & batch::FAST_COPY_HEADER_MASK == batch::FAST_COPY {
                    let mut dwords = [0u32; batch::FAST_COPY_DWORDS];
                    for (index, dword) in dwords.iter_mut().enumerate() {
                        *dword = Self::read_dword(vm, at + 4 * index as u64)?;
                    }
                    Self::fast_copy(vm, &dwords).map_err(|()| Outcome::Fault(at))?;
                    return Ok(Some(at + 4 * batch::FAST_COPY_DWORDS as u64));
                }
                if header & MEM_COPY_HEADER_MASK == MEM_COPY {
                    let field = |index: u64| Self::read_dword(vm, at + 4 * index).map(|v| u64::from(v) + 1);
                    let (width, rows, pitch) = (field(1)?, field(2)?, field(3)?);
                    let (source, target) = (Self::read_qword(vm, at + 20)?, Self::read_qword(vm, at + 28)?);
                    if header & MATRIX != 0 {
                        for row in 0..rows {
                            Self::copy(vm, source + row * pitch, target + row * pitch, width)?;
                        }
                    } else {
                        let bytes = if header & PAGES != 0 { width * batch::MEM_COPY_PAGE } else { width };
                        Self::copy(vm, source, target, bytes)?;
                    }
                    return Ok(Some(at + 4 * batch::MEM_COPY_DWORDS as u64));
                }
                Ok(None)
            })();
            match step {
                Ok(Some(next)) => at = next,
                Ok(None) => return Outcome::Done,
                Err(outcome) => return outcome,
            }
        }
        Outcome::Hang
    }

    /// Lets a taken run through: the batch, then the fence, then the signals.
    fn finish(&mut self, run: Held) {
        if self.lab.as_ref().is_some_and(|lab| lab.runs_nothing) {
            return self.finish_unrun(run);
        }
        let Some(queue) = self.queues.get(&run.queue) else { return };
        let queue_vm = queue.vm;
        // The register is the queue's context's, so it reads as its queue was made.
        let bit = self.registers.values.entry(COMMON_SLICE_CHICKEN3).or_default();
        match queue.keying {
            Keying::BindingTableAndIndex if !self.keying_ignored => *bit |= STATE_CACHE_PERF_FIX_OFF,
            _ => *bit &= !STATE_CACHE_PERF_FIX_OFF,
        }
        let vm = self.vms.get(&queue.vm);
        let mut outcome = match vm {
            Some(_) if self.fault_runs => Outcome::Fault(run.batch),
            Some(vm) if !queue.banned => Self::run(vm, &mut self.registers, run.batch),
            _ => Outcome::Hang,
        };
        let long_running = vm.is_some_and(|vm| vm.flags & abi::VM_LONG_RUNNING != 0);
        if outcome == Outcome::Parked {
            if long_running {
                return;
            }
            // Anywhere else xe's job timeout ends it.
            outcome = Outcome::Hang;
        }
        if let (Outcome::Done, Some(vm)) = (outcome, vm) {
            for sync in &run.syncs {
                if let Sync::UserFence { address, value } = *sync
                    && let Err(fault) = Self::store(vm, address, value)
                {
                    outcome = fault;
                }
            }
        }
        if outcome != Outcome::Done {
            self.queues.get_mut(&run.queue).expect("found above").banned = true;
            if let Outcome::Fault(address) = outcome {
                self.faults.push(address);
                // A hang adds nothing to a VM's list, a fault does, if the card can say.
                if self.names_faults {
                    let fault = VmFault {
                        address: address & !0xFFF,
                        precision: 4096,
                        access: abi::FAULT_ACCESS_TYPE_READ,
                        kind: abi::FAULT_TYPE_NOT_PRESENT,
                        level: abi::FAULT_LEVEL_PTE,
                    };
                    self.vm_faults.entry(queue_vm).or_default().push(fault);
                }
            }
        }
        for sync in &run.syncs {
            if let Sync::Signal { timeline, point } = *sync
                && let Some(timeline) = self.timelines.get_mut(&timeline)
            {
                timeline.reached = timeline.reached.max(point);
            }
        }
    }

    /// The kernel's own checks of an exec's or a bind's syncs, in a VM made with `vm_flags`.
    fn check_syncs(&self, vm_flags: u32, syncs: &[Sync]) -> io::Result<()> {
        let mut fences = 0;
        for sync in syncs {
            if matches!(sync, Sync::Signal { .. }) && vm_flags & abi::VM_LONG_RUNNING != 0 {
                return Err(errno(libc::EOPNOTSUPP));
            }
            match *sync {
                Sync::After { timeline, point } => {
                    let timeline = self.timelines.get(&timeline).ok_or_else(|| errno(libc::ENOENT))?;
                    if timeline.attached < point {
                        return Err(errno(libc::EINVAL));
                    }
                }
                Sync::Signal { timeline, .. } => {
                    self.timelines.get(&timeline).ok_or_else(|| errno(libc::ENOENT))?;
                }
                Sync::UserFence { address, .. } => {
                    fences += 1;
                    if !address.is_multiple_of(8) || fences > 1 {
                        return Err(errno(libc::EINVAL));
                    }
                }
            }
        }
        Ok(())
    }

    fn attach(&mut self, syncs: &[Sync]) {
        for sync in syncs {
            if let Sync::Signal { timeline, point } = *sync
                && let Some(timeline) = self.timelines.get_mut(&timeline)
            {
                timeline.attached = timeline.attached.max(point);
            }
        }
    }
}

/// Where the byte at byte `x` of row `y` of a surface lies, from the surface's start, as the
/// stand-in card lays out each tiling: whole tiles in rows of tiles across the pitch, and inside a
/// tile the bits of `x` and `y` spread over the offset's bits as gmmlib's swizzles have them
/// (`INTEL_TILE_X`, `INTEL_TILE_4` and `INTEL_TILE_64_32` in
/// `Source/GmmLib/Utility/CpuSwizzleBlt/CpuSwizzleBlt.c`, where `X` marks a bit of `x` and `Y` one
/// of `y`, from the offset's bit 15 down to bit 0):
///
/// - TileX, `o o o o Y Y Y X X X X X X X X X`: 512 bytes by 8 rows.
/// - Tile4, `o o o o Y Y X Y X X Y Y X X X X`: 128 bytes by 32 rows.
/// - Tile64 of 32-bit pixels, `Y Y X X Y Y X Y X X Y Y X X X X`: 512 bytes by 128 rows.
pub fn surface_offset(tiling: batch::Tiling, pitch: u64, x: u64, y: u64) -> u64 {
    let (x_bits, y_bits) = match tiling {
        batch::Tiling::Linear => return y * pitch + x,
        batch::Tiling::TileX => (0x01FFu16, 0x0E00u16),
        batch::Tiling::Tile4 => (0x02CF, 0x0D30),
        batch::Tiling::Tile64 => (0x32CF, 0xCD30),
    };
    let (width, rows) = (u64::from(tiling.width()), u64::from(tiling.rows()));
    let tile = (y / rows) * (pitch / width) + x / width;
    tile * width * rows + spread(x % width, x_bits) + spread(y % rows, y_bits)
}

/// Puts the low bits of `value`, lowest first, at the bits `mask` has set.
fn spread(value: u64, mask: u16) -> u64 {
    let mut out = 0;
    let mut next = 0;
    for bit in 0..16 {
        if mask >> bit & 1 != 0 {
            out |= (value >> next & 1) << bit;
            next += 1;
        }
    }
    out
}

/// Names the first few unbound addresses of a kind in the log.
fn lab_says(what: &str, address: u64, vm: u32, count: u64) {
    #[cfg(feature = "lab-fake-card")]
    if count <= LAB_NAMED {
        log::warn!(
            "intel: lab card: a {what} at {address:#x} in VM {vm} has nothing bound there; a card would \
             fault ({count} so far{})",
            if count == LAB_NAMED { ", no more named" } else { "" }
        );
    }
    #[cfg(not(feature = "lab-fake-card"))]
    let _ = (what, address, vm, count);
}

/// See the module documentation. Clones share one state, so a test keeps one and gives the other
/// away.
#[derive(Debug, Clone, Default)]
pub struct FakeXe(Arc<Mutex<State>>);

impl FakeXe {
    fn state(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The lab's card. It records no calls, so it keeps no more than a guest holds, and until
    /// [`Self::stop_running`] it runs the instructions this backend writes, so the self-test
    /// passes on it as it does in the tests.
    pub fn lab() -> Self {
        let xe = Self::default();
        xe.state().lab = Some(Lab::default());
        xe
    }

    /// From now on every batch ends at once, unrun: see [`State::finish_unrun`].
    pub fn stop_running(&self) {
        self.state().lab.get_or_insert_with(Lab::default).runs_nothing = true;
    }

    /// Batches and user fences seen at unbound addresses since [`Self::stop_running`].
    pub fn unbound(&self) -> (u64, u64) {
        self.state().lab.as_ref().map_or((0, 0), |lab| (lab.unbound_batches, lab.unbound_fences))
    }

    pub fn calls(&self) -> Vec<Call> {
        self.state().calls.clone()
    }

    /// The calls of one kind, by the method's name.
    pub fn calls_named(&self, name: &str) -> Vec<Call> {
        self.state().calls.iter().filter(|call| call.name() == name).cloned().collect()
    }

    /// Fails the call with this index, counted from the first call ever made.
    pub fn fail_call(&self, index: usize, code: i32) {
        self.state().fail_at.insert(index, code);
    }

    /// Fails every call of one kind from now on, until [`FakeXe::heal`].
    pub fn fail(&self, name: &'static str, code: i32) {
        self.state().fail_named.insert(name, code);
    }

    pub fn heal(&self) {
        let mut state = self.state();
        state.fail_named.clear();
        state.fail_at.clear();
    }

    /// VMs, buffers, queues and sync objects made and not yet destroyed.
    pub fn live(&self) -> usize {
        let state = self.state();
        state.vms.len() + state.buffers.len() + state.queues.len() + state.timelines.len()
    }

    /// Holds runs back from now on: an exec is taken, and nothing of it happens until
    /// [`FakeXe::release_runs`].
    pub fn hold_runs(&self) {
        self.state().hold_runs = true;
    }

    /// Has every run from now on fault at its batch, unrun, as a job a card gives up on: the queue
    /// banned, the timeline signalled all the same, the fence unwritten.
    pub fn fault_runs(&self) {
        self.state().fault_runs = true;
    }

    pub fn release_runs(&self) {
        let mut state = self.state();
        state.hold_runs = false;
        for run in std::mem::take(&mut state.held) {
            state.finish(run);
        }
    }

    /// Ends the held runs as xe ends a hung one: the queue banned, the timeline signalled, the
    /// fence unwritten.
    pub fn lose_held_runs(&self) {
        let mut state = self.state();
        for run in std::mem::take(&mut state.held) {
            if let Some(queue) = state.queues.get_mut(&run.queue) {
                queue.banned = true;
            }
            state.finish(run);
        }
    }

    /// The addresses batches faulted at.
    pub fn faults(&self) -> Vec<u64> {
        self.state().faults.clone()
    }

    /// Lets batches set and read `registers`, which hold zero until set. No other register is
    /// reached.
    pub fn reach_registers(&self, registers: &[u32]) {
        self.state().registers.reachable.extend(registers);
    }

    /// Answers a queue keyed by binding table as a kernel before Linux v7.1 does: xe doesn't know
    /// the property, which it says as a wrong request.
    pub fn without_keying(&self) {
        self.state().keying_refused = Some(libc::EINVAL);
    }

    /// Refuses a queue keyed by binding table with `code`, and only such a queue.
    pub fn refuse_keying(&self, code: i32) {
        self.state().keying_refused = Some(code);
    }

    /// Takes a queue keyed by binding table and sets nothing for it, as a kernel whose property
    /// doesn't reach the card would.
    pub fn ignore_keying(&self) {
        self.state().keying_ignored = true;
    }

    pub fn set_names_faults(&self, names: bool) {
        self.state().names_faults = names;
    }

    /// The flags a VM was made with.
    pub fn vm_flags(&self, vm: u32) -> Option<u32> {
        self.state().vms.get(&vm).map(|vm| vm.flags)
    }

    /// Bans a queue, as xe does to one whose work it gave up on.
    pub fn ban(&self, queue: u32) {
        if let Some(queue) = self.state().queues.get_mut(&queue) {
            queue.banned = true;
        }
    }

    /// Whether anything is bound at a GPU address of a VM.
    pub fn is_bound(&self, vm: u32, address: u64) -> bool {
        self.state().vms.get(&vm).is_some_and(|vm| vm.host(address, 1).is_some())
    }
}

impl Xe for FakeXe {
    fn create_vm(&mut self, flags: u32) -> io::Result<u32> {
        let mut state = self.state();
        state.enter(Call::CreateVm { flags })?;
        let id = state.new_id();
        state.vms.insert(id, Vm { flags, bound: BTreeMap::new() });
        Ok(id)
    }

    fn destroy_vm(&mut self, vm: u32) -> io::Result<()> {
        let mut state = self.state();
        state.enter(Call::DestroyVm(vm))?;
        state.vms.remove(&vm).map(drop).ok_or_else(|| errno(libc::ENOENT))
    }

    fn create_buffer(&mut self, buffer: &NewBuffer) -> io::Result<u32> {
        let mut state = self.state();
        state.enter(Call::CreateBuffer(*buffer))?;
        if buffer.size == 0 || buffer.placement == 0 || !buffer.size.is_multiple_of(0x1000) {
            return Err(errno(libc::EINVAL));
        }
        let bytes = Bytes::new(usize::try_from(buffer.size).map_err(|_| errno(libc::ENOMEM))?)?;
        let id = state.new_id();
        state.buffers.insert(id, Arc::new(bytes));
        state.cpu_caching.insert(id, buffer.cpu_caching);
        Ok(id)
    }

    fn close_buffer(&mut self, handle: u32) -> io::Result<()> {
        let mut state = self.state();
        state.enter(Call::CloseBuffer(handle))?;
        // A map or a binding made earlier keeps its own hold on the bytes, as a real one does.
        state.cpu_caching.remove(&handle);
        state.buffers.remove(&handle).map(drop).ok_or_else(|| errno(libc::ENOENT))
    }

    fn map_buffer(&mut self, handle: u32, size: u64) -> io::Result<CpuMap> {
        let mut state = self.state();
        state.enter(Call::MapBuffer(handle))?;
        let bytes = state.buffers.get(&handle).ok_or_else(|| errno(libc::ENOENT))?;
        if size as usize > bytes.length {
            return Err(errno(libc::EINVAL));
        }
        let start = map(&bytes.file, size as usize)?;
        // SAFETY: a mapping just made, of `size` bytes, that nothing else holds.
        Ok(unsafe { CpuMap::owned(start, size as usize) })
    }

    fn export_buffer(&mut self, handle: u32) -> io::Result<OwnedFd> {
        let mut state = self.state();
        state.enter(Call::ExportBuffer(handle))?;
        let bytes = state.buffers.get(&handle).ok_or_else(|| errno(libc::ENOENT))?;
        bytes.file.try_clone()
    }

    fn bind(&mut self, vm: u32, op: &BindOp, signal: Option<Sync>) -> io::Result<()> {
        let mut state = self.state();
        state.enter(Call::Bind { vm, op: *op, signal })?;
        let aligned = [op.address, op.length].iter().all(|value| value.is_multiple_of(0x1000));
        if !aligned || op.length == 0 {
            return Err(errno(libc::EINVAL));
        }
        let host = match op.op {
            abi::BIND_MAP => {
                let bytes = state.buffers.get(&op.buffer).ok_or_else(|| errno(libc::ENOENT))?;
                let write_back = state.cpu_caching.get(&op.buffer) == Some(&abi::CPU_CACHING_WRITE_BACK);
                if write_back && !pat::coherent(op.pat_index) {
                    return Err(errno(libc::EINVAL));
                }
                let end = op.offset_or_host_address.checked_add(op.length);
                if end.is_none_or(|end| end > bytes.length as u64) {
                    return Err(errno(libc::EINVAL));
                }
                // SAFETY: the offset was just checked against the buffer.
                let host = unsafe { bytes.start.add(op.offset_or_host_address as usize) };
                Some((host, Some(Arc::clone(bytes))))
            }
            abi::BIND_MAP_USERPTR if !pat::coherent(op.pat_index) => return Err(errno(libc::EINVAL)),
            abi::BIND_MAP_USERPTR => Some((op.offset_or_host_address as *mut u8, None)),
            abi::BIND_UNMAP => None,
            _ => return Err(errno(libc::EINVAL)),
        };
        let signals = signal.map(|sync| [sync]);
        let signals = signals.as_ref().map_or(&[][..], |sync| &sync[..]);
        let flags = state.vms.get(&vm).ok_or_else(|| errno(libc::ENOENT))?.flags;
        state.check_syncs(flags, signals)?;
        let space = state.vms.get_mut(&vm).ok_or_else(|| errno(libc::ENOENT))?;
        space.clear(op.address, op.length);
        if let Some((host, buffer)) = host {
            #[cfg(feature = "lab-fake-card")]
            let large = buffer.as_ref().is_some_and(|bytes| stamps::may_be_a_slot(bytes));
            space.bound.insert(op.address, Bound { length: op.length, host, buffer });
            #[cfg(feature = "lab-fake-card")]
            if large && let Some(lab) = state.lab.as_mut() {
                lab.stamps.watch(vm);
            }
        }
        state.attach(signals);
        state.finish_signals(signals);
        if let Some(Sync::UserFence { address, value }) = signal {
            // SAFETY: a bind's user fence is an address in this process that the caller keeps alive,
            // which is what the kernel is given too.
            unsafe { (address as *mut u64).write_volatile(value) };
        }
        Ok(())
    }

    fn create_queue(&mut self, vm: u32, engine: EngineInstance, keying: Keying) -> io::Result<u32> {
        let mut state = self.state();
        state.enter(Call::CreateQueue { vm, engine, keying })?;
        if keying == Keying::BindingTableAndIndex {
            if let Some(code) = state.keying_refused {
                return Err(errno(code));
            }
            // xe's own rule: the property is for render queues only.
            if engine.class != crate::xe_query::Engine::RENDER {
                return Err(errno(libc::EOPNOTSUPP));
            }
        }
        if !state.vms.contains_key(&vm) {
            return Err(errno(libc::ENOENT));
        }
        let id = state.new_id();
        state.queues.insert(id, Queue { vm, banned: false, keying });
        Ok(id)
    }

    fn destroy_queue(&mut self, queue: u32) -> io::Result<()> {
        let mut state = self.state();
        state.enter(Call::DestroyQueue(queue))?;
        state.queues.remove(&queue).map(drop).ok_or_else(|| errno(libc::ENOENT))
    }

    fn queue_banned(&mut self, queue: u32) -> io::Result<bool> {
        let mut state = self.state();
        state.enter(Call::QueueBanned(queue))?;
        state.queues.get(&queue).map(|queue| queue.banned).ok_or_else(|| errno(libc::ENOENT))
    }

    fn exec(&mut self, queue: u32, batch: u64, syncs: &[Sync]) -> io::Result<()> {
        let mut state = self.state();
        state.enter(Call::Exec { queue, batch, syncs: syncs.to_vec() })?;
        let found = state.queues.get(&queue).ok_or_else(|| errno(libc::ENOENT))?;
        let Some(flags) = state.vms.get(&found.vm).map(|vm| vm.flags).filter(|_| !found.banned) else {
            return Err(errno(libc::ECANCELED));
        };
        state.check_syncs(flags, syncs)?;
        state.attach(syncs);
        let run = Held { queue, batch, syncs: syncs.to_vec() };
        if state.hold_runs {
            state.held.push(run);
        } else {
            state.finish(run);
        }
        Ok(())
    }

    fn create_syncobj(&mut self) -> io::Result<u32> {
        let mut state = self.state();
        state.enter(Call::CreateSyncobj)?;
        let id = state.new_id();
        state.timelines.insert(id, Timeline::default());
        Ok(id)
    }

    fn destroy_syncobj(&mut self, syncobj: u32) -> io::Result<()> {
        let mut state = self.state();
        state.enter(Call::DestroySyncobj(syncobj))?;
        state.timelines.remove(&syncobj).map(drop).ok_or_else(|| errno(libc::ENOENT))
    }

    fn point_reached(&mut self, syncobj: u32, point: u64, _patience: Duration) -> io::Result<bool> {
        let mut state = self.state();
        state.enter(Call::PointReached { syncobj, point })?;
        let timeline = state.timelines.get(&syncobj).ok_or_else(|| errno(libc::ENOENT))?;
        Ok(timeline.reached >= point)
    }

    fn engine_cycles(&mut self, engine: EngineInstance) -> io::Result<EngineCycles> {
        let mut state = self.state();
        state.enter(Call::EngineCycles(engine))?;
        state.clock_reads += 1;
        Ok(EngineCycles {
            cycles: state.clock_reads * CYCLES_PER_READ,
            width: 64,
            cpu_nanoseconds: state.clock_reads * NANOSECONDS_PER_READ,
        })
    }

    fn names_faults(&mut self) -> bool {
        let mut state = self.state();
        let _ = state.enter(Call::NamesFaults);
        state.names_faults
    }

    fn vm_faults(&mut self, vm: u32) -> io::Result<Vec<VmFault>> {
        let mut state = self.state();
        state.enter(Call::VmFaults(vm))?;
        if !state.vms.contains_key(&vm) {
            return Err(errno(libc::ENOENT));
        }
        Ok(state.vm_faults.get(&vm).cloned().unwrap_or_default())
    }
}

impl State {
    /// The lab's card ends a run at once without running it: the user fence written, the
    /// timelines signalled. A batch or fence at an address nothing is bound at would fault on a
    /// card; it is named in the log and counted, and the run still ends, so one fault doesn't hide
    /// what comes after. A ring, in a long-running VM, stays parked: nothing here moves its fence.
    fn finish_unrun(&mut self, run: Held) {
        let Some(vm_id) = self.queues.get(&run.queue).map(|queue| queue.vm) else { return };
        let Some(vm) = self.vms.get(&vm_id) else { return };
        // Before the fence: the core looks for the stamps once it has seen the fence.
        #[cfg(feature = "lab-fake-card")]
        if let Some(lab) = self.lab.as_mut() {
            lab.stamps.copy_any(vm_id, vm);
        }
        let batch_bound = vm.host(run.batch, 4).is_some();
        if vm.flags & abi::VM_LONG_RUNNING != 0 {
            if !batch_bound {
                let lab = self.lab.get_or_insert_with(Lab::default);
                lab.unbound_batches += 1;
                lab_says("ring batch", run.batch, vm_id, lab.unbound_batches);
            }
            return;
        }
        let mut fences_unbound = Vec::new();
        for sync in &run.syncs {
            if let Sync::UserFence { address, value } = *sync
                && Self::store(vm, address, value).is_err()
            {
                fences_unbound.push(address);
            }
        }
        let lab = self.lab.get_or_insert_with(Lab::default);
        if !batch_bound {
            lab.unbound_batches += 1;
            lab_says("batch", run.batch, vm_id, lab.unbound_batches);
        }
        for address in fences_unbound {
            lab.unbound_fences += 1;
            lab_says("user fence", address, vm_id, lab.unbound_fences);
        }
        self.finish_signals(&run.syncs);
    }

    /// A bind is in place as soon as the stand-in has taken it, so its point is reached at once.
    fn finish_signals(&mut self, syncs: &[Sync]) {
        for sync in syncs {
            if let Sync::Signal { timeline, point } = *sync
                && let Some(timeline) = self.timelines.get_mut(&timeline)
            {
                timeline.reached = timeline.reached.max(point);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each of gmmlib's swizzles puts every byte of a tile at its own offset inside the tile, and
    /// the tiles follow each other along the pitch, then down.
    #[test]
    fn each_tile_layout_fills_its_tile_once_and_tiles_follow_along_the_pitch() {
        for tiling in [batch::Tiling::TileX, batch::Tiling::Tile4, batch::Tiling::Tile64] {
            let (width, rows) = (u64::from(tiling.width()), u64::from(tiling.rows()));
            let mut seen = vec![false; (width * rows) as usize];
            for y in 0..rows {
                for x in 0..width {
                    let offset = surface_offset(tiling, 4 * width, x, y) as usize;
                    assert!(!std::mem::replace(&mut seen[offset], true), "{tiling:?} ({x}, {y}) twice");
                }
            }
            assert!(seen.iter().all(|&hit| hit), "{tiling:?} leaves a byte out");
            let tile = width * rows;
            assert_eq!(surface_offset(tiling, 4 * width, width, 0), tile, "{tiling:?} the next tile across");
            assert_eq!(
                surface_offset(tiling, 4 * width, 0, rows),
                4 * tile,
                "{tiling:?} the next row of tiles"
            );
        }
        // Inside a tile an OWord of a row is in order, and Tile4's second row follows its first OWord.
        assert_eq!(surface_offset(batch::Tiling::Tile4, 128, 15, 0), 15);
        assert_eq!(surface_offset(batch::Tiling::Tile4, 128, 0, 1), 16);
        assert_eq!(surface_offset(batch::Tiling::Linear, 7680, 8, 3), 3 * 7680 + 8);
    }
    use crate::xe_query::Engine;

    fn buffer() -> NewBuffer {
        NewBuffer { size: 0x1000, placement: 1, flags: 0, cpu_caching: abi::CPU_CACHING_WRITE_BACK }
    }

    fn map(buffer: u32, address: u64) -> BindOp {
        BindOp {
            op: abi::BIND_MAP,
            buffer,
            offset_or_host_address: 0,
            length: 0x1000,
            address,
            pat_index: pat::SHARED_WITH_CPU,
        }
    }

    /// A VM with a batch buffer at 0x10000 and a data buffer at 0x20000, and a queue in it.
    fn rig() -> (FakeXe, u32, u32, CpuMap, CpuMap) {
        let mut xe = FakeXe::default();
        let vm = xe.create_vm(0).unwrap();
        let (batch, data) = (xe.create_buffer(&buffer()).unwrap(), xe.create_buffer(&buffer()).unwrap());
        xe.bind(vm, &map(batch, 0x1_0000), None).unwrap();
        xe.bind(vm, &map(data, 0x2_0000), None).unwrap();
        let engine = EngineInstance { class: 0, instance: 0, gt: 0 };
        let queue = xe.create_queue(vm, engine, Keying::BindingIndex).unwrap();
        let (batch_map, data_map) =
            (xe.map_buffer(batch, 0x1000).unwrap(), xe.map_buffer(data, 0x1000).unwrap());
        (xe, vm, queue, batch_map, data_map)
    }

    fn qword_at(map: &CpuMap, offset: u64) -> u64 {
        let mut bytes = [0; 8];
        assert!(map.read(offset, &mut bytes));
        u64::from_le_bytes(bytes)
    }

    #[test]
    fn a_vm_lists_the_faults_of_its_runs_only_if_the_card_can_say_and_a_hang_adds_none() {
        let (mut xe, vm, queue, mut batch_map, _data) = rig();
        assert!(xe.vm_faults(99).is_err(), "no such VM");
        xe.set_names_faults(true);
        // A store to an address nothing is bound at.
        let store = batch::store_qword(0x9_0000_0123, 1);
        let words: Vec<u8> = store.iter().chain(&[batch::BATCH_END]).flat_map(|w| w.to_le_bytes()).collect();
        assert!(batch_map.write(0, &words));
        assert_eq!(xe.vm_faults(vm).unwrap(), [], "nothing has run");
        xe.exec(queue, 0x1_0000, &[]).unwrap();
        let listed = xe.vm_faults(vm).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!((listed[0].address, listed[0].precision), (0x9_0000_0000, 4096), "to the page");
        assert_eq!(listed[0].kind, abi::FAULT_TYPE_NOT_PRESENT);
        assert_eq!(xe.calls_named("vm_faults").len(), 3);
        // A run xe gave up on as a hang is a ban and adds nothing to the list.
        let (mut hung, vm, queue, mut batch_map, _data) = rig();
        hung.set_names_faults(true);
        assert!(batch_map.write(0, &words));
        hung.hold_runs();
        hung.exec(queue, 0x1_0000, &[]).unwrap();
        hung.lose_held_runs();
        assert!(hung.queue_banned(queue).unwrap());
        assert_eq!(hung.vm_faults(vm).unwrap(), []);
        // A card that can't say lists nothing for a fault either.
        let (mut mute, vm, queue, mut batch_map, _data) = rig();
        assert!(batch_map.write(0, &words));
        mute.exec(queue, 0x1_0000, &[]).unwrap();
        assert_eq!(mute.faults(), [0x9_0000_0123]);
        assert_eq!(mute.vm_faults(vm).unwrap(), []);
    }

    #[test]
    fn only_a_render_queue_is_keyed_by_binding_table_and_its_context_reads_so() {
        let (mut xe, vm, plain, mut batch_map, data_map) = rig();
        let engine = |class| EngineInstance { class, instance: 0, gt: 0 };
        let keyed = xe.create_queue(vm, engine(Engine::RENDER), Keying::BindingTableAndIndex).unwrap();
        for class in [Engine::COPY, Engine::COMPUTE, Engine::VIDEO_DECODE, Engine::VIDEO_ENHANCE] {
            let refused = xe.create_queue(vm, engine(class), Keying::BindingTableAndIndex).unwrap_err();
            assert_eq!(refused.raw_os_error(), Some(libc::EOPNOTSUPP), "{class}");
        }
        xe.reach_registers(&[COMMON_SLICE_CHICKEN3]);
        let read = batch::store_register(COMMON_SLICE_CHICKEN3, 0x2_0000);
        let words: Vec<u8> = read.iter().chain(&[batch::BATCH_END]).flat_map(|w| w.to_le_bytes()).collect();
        assert!(batch_map.write(0, &words));
        for (queue, bit) in [(keyed, STATE_CACHE_PERF_FIX_OFF), (plain, 0), (keyed, STATE_CACHE_PERF_FIX_OFF)]
        {
            xe.exec(queue, 0x1_0000, &[]).unwrap();
            assert_eq!(qword_at(&data_map, 0) as u32 & STATE_CACHE_PERF_FIX_OFF, bit, "queue {queue}");
        }
        assert!(xe.calls().contains(&Call::CreateQueue {
            vm,
            engine: engine(Engine::RENDER),
            keying: Keying::BindingTableAndIndex
        }));
    }

    #[test]
    fn a_kernel_without_the_keying_or_one_that_refuses_it_still_makes_plain_queues() {
        let (mut xe, vm, _queue, _batch, _data) = rig();
        let render = EngineInstance { class: Engine::RENDER, instance: 0, gt: 0 };
        xe.without_keying();
        let refused = xe.create_queue(vm, render, Keying::BindingTableAndIndex).unwrap_err();
        assert_eq!(refused.raw_os_error(), Some(libc::EINVAL), "a property it doesn't know");
        xe.refuse_keying(libc::ENOMEM);
        let refused = xe.create_queue(vm, render, Keying::BindingTableAndIndex).unwrap_err();
        assert_eq!(refused.raw_os_error(), Some(libc::ENOMEM));
        assert!(xe.create_queue(vm, render, Keying::BindingIndex).is_ok());
    }

    #[test]
    fn a_map_an_export_and_a_binding_reach_the_same_bytes_and_outlive_the_buffer() {
        let (mut xe, vm, _queue, mut batch_map, data_map) = rig();
        assert!(batch_map.write(8, &0x1234_5678_u64.to_le_bytes()));
        let exported = std::fs::File::from(xe.export_buffer(2).unwrap());
        let mut read = [0; 8];
        std::os::unix::fs::FileExt::read_exact_at(&exported, &mut read, 8).unwrap();
        assert_eq!(u64::from_le_bytes(read), 0x1234_5678, "the export is the buffer's own memfd");
        let state = xe.state();
        assert_eq!(State::read_qword(&state.vms[&vm], 0x1_0008), Ok(0x1234_5678));
        drop(state);
        xe.close_buffer(2).unwrap();
        assert_eq!(qword_at(&batch_map, 8), 0x1234_5678, "a map keeps the bytes of a closed buffer");
        assert_eq!(qword_at(&data_map, 0), 0);
    }

    #[test]
    fn the_lab_card_ends_a_batch_unrun_writes_the_fence_and_records_nothing() {
        let (xe, vm, queue, mut batch_map, data_map) = rig();
        xe.state().lab = Some(Lab::default());
        xe.stop_running();
        // A store that would change the data if it ran, and the fence beside it.
        let store = batch::store_qword(0x2_0010, 7);
        let words: Vec<u8> = store.iter().chain(&[batch::BATCH_END]).flat_map(|w| w.to_le_bytes()).collect();
        assert!(batch_map.write(0, &words));
        let calls = xe.calls().len();
        let mut runner = xe.clone();
        runner.exec(queue, 0x1_0000, &[Sync::UserFence { address: 0x2_0000, value: 5 }]).unwrap();
        assert_eq!(qword_at(&data_map, 0), 5, "the user fence is written");
        assert_eq!(qword_at(&data_map, 0x10), 0, "the batch did not run");
        assert_eq!(xe.calls().len(), calls, "the lab card records no calls");
        assert_eq!(xe.unbound(), (0, 0));
        runner.exec(queue, 0x9_0000, &[Sync::UserFence { address: 0x9_0000, value: 6 }]).unwrap();
        assert_eq!(xe.unbound(), (1, 1), "both would fault on a card and are counted");
        assert!(!runner.queue_banned(queue).unwrap(), "and the queue goes on");
        let _ = vm;
    }

    #[test]
    fn the_lab_card_leaves_a_ring_parked() {
        let mut xe = FakeXe::lab();
        xe.stop_running();
        let vm = xe.create_vm(abi::VM_LONG_RUNNING).unwrap();
        let ring = xe.create_buffer(&buffer()).unwrap();
        xe.bind(vm, &map(ring, 0x1_0000), None).unwrap();
        let queue = xe
            .create_queue(vm, EngineInstance { class: 0, instance: 0, gt: 0 }, Keying::BindingIndex)
            .unwrap();
        xe.exec(queue, 0x1_0000, &[]).unwrap();
        assert!(!xe.queue_banned(queue).unwrap());
        assert_eq!(xe.unbound(), (0, 0));
        xe.exec(queue, 0x9_0000, &[]).unwrap();
        assert_eq!(xe.unbound(), (1, 0), "a ring at an unbound address is counted too");
    }

    #[test]
    fn a_batch_of_stores_runs_and_the_fence_is_written_after_it() {
        let (mut xe, _, queue, mut batch_map, data_map) = rig();
        let mut dwords = batch::store_qword(0x2_0010, 0xAABB).to_vec();
        dwords.push(batch::BATCH_END);
        batch_map.write(0, &batch::bytes(&dwords));
        let done = xe.create_syncobj().unwrap();
        let syncs =
            [Sync::UserFence { address: 0x2_0100, value: 7 }, Sync::Signal { timeline: done, point: 1 }];
        xe.exec(queue, 0x1_0000, &syncs).unwrap();
        assert_eq!((qword_at(&data_map, 0x10), qword_at(&data_map, 0x100)), (0xAABB, 7));
        assert!(xe.point_reached(done, 1, Duration::ZERO).unwrap());
        assert!(!xe.queue_banned(queue).unwrap());
    }

    #[test]
    fn a_store_to_an_unbound_address_bans_the_queue_and_leaves_the_fence() {
        let (mut xe, _, queue, mut batch_map, data_map) = rig();
        let mut dwords = batch::store_qword(0x9_0000, 1).to_vec();
        dwords.push(batch::BATCH_END);
        batch_map.write(0, &batch::bytes(&dwords));
        let done = xe.create_syncobj().unwrap();
        let syncs =
            [Sync::UserFence { address: 0x2_0100, value: 7 }, Sync::Signal { timeline: done, point: 1 }];
        xe.exec(queue, 0x1_0000, &syncs).unwrap();
        assert_eq!(qword_at(&data_map, 0x100), 0, "the fence was never written");
        assert!(xe.point_reached(done, 1, Duration::ZERO).unwrap(), "the run still ended");
        assert!(xe.queue_banned(queue).unwrap());
        assert_eq!(xe.faults(), [0x9_0000]);
        let refused = xe.exec(queue, 0x1_0000, &syncs[1..]).unwrap_err();
        assert_eq!(refused.raw_os_error(), Some(libc::ECANCELED));
    }

    #[test]
    fn a_batch_that_jumps_to_itself_is_a_hang() {
        let (mut xe, _, queue, mut batch_map, _) = rig();
        batch_map.write(0, &batch::bytes(&batch::batch_start(0x1_0000)));
        let done = xe.create_syncobj().unwrap();
        xe.exec(queue, 0x1_0000, &[Sync::Signal { timeline: done, point: 1 }]).unwrap();
        assert!(xe.queue_banned(queue).unwrap());
        assert!(xe.faults().is_empty());
    }

    #[test]
    fn the_kernels_rules_for_syncs_are_kept() {
        let (mut xe, _, queue, mut batch_map, _) = rig();
        batch_map.write(0, &batch::bytes(&[batch::BATCH_END]));
        let done = xe.create_syncobj().unwrap();
        let signal = Sync::Signal { timeline: done, point: 1 };
        let invalid = |result: io::Result<()>| result.unwrap_err().raw_os_error() == Some(libc::EINVAL);
        let fence = |address| Sync::UserFence { address, value: 1 };
        assert!(invalid(xe.exec(queue, 0x1_0000, &[fence(0x2_0004), signal])), "not 8-byte aligned");
        assert!(invalid(xe.exec(queue, 0x1_0000, &[fence(0x2_0000), fence(0x2_0008), signal])), "two");
        let binds = xe.create_syncobj().unwrap();
        let after = Sync::After { timeline: binds, point: 1 };
        assert!(invalid(xe.exec(queue, 0x1_0000, &[after, signal])), "nothing attached to wait for");
        assert!(xe.exec(queue, 0x1_0000, &[signal]).is_ok());
    }

    /// xe binds a write-back buffer or host pages only with a page-table entry coherent with the
    /// CPU, and a write-combined buffer with any.
    #[test]
    fn a_write_back_buffer_or_host_pages_need_a_coherent_entry() {
        let mut xe = FakeXe::default();
        let vm = xe.create_vm(0).unwrap();
        let invalid = |result: io::Result<()>| result.unwrap_err().raw_os_error() == Some(libc::EINVAL);
        let write_back = xe.create_buffer(&buffer()).unwrap();
        let card = BindOp { pat_index: pat::CARD, ..map(write_back, 0x1_0000) };
        assert!(invalid(xe.bind(vm, &card, None)), "no coherency on a write-back buffer");
        xe.bind(vm, &map(write_back, 0x1_0000), None).unwrap();
        let combined = NewBuffer { cpu_caching: abi::CPU_CACHING_WRITE_COMBINED, ..buffer() };
        let combined = xe.create_buffer(&combined).unwrap();
        xe.bind(vm, &BindOp { pat_index: pat::CARD, ..map(combined, 0x2_0000) }, None).unwrap();
        let pages = Box::new([0u8; 0x1000]);
        let host = BindOp {
            op: abi::BIND_MAP_USERPTR,
            buffer: 0,
            offset_or_host_address: pages.as_ptr() as u64,
            pat_index: pat::CARD,
            ..map(0, 0x3_0000)
        };
        assert!(invalid(xe.bind(vm, &host, None)), "no coherency on host pages");
        xe.bind(vm, &BindOp { pat_index: pat::SHARED_WITH_CPU, ..host }, None).unwrap();
    }

    #[test]
    fn a_long_running_vm_takes_no_signals_and_lets_a_parked_batch_be() {
        let mut xe = FakeXe::default();
        let vm = xe.create_vm(abi::VM_LONG_RUNNING).unwrap();
        let handle = xe.create_buffer(&buffer()).unwrap();
        let timeline = xe.create_syncobj().unwrap();
        let signal = Sync::Signal { timeline, point: 1 };
        let unsupported =
            |result: io::Result<()>| result.unwrap_err().raw_os_error() == Some(libc::EOPNOTSUPP);
        assert!(unsupported(xe.bind(vm, &map(handle, 0x1_0000), Some(signal))));
        let bound = Box::new(0u64);
        let fence = Sync::UserFence { address: std::ptr::from_ref(&*bound) as u64, value: 3 };
        xe.bind(vm, &map(handle, 0x1_0000), Some(fence)).unwrap();
        // SAFETY: the box is alive, and the stand-in wrote through the same address.
        assert_eq!(
            unsafe { std::ptr::from_ref(&*bound).read_volatile() },
            3,
            "written once the bind is in place"
        );
        let queue = xe
            .create_queue(vm, EngineInstance { class: 4, instance: 0, gt: 0 }, Keying::BindingIndex)
            .unwrap();
        let mut batch_map = xe.map_buffer(handle, 0x1000).unwrap();
        batch_map.write(0, &batch::bytes(&[batch::SEMAPHORE_WAIT_HEADER, 1, 0x1_0800, 0, 0]));
        assert!(unsupported(xe.exec(queue, 0x1_0000, &[signal])));
        xe.exec(queue, 0x1_0000, &[]).unwrap();
        assert!(!xe.queue_banned(queue).unwrap(), "a ring may park for as long as it likes here");
        assert_eq!(xe.vm_flags(vm), Some(abi::VM_LONG_RUNNING));
        xe.ban(queue);
        assert!(xe.queue_banned(queue).unwrap());
    }

    #[test]
    fn held_runs_do_nothing_until_they_are_let_through_or_lost() {
        let (mut xe, _, queue, mut batch_map, data_map) = rig();
        batch_map.write(0, &batch::bytes(&[batch::BATCH_END]));
        let done = xe.create_syncobj().unwrap();
        let syncs = |point| {
            [Sync::UserFence { address: 0x2_0000, value: point }, Sync::Signal { timeline: done, point }]
        };
        xe.hold_runs();
        xe.exec(queue, 0x1_0000, &syncs(1)).unwrap();
        assert!(!xe.point_reached(done, 1, Duration::ZERO).unwrap());
        xe.release_runs();
        assert!(xe.point_reached(done, 1, Duration::ZERO).unwrap());
        assert_eq!(qword_at(&data_map, 0), 1);
        xe.hold_runs();
        xe.exec(queue, 0x1_0000, &syncs(2)).unwrap();
        xe.lose_held_runs();
        assert!(xe.point_reached(done, 2, Duration::ZERO).unwrap());
        assert_eq!(qword_at(&data_map, 0), 1, "a lost run writes no fence");
        assert!(xe.queue_banned(queue).unwrap());
    }

    #[test]
    fn an_unbind_takes_only_its_own_range() {
        let mut xe = FakeXe::default();
        let vm = xe.create_vm(0).unwrap();
        let big = NewBuffer { size: 0x3000, ..buffer() };
        let handle = xe.create_buffer(&big).unwrap();
        xe.bind(vm, &BindOp { length: 0x3000, ..map(handle, 0x1_0000) }, None).unwrap();
        let unbind = BindOp { op: abi::BIND_UNMAP, buffer: 0, ..map(0, 0x1_1000) };
        xe.bind(vm, &unbind, None).unwrap();
        assert!(xe.is_bound(vm, 0x1_0000) && !xe.is_bound(vm, 0x1_1000) && xe.is_bound(vm, 0x1_2000));
    }

    #[test]
    fn any_call_can_be_failed_and_everything_made_is_counted() {
        let mut xe = FakeXe::default();
        xe.fail_call(1, libc::ENOMEM);
        let vm = xe.create_vm(0).unwrap();
        assert_eq!(xe.create_buffer(&buffer()).unwrap_err().raw_os_error(), Some(libc::ENOMEM));
        let handle = xe.create_buffer(&buffer()).unwrap();
        assert_eq!(xe.live(), 2);
        xe.fail("close_buffer", libc::EBUSY);
        assert!(xe.close_buffer(handle).is_err());
        xe.heal();
        xe.close_buffer(handle).unwrap();
        xe.destroy_vm(vm).unwrap();
        assert_eq!(xe.live(), 0);
        assert_eq!(xe.calls_named("create_buffer").len(), 2);
    }
}
