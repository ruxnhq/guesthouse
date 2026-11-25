//! What host pages on a submission's buffer list cost each submission.
//!
//! amdgpu looks up every page of a buffer made from host pages each time a submission lists it,
//! whether the submission touches the buffer or not. The daemon maps the guest's fence pages into
//! every address space as such buffers, so they are on every list. This sends the same tiny job
//! many times and times the submission call alone, with no host pages listed, with the whole fence
//! area as one buffer, and with only so many pages of it listed in pieces, as the daemon lists them.
//!
//! Run it with no arguments to use the one AMD card, or pass a PCI address to pick one of several.

use guesthouse_backend_amd::{
    Context, CpuAccess, DOMAIN_GTT, Device, GemHandle, HW_IP_DMA, Ib, MapFlags, MemoryType, Syncobj,
    adopt_buffer, close_buffer, create_buffer, destroy_buffer_list, make_buffer_list, map_buffer,
    sdma_write_ib, submit_all, unmap_buffer, wait_eventfd,
};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::process::ExitCode;
use std::time::Instant;

const PAGE: u64 = 4096;
/// The daemon's fence area and the size of the pieces it lists: `guesthouse_core::FENCE_AREA` and
/// the AMD backend's `FENCE_PIECE`.
const FENCE_AREA: u64 = 16 << 20;
const PIECE: u64 = 1 << 20;
const SUBMISSIONS: usize = 2000;
const WAIT_MS: i32 = 2000;
const MAGIC: u32 = 0xCAFE_D00D;
/// Where the buffers go, above the card's first address.
const JOB_AT: u64 = 0x100_000;
const PAGES_AT: u64 = 0x4000_0000;

