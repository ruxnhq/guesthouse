//! The thin layer that calls the kernel: one method per xe call, behind [`Xe`], so everything above
//! it runs the same against a card and against the stand-in the tests use.
//!
//! [`XeCard`] is the real one. It packs each request with [`super::abi`], makes the call, and reads
//! the answer. It decides nothing.

use super::abi::{self, BindOp, EngineInstance, Keying, NewBuffer, Sync, VmFault};
use crate::ioctl::drm_ioctl;
use crate::protocol::bytes::{dword, qword};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;
use std::time::Duration;

/// An engine's cycle counter and the CPU's monotonic clock, read together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineCycles {
    pub cycles: u64,
    /// How many bits of `cycles` count.
    pub width: u32,
    pub cpu_nanoseconds: u64,
}

/// What xe can be asked to do. Ids are xe's own: a VM, a buffer handle, an exec queue, a sync object.
pub trait Xe {
    fn create_vm(&mut self, flags: u32) -> io::Result<u32>;
    fn destroy_vm(&mut self, vm: u32) -> io::Result<()>;
    fn create_buffer(&mut self, buffer: &NewBuffer) -> io::Result<u32>;
    fn close_buffer(&mut self, handle: u32) -> io::Result<()>;
    /// Maps a buffer for the CPU.
    fn map_buffer(&mut self, handle: u32, size: u64) -> io::Result<CpuMap>;
    /// Hands a buffer out as a file another process can import.
    fn export_buffer(&mut self, handle: u32) -> io::Result<OwnedFd>;
    /// Changes a VM's mappings. With `signal` given the call returns at once and that point is
    /// signalled when the change is in place.
    fn bind(&mut self, vm: u32, op: &BindOp, signal: Option<Sync>) -> io::Result<()>;
    /// Makes a queue of one engine in a VM. [`Keying::BindingTableAndIndex`] is for a render
    /// engine only, and a kernel before Linux v7.1 refuses it.
    fn create_queue(&mut self, vm: u32, engine: EngineInstance, keying: Keying) -> io::Result<u32>;
    fn destroy_queue(&mut self, queue: u32) -> io::Result<()>;
    /// Whether xe has banned a queue, which it does to one whose work hung or faulted.
    fn queue_banned(&mut self, queue: u32) -> io::Result<bool>;
    /// Sends the batch at a GPU address in the queue's VM.
    fn exec(&mut self, queue: u32, batch: u64, syncs: &[Sync]) -> io::Result<()>;
    fn create_syncobj(&mut self) -> io::Result<u32>;
    fn destroy_syncobj(&mut self, syncobj: u32) -> io::Result<()>;
    /// Whether a timeline has reached a point, waiting up to `patience` for it. With no patience
    /// it only looks.
    fn point_reached(&mut self, syncobj: u32, point: u64, patience: Duration) -> io::Result<bool>;
    fn engine_cycles(&mut self, engine: EngineInstance) -> io::Result<EngineCycles>;
    /// Whether this kernel can say which address a VM faulted at. Newer than the rest, and nothing
    /// depends on it.
    fn names_faults(&mut self) -> bool;
    /// The faults xe has saved for a VM, oldest first. What an answer means, from Linux v7.2
    /// (`xe_vm.c`, `xe_pagefault.c`, `xe_drm.h`), where "read" is in that source and "inferred" is
    /// not stated there:
    ///
    /// - Read: the only code that adds to the list is the page fault worker, after it failed to
    ///   service a fault the GPU reported (`xe_pagefault_save_to_vm`). A hang or a job timeout adds
    ///   nothing, so after a plain hang the list is empty and an empty list does not mean the queue
    ///   was banned for nothing. (Checked in the files named, not in the whole tree.)
    /// - Read: a VM that is not in fault mode is still found and its fault saved. The source says
    ///   so in a comment, and the service step refuses such a VM unless it has a scratch page, so
    ///   a fault in a VM made without either lands in the list.
    /// - Read: the worker exists only where the device has unified shared memory support
    ///   (`xe_pagefault_init` returns early without it), so on a card without it the list is always
    ///   empty, fault or not. Entries of reserved engines are left out.
    /// - Read: each entry is exact to 4 KiB, the list keeps at most 50 entries and drops later
    ///   ones without a word, and it is emptied only when the VM is freed, so it can hold faults
    ///   from before the run asked about.
    /// - Inferred: the entry is saved by a worker after the GPU's report, so a list asked for
    ///   straight after a ban can still be short by one. And that the Arc B580 reports page faults
    ///   to xe for a VM that is not in fault mode, which depends on its firmware.
    fn vm_faults(&mut self, vm: u32) -> io::Result<Vec<VmFault>>;
}