pub fn main() -> ExitCode {
    let address = std::env::args().nth(1);
    match run(address.as_deref()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("amd-list-cost failed: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Shared memory of the fence area's size, as the guest's is, written once so every page is there.
struct HostPages(*mut u8);

impl HostPages {
    fn new() -> Result<Self, String> {
        let failed = |what: &str| format!("{what}: {}", std::io::Error::last_os_error());
        // SAFETY: a new file of our own, and its descriptor is checked before use.
        let file = unsafe { libc::memfd_create(c"amd-list-cost".as_ptr(), libc::MFD_CLOEXEC) };
        if file < 0 {
            return Err(failed("no file for the pages"));
        }
        // SAFETY: the descriptor just made, which this owns from here.
        let file = unsafe { OwnedFd::from_raw_fd(file) };
        // SAFETY: sizing our own file.
        if unsafe { libc::ftruncate(file.as_raw_fd(), FENCE_AREA as libc::off_t) } != 0 {
            return Err(failed("sizing the pages"));
        }
        // SAFETY: a fresh shared mapping of our own file, checked below.
        let pages = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                FENCE_AREA as usize,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if pages == libc::MAP_FAILED {
            return Err(failed("mapping the pages"));
        }
        // SAFETY: the mapping is ours, writable and this long.
        unsafe { std::ptr::write_bytes(pages.cast::<u8>(), 1, FENCE_AREA as usize) };
        Ok(Self(pages.cast()))
    }

    fn address(&self) -> u64 {
        self.0 as u64
    }
}

impl Drop for HostPages {
    fn drop(&mut self) {
        // SAFETY: the mapping made in `new`, with nothing left that points into it.
        unsafe { libc::munmap(self.0.cast(), FENCE_AREA as usize) };
    }
}

/// The job every submission sends, and the two buffers it needs.
struct Job {
    ib: Ib,
    buffers: [GemHandle; 2],
}

fn run(address: Option<&str>) -> Result<(), String> {
    let device = Device::open(address).map_err(|error| format!("opening the AMD card: {error}"))?;
    let info = device.info().clone();
    println!(
        "card            device {:#06x} rev {} family {}",
        info.device_id, info.pci_revision, info.family
    );
    if !info.engines.dma {
        return Err("this card reports no SDMA ring, which the job here runs on".into());
    }
    let job = job(&device, info.va_start, info.va_alignment.max(PAGE))?;
    let context = Context::create(&device).map_err(with("creating a context"))?;
    let pages = HostPages::new()?;
    let at = info.va_start + PAGES_AT;

    println!("\n{SUBMISSIONS} submissions each, the submission call alone, in microseconds");
    println!("{:<44} {:>8} {:>8} {:>8}", "host pages on the list", "median", "mean", "p90");
    let none = time(&device, &context, &job, &pages, at, &[])?;
    report("none", none);
    let whole = time(&device, &context, &job, &pages, at, &[FENCE_AREA])?;
    report("the whole area, one buffer of 16 MiB", whole);
    for in_use in [1, 64, 256, 1024, FENCE_AREA / PAGE] {
        let pieces = (in_use * PAGE).div_ceil(PIECE);
        let timed = time(&device, &context, &job, &pages, at, &vec![PIECE; pieces as usize])?;
        report(&format!("{in_use} pages in use, {pieces} pieces of 1 MiB"), timed);
    }

    context.destroy(&device);
    for (buffer, address) in job.buffers.into_iter().zip([info.va_start + JOB_AT, info.va_start + 2 * JOB_AT])
    {
        let _ = unmap_buffer(&device, buffer, address, PAGE);
        close_buffer(&device, buffer);
    }
    Ok(())
}

/// A command buffer that stores one word in a second buffer, both mapped on the card.
fn job(device: &Device, va_start: u64, align: u64) -> Result<Job, String> {
    let make = || create_buffer(device, PAGE, align, DOMAIN_GTT, CpuAccess::Required);
    let commands = make().map_err(with("creating the command buffer"))?;
    let result = make().map_err(with("creating the result buffer"))?;
    let (commands_at, result_at) = (va_start + JOB_AT, va_start + 2 * JOB_AT);
    let flags =
        MapFlags { readable: true, writable: true, executable: true, memory_type: MemoryType::Uncached };
    map_buffer(device, commands, commands_at, 0, PAGE, flags).map_err(with("mapping the command buffer"))?;
    map_buffer(device, result, result_at, 0, PAGE, flags).map_err(with("mapping the result buffer"))?;
    let dwords = sdma_write_ib(result_at, &[MAGIC]);
    device
        .map_cpu(commands, PAGE as usize)
        .map_err(with("reaching the command buffer"))?
        .write_dwords(&dwords);
    let ib = Ib { address: commands_at, bytes: (dwords.len() * 4) as u32 };
    Ok(Job { ib, buffers: [commands, result] })
}

/// Sends the job [`SUBMISSIONS`] times with `pieces` of the host pages on the list, one after
/// another from the start of the pages, and returns the sorted times of the submission call.
fn time(
    device: &Device,
    context: &Context,
    job: &Job,
    pages: &HostPages,
    at: u64,
    pieces: &[u64],
) -> Result<Vec<f64>, String> {
    let flags =
        MapFlags { readable: true, writable: true, executable: false, memory_type: MemoryType::Uncached };
    let mut adopted = Vec::new();
    let mut offset = 0;
    for &size in pieces {
        let buffer = adopt_buffer(device, pages.address() + offset, size, false)
            .map_err(with("adopting host pages"))?;
        map_buffer(device, buffer, at + offset, 0, size, flags).map_err(with("mapping host pages"))?;
        adopted.push((buffer, at + offset, size));
        offset += size;
    }
    let mut listed = job.buffers.to_vec();
    listed.extend(adopted.iter().map(|&(buffer, ..)| buffer));
    let list = make_buffer_list(device, &listed, None).map_err(with("making the buffer list"))?;

    let mut times = Vec::with_capacity(SUBMISSIONS);
    let mut sent = Ok(());
    for _ in 0..SUBMISSIONS {
        sent = send(device, context, job, list).map(|taken| times.push(taken));
        if sent.is_err() {
            break;
        }
    }
    let _ = destroy_buffer_list(device, list);
    for (buffer, address, size) in adopted {
        let _ = unmap_buffer(device, buffer, address, size);
        close_buffer(device, buffer);
    }
    sent?;
    times.sort_by(f64::total_cmp);
    Ok(times)
}

/// One submission, waited for, and how long the submission call took in microseconds.
fn send(device: &Device, context: &Context, job: &Job, list: u64) -> Result<f64, String> {
    let signal = Syncobj::create(device).map_err(with("creating a sync object"))?;
    let start = Instant::now();
    let submitted = submit_all(device, context, HW_IP_DMA, &[job.ib], &[], Some(list), &signal);
    let taken = start.elapsed().as_secs_f64() * 1e6;
    let done =
        submitted.and_then(|_| signal.arm_eventfd(device)).and_then(|event| wait_eventfd(&event, WAIT_MS));
    signal.destroy(device);
    match done {
        Ok(true) => Ok(taken),
        Ok(false) => Err("a submission did not complete before the timeout".into()),
        Err(error) => Err(format!("submitting: {error}")),
    }
}

fn report(what: &str, sorted: Vec<f64>) {
    let mean = sorted.iter().sum::<f64>() / sorted.len() as f64;
    let (median, p90) = (sorted[sorted.len() / 2], sorted[sorted.len() * 9 / 10]);
    println!("{what:<44} {median:>8.1} {mean:>8.1} {p90:>8.1}");
}

fn with(what: &'static str) -> impl Fn(std::io::Error) -> String {
    move |error| format!("{what}: {error}")
}