/// A buffer's bytes as the CPU sees them. Reads and writes are checked against its length.
#[derive(Debug)]
pub struct CpuMap {
    start: *mut u8,
    length: usize,
    /// Whether dropping this unmaps the bytes. The stand-in's maps borrow memory it keeps.
    owned: bool,
}

// SAFETY: the mapping is plain memory owned by this value (or kept alive by the stand-in that lent
// it), with no thread affinity.
unsafe impl Send for CpuMap {}

impl CpuMap {
    /// A map over memory someone else keeps alive.
    ///
    /// # Safety
    ///
    /// `start` must point at `length` readable and writable bytes that outlive the map.
    pub unsafe fn borrowed(start: *mut u8, length: usize) -> Self {
        Self { start, length, owned: false }
    }

    /// A map that unmaps its bytes when dropped.
    ///
    /// # Safety
    ///
    /// `start` and `length` must be a shared mapping made by mmap that nothing else unmaps.
    #[cfg(any(test, feature = "lab-fake-card"))]
    pub(crate) unsafe fn owned(start: *mut u8, length: usize) -> Self {
        Self { start, length, owned: true }
    }

    pub fn len(&self) -> usize {
        self.length
    }

    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    fn range(&self, offset: u64, length: usize) -> Option<usize> {
        let offset = usize::try_from(offset).ok()?;
        (offset.checked_add(length)? <= self.length).then_some(offset)
    }

    /// Reads at an offset. False, with nothing read, if any of it lies past the end.
    pub fn read(&self, offset: u64, out: &mut [u8]) -> bool {
        let Some(offset) = self.range(offset, out.len()) else { return false };
        // SAFETY: the range was checked against the mapping's length, and `out` is a separate
        // buffer of the same length.
        unsafe { std::ptr::copy_nonoverlapping(self.start.add(offset), out.as_mut_ptr(), out.len()) };
        true
    }

    /// Writes at an offset. False, with nothing written, if any of it lies past the end.
    pub fn write(&mut self, offset: u64, bytes: &[u8]) -> bool {
        let Some(offset) = self.range(offset, bytes.len()) else { return false };
        // SAFETY: as in `read`, the other way.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.start.add(offset), bytes.len()) };
        true
    }

    /// Changes the first and third byte of each four over, in place, as red and blue in a picture
    /// of 32-bit pixels. A last part shorter than four is left alone. False, with nothing changed,
    /// if any of it lies past the end.
    pub fn turn_red_and_blue(&mut self, offset: u64, length: usize) -> bool {
        let Some(offset) = self.range(offset, length) else { return false };
        // SAFETY: the range was checked against the mapping's length. The pixels are read and
        // written through the pointer, never as a reference, as the guest shares the memory.
        unsafe {
            let first = self.start.add(offset).cast::<u32>();
            for index in 0..length / 4 {
                let at = first.add(index);
                let pixel = at.read_unaligned();
                at.write_unaligned((pixel & 0xFF00_FF00) | ((pixel >> 16) & 0xFF) | ((pixel & 0xFF) << 16));
            }
        }
        true
    }
}

impl Drop for CpuMap {
    fn drop(&mut self) {
        if self.owned {
            // SAFETY: the mapping was made by mmap with this start and length, and nothing else
            // holds it.
            unsafe { libc::munmap(self.start.cast(), self.length) };
        }
    }
}

/// An Intel card's render node, opened for work.
#[derive(Debug)]
pub struct XeCard {
    file: File,
}

impl XeCard {
    pub fn open(device_file: &Path) -> io::Result<Self> {
        Ok(Self { file: File::options().read(true).write(true).open(device_file)? })
    }

    /// A call whose request holds no pointer, or only pointers the caller keeps alive across it.
    fn call(&self, number: u64, request: &mut [u8]) -> io::Result<()> {
        // SAFETY: every request built in this file points only at locals that live until the call
        // returns, each as long as the request says.
        unsafe { drm_ioctl(&self.file, number, request) }
    }
}

fn answer(request: &[u8], at: usize) -> u32 {
    dword(request, at).unwrap_or(0)
}

/// The monotonic clock's time `patience` from now, in nanoseconds, which is how a sync object
/// wait takes its timeout. Zero patience is zero, which the kernel reads as "only look".
fn deadline(patience: Duration) -> i64 {
    if patience.is_zero() {
        return 0;
    }
    let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: the pointer is to a local timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
    let now = now.tv_sec.saturating_mul(1_000_000_000).saturating_add(now.tv_nsec);
    now.saturating_add(i64::try_from(patience.as_nanos()).unwrap_or(i64::MAX))
}

impl Xe for XeCard {
    fn create_vm(&mut self, flags: u32) -> io::Result<u32> {
        let mut request = abi::vm::create(flags);
        self.call(abi::VM_CREATE, &mut request)?;
        Ok(answer(&request, abi::vm::CREATED_ID))
    }

    fn destroy_vm(&mut self, vm: u32) -> io::Result<()> {
        self.call(abi::VM_DESTROY, &mut abi::vm::destroy(vm))
    }

    fn create_buffer(&mut self, buffer: &NewBuffer) -> io::Result<u32> {
        let mut request = buffer.request();
        self.call(abi::GEM_CREATE, &mut request)?;
        Ok(answer(&request, NewBuffer::HANDLE))
    }

    fn close_buffer(&mut self, handle: u32) -> io::Result<()> {
        self.call(abi::GEM_CLOSE, &mut abi::close_buffer(handle))
    }

    fn map_buffer(&mut self, handle: u32, size: u64) -> io::Result<CpuMap> {
        let mut request = abi::mmap_offset::request(handle);
        self.call(abi::GEM_MMAP_OFFSET, &mut request)?;
        let offset = qword(&request, abi::mmap_offset::OFFSET).unwrap_or(0);
        let length = usize::try_from(size).map_err(|_| io::ErrorKind::InvalidInput)?;
        let offset = libc::off_t::try_from(offset).map_err(|_| io::ErrorKind::InvalidInput)?;
        // SAFETY: a fresh shared mapping of the node at the offset xe gave for this buffer. Nothing
        // else in the process is touched.
        let start = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                length,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                self.file.as_raw_fd(),
                offset,
            )
        };
        if start == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(CpuMap { start: start.cast(), length, owned: true })
    }

    fn export_buffer(&mut self, handle: u32) -> io::Result<OwnedFd> {
        let mut request = abi::export::request(handle);
        self.call(abi::PRIME_HANDLE_TO_FD, &mut request)?;
        let file = answer(&request, abi::export::FILE) as i32;
        // SAFETY: the kernel just made this descriptor for us, and nothing else owns it.
        Ok(unsafe { OwnedFd::from_raw_fd(file) })
    }

    fn bind(&mut self, vm: u32, op: &BindOp, signal: Option<Sync>) -> io::Result<()> {
        let mut sync = signal.map(|sync| sync.bytes());
        let (address, count) = match sync.as_mut() {
            Some(bytes) => (bytes.as_mut_ptr() as u64, 1),
            None => (0, 0),
        };
        self.call(abi::VM_BIND, &mut abi::bind::request(vm, op, address, count))
    }

    fn create_queue(&mut self, vm: u32, engine: EngineInstance, keying: Keying) -> io::Result<u32> {
        let mut instance = engine.bytes();
        let mut extension = abi::queue::keyed_by_table();
        let extensions = match keying {
            Keying::BindingIndex => 0,
            Keying::BindingTableAndIndex => extension.as_mut_ptr() as u64,
        };
        let mut request = abi::queue::create(vm, instance.as_mut_ptr() as u64, extensions);
        self.call(abi::EXEC_QUEUE_CREATE, &mut request)?;
        Ok(answer(&request, abi::queue::CREATED_ID))
    }

    fn destroy_queue(&mut self, queue: u32) -> io::Result<()> {
        self.call(abi::EXEC_QUEUE_DESTROY, &mut abi::queue::destroy(queue))
    }

    fn queue_banned(&mut self, queue: u32) -> io::Result<bool> {
        let mut request = abi::queue::property(queue, abi::QUEUE_PROPERTY_BAN);
        self.call(abi::EXEC_QUEUE_GET_PROPERTY, &mut request)?;
        Ok(qword(&request, abi::queue::PROPERTY_VALUE).unwrap_or(0) != 0)
    }

    fn exec(&mut self, queue: u32, batch: u64, syncs: &[Sync]) -> io::Result<()> {
        let mut array = Sync::array(syncs);
        let mut request = abi::exec::request(queue, batch, array.as_mut_ptr() as u64, syncs.len() as u32);
        self.call(abi::EXEC, &mut request)
    }

    fn create_syncobj(&mut self) -> io::Result<u32> {
        let mut request = abi::syncobj::create();
        self.call(abi::SYNCOBJ_CREATE, &mut request)?;
        Ok(answer(&request, abi::syncobj::HANDLE))
    }

    fn destroy_syncobj(&mut self, syncobj: u32) -> io::Result<()> {
        self.call(abi::SYNCOBJ_DESTROY, &mut abi::syncobj::destroy(syncobj))
    }

    fn point_reached(&mut self, syncobj: u32, point: u64, patience: Duration) -> io::Result<bool> {
        let (mut handle, mut point) = (syncobj, point);
        let mut request = abi::syncobj::wait(
            std::ptr::from_mut(&mut handle) as u64,
            std::ptr::from_mut(&mut point) as u64,
            deadline(patience),
        );
        match self.call(abi::SYNCOBJ_TIMELINE_WAIT, &mut request) {
            Ok(()) => Ok(true),
            // The wait's own way of saying the time ran out.
            Err(error) if error.raw_os_error() == Some(libc::ETIME) => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn engine_cycles(&mut self, engine: EngineInstance) -> io::Result<EngineCycles> {
        let mut answer = abi::engine_cycles::request(engine);
        crate::xe_query::ask_into(&self.file, abi::QUERY_ENGINE_CYCLES, &mut answer)?;
        Ok(EngineCycles {
            cycles: qword(&answer, abi::engine_cycles::CYCLES).unwrap_or(0),
            width: dword(&answer, abi::engine_cycles::WIDTH).unwrap_or(0),
            cpu_nanoseconds: qword(&answer, abi::engine_cycles::CPU_TIME).unwrap_or(0),
        })
    }

    fn names_faults(&mut self) -> bool {
        // Mesa's probe: a kernel with the call looks for the VM and finds none, and one without
        // doesn't know the call.
        let asked = self.call(abi::VM_GET_PROPERTY, &mut abi::vm_property::probe());
        asked.is_err_and(|error| error.raw_os_error() == Some(libc::ENOENT))
    }

    fn vm_faults(&mut self, vm: u32) -> io::Result<Vec<VmFault>> {
        // First the size the list takes, then the list. It can only grow between the two, and the
        // kernel copies what is asked for if that is no more than it holds.
        let mut request = abi::vm_property::faults(vm, 0, 0);
        self.call(abi::VM_GET_PROPERTY, &mut request)?;
        let size = abi::vm_property::size_of_answer(&request);
        if size == 0 {
            return Ok(Vec::new());
        }
        let mut list = vec![0u8; size as usize];
        let mut request = abi::vm_property::faults(vm, size, list.as_mut_ptr() as u64);
        self.call(abi::VM_GET_PROPERTY, &mut request)?;
        Ok(VmFault::parse(&list))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn red_and_blue_change_over_in_place_and_nothing_else_does() {
        let mut bytes: Vec<u8> = (0..20).collect();
        // SAFETY: the vector outlives the map and is as long as given.
        let mut map = unsafe { CpuMap::borrowed(bytes.as_mut_ptr(), bytes.len()) };
        // Two whole pixels from the second byte, and one byte that isn't a pixel.
        assert!(map.turn_red_and_blue(1, 9));
        assert!(!map.turn_red_and_blue(12, 9), "past the end");
        drop(map);
        assert_eq!(bytes, [0, 3, 2, 1, 4, 7, 6, 5, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19]);
    }

    #[test]
    fn a_map_refuses_what_lies_past_its_end() {
        let mut bytes = vec![0u8; 16];
        // SAFETY: the vector outlives the map and is as long as given.
        let mut map = unsafe { CpuMap::borrowed(bytes.as_mut_ptr(), bytes.len()) };
        assert!(map.write(8, &[1; 8]));
        assert!(!map.write(9, &[2; 8]), "one byte too far");
        assert!(!map.write(u64::MAX, &[2]));
        let mut out = [0; 8];
        assert!(map.read(8, &mut out) && out == [1; 8]);
        assert!(!map.read(12, &mut out));
        assert_eq!(map.len(), 16);
    }

    #[test]
    fn no_patience_is_only_a_look_and_some_is_a_time_ahead() {
        assert_eq!(deadline(Duration::ZERO), 0);
        let soon = deadline(Duration::from_secs(2));
        assert!(soon > deadline(Duration::from_millis(1)) && soon > 2_000_000_000);
    }

    #[test]
    fn a_file_that_is_no_card_refuses_every_call_and_nothing_breaks() {
        let mut card = XeCard::open(Path::new("/dev/null")).unwrap();
        assert!(card.create_vm(0).is_err());
        let buffer = NewBuffer { size: 0x1000, placement: 1, flags: 0, cpu_caching: 1 };
        assert!(card.create_buffer(&buffer).is_err());
        assert!(card.exec(1, 0x1000, &[Sync::Signal { timeline: 1, point: 1 }]).is_err());
        assert!(card.point_reached(1, 1, Duration::ZERO).is_err());
        assert!(!card.names_faults());
        assert!(card.vm_faults(1).is_err());
    }
}
