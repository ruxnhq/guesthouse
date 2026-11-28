//! The vendor's management library, through its documented C API.
//!
//! It is loaded when the card's sensors publisher starts and never linked, so a machine without it only goes without
//! readings. Each getter is looked up alone, so an older library loses that reading and no more.

use super::processes::{ProcessMemory, ProcessSource, Sample, Samples};
use super::sampler::{FastReadings, SensorSource, SlowReadings};
use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::mem;
use std::ptr;

pub(super) const LIBRARY_NAME: &CStr = c"libnvidia-ml.so.1";

const SUCCESS: c_int = 0;
/// What the per-process samples answer when the driver has none newer than asked for.
const NOT_FOUND: c_int = 6;
/// The buffer was too small, and the count now says how many there are.
const INSUFFICIENT_SIZE: c_int = 7;
/// The library's word for a figure it doesn't have, as in a process's memory.
const VALUE_NOT_AVAILABLE: u64 = u64::MAX;
/// A list longer than this is not believed: a card holds a few dozen processes, not thousands.
const MOST_ENTRIES: usize = 1024;
/// Room for this many at first, which a desktop and a few guests stay well inside.
const FIRST_ROOM: usize = 64;
/// From the API's enumerations.
const SENSOR_GPU: c_int = 0;
const CLOCK_GRAPHICS: c_int = 0;
const CLOCK_MEMORY: c_int = 2;

/// The library's name for a card.
type Device = *mut c_void;

/// The API's memory structure.
#[repr(C)]
#[derive(Default)]
struct MemoryInfo {
    total: u64,
    free: u64,
    used: u64,
}

/// The API's utilization structure: how busy the card and its memory were.
#[repr(C)]
#[derive(Default)]
struct Utilization {
    gpu: c_uint,
    memory: c_uint,
}

/// The API's per-process utilization sample: what one process used in one of the driver's sample
/// periods, which ended at `time_stamp`, in microseconds of the CPU's clock.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct UtilizationSample {
    pid: c_uint,
    time_stamp: u64,
    sm: c_uint,
    memory: c_uint,
    encoder: c_uint,
    decoder: c_uint,
}

/// The API's process record, as its third set of process lists returns it.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct ProcessInfo {
    pid: c_uint,
    used_memory: u64,
    gpu_instance: c_uint,
    compute_instance: c_uint,
}

// The layouts the API's header gives these, which a mistake here would read past.
const _: () = assert!(mem::size_of::<UtilizationSample>() == 32);
const _: () = assert!(mem::size_of::<ProcessInfo>() == 24);

type Init = unsafe extern "C" fn() -> c_int;
type HandleByBus = unsafe extern "C" fn(*const c_char, *mut Device) -> c_int;
type HandleByIndex = unsafe extern "C" fn(c_uint, *mut Device) -> c_int;
type ReadWord = unsafe extern "C" fn(Device, *mut c_uint) -> c_int;
type ReadWordOf = unsafe extern "C" fn(Device, c_int, *mut c_uint) -> c_int;
type ReadMemory = unsafe extern "C" fn(Device, *mut MemoryInfo) -> c_int;
type ReadUtilization = unsafe extern "C" fn(Device, *mut Utilization) -> c_int;
type ReadSamples = unsafe extern "C" fn(Device, *mut UtilizationSample, *mut c_uint, u64) -> c_int;
type ReadProcesses = unsafe extern "C" fn(Device, *mut c_uint, *mut ProcessInfo) -> c_int;

/// The library, set up for one card.
pub(super) struct Library {
    device: Device,
    temperature: Option<ReadWordOf>,
    utilization: Option<ReadUtilization>,
    performance_state: Option<ReadWord>,
    clock: Option<ReadWordOf>,
    most_clock: Option<ReadWordOf>,
    power: Option<ReadWord>,
    power_limit: Option<ReadWord>,
    fan: Option<ReadWord>,
    memory: Option<ReadMemory>,
    samples: Option<ReadSamples>,
    compute_processes: Option<ReadProcesses>,
    graphics_processes: Option<ReadProcesses>,
}

/// Looks a function up by name.
///
/// # Safety
///
/// `F` must be a function pointer type matching the API's signature for `name`.
unsafe fn symbol<F: Copy>(library: *mut c_void, name: &CStr) -> Option<F> {
    // SAFETY: the library handle is open, and the name ends in a zero.
    let address = unsafe { libc::dlsym(library, name.as_ptr()) };
    // SAFETY: a function pointer and a data pointer are the same size here, and the caller vouches
    // for the signature.
    (!address.is_null()).then(|| unsafe { mem::transmute_copy(&address) })
}

impl Library {
    /// Loads the library and finds the card at `card`, a PCI address as `0000:01:00.0`. With none,
    /// it is the library's first card, as the driver's first device file is when its list of cards
    /// can't be read. `None` if any of that can't be done.
    pub(super) fn open(name: &CStr, card: Option<&str>) -> Option<Self> {
        // SAFETY: loading a library by name. Nothing else in this process depends on it.
        let library = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if library.is_null() {
            return None;
        }
        // SAFETY: each type below is that function's documented signature.
        unsafe {
            let init: Init = symbol(library, c"nvmlInit_v2")?;
            if init() != SUCCESS {
                return None;
            }
            let mut device = ptr::null_mut();
            let found = match card {
                Some(card) => {
                    let by_bus: HandleByBus = symbol(library, c"nvmlDeviceGetHandleByPciBusId_v2")?;
                    let bus = CString::new(card).ok()?;
                    by_bus(bus.as_ptr(), &mut device)
                }
                None => {
                    let by_index: HandleByIndex = symbol(library, c"nvmlDeviceGetHandleByIndex_v2")?;
                    by_index(0, &mut device)
                }
            };
            if found != SUCCESS {
                return None;
            }
            Some(Self {
                device,
                temperature: symbol(library, c"nvmlDeviceGetTemperature"),
                utilization: symbol(library, c"nvmlDeviceGetUtilizationRates"),
                performance_state: symbol(library, c"nvmlDeviceGetPerformanceState"),
                clock: symbol(library, c"nvmlDeviceGetClockInfo"),
                most_clock: symbol(library, c"nvmlDeviceGetMaxClockInfo"),
                power: symbol(library, c"nvmlDeviceGetPowerUsage"),
                power_limit: symbol(library, c"nvmlDeviceGetEnforcedPowerLimit"),
                fan: symbol(library, c"nvmlDeviceGetFanSpeed"),
                memory: symbol(library, c"nvmlDeviceGetMemoryInfo"),
                samples: symbol(library, c"nvmlDeviceGetProcessUtilization"),
                compute_processes: symbol(library, c"nvmlDeviceGetComputeRunningProcesses_v3"),
                graphics_processes: symbol(library, c"nvmlDeviceGetGraphicsRunningProcesses_v3"),
            })
        }
    }

    fn word(&self, getter: Option<ReadWord>) -> Option<u32> {
        let mut value = 0;
        // SAFETY: the signature was vouched for at lookup, and `value` outlives the call.
        (unsafe { getter?(self.device, &mut value) } == SUCCESS).then_some(value)
    }

    fn word_of(&self, getter: Option<ReadWordOf>, which: c_int) -> Option<u32> {
        let mut value = 0;
        // SAFETY: as above.
        (unsafe { getter?(self.device, which, &mut value) } == SUCCESS).then_some(value)
    }

    fn utilization(&self) -> Option<Utilization> {
        let mut busy = Utilization::default();
        // SAFETY: as above.
        (unsafe { self.utilization?(self.device, &mut busy) } == SUCCESS).then_some(busy)
    }

    fn memory(&self) -> Option<MemoryInfo> {
        let mut info = MemoryInfo::default();
        // SAFETY: as above.
        (unsafe { self.memory?(self.device, &mut info) } == SUCCESS).then_some(info)
    }
}

/// Fills a list the API's way: into room for `room` entries, and once more with room for as many as
/// it says there are if that was too few. `call` gets the room and the count, and sets the count to
/// how many it wrote. `Err` is the API's answer when that isn't success.
fn fill_list<T: Default + Clone>(
    mut call: impl FnMut(&mut [T], &mut c_uint) -> c_int,
) -> Result<Vec<T>, c_int> {
    let mut room = FIRST_ROOM;
    for _ in 0..2 {
        let mut list = vec![T::default(); room];
        let mut count = c_uint::try_from(room).unwrap_or(c_uint::MAX);
        match call(&mut list, &mut count) {
            SUCCESS => {
                list.truncate((count as usize).min(room));
                return Ok(list);
            }
            INSUFFICIENT_SIZE if (count as usize) > room && (count as usize) <= MOST_ENTRIES => {
                room = count as usize;
            }
            answer => return Err(answer),
        }
    }
    Err(INSUFFICIENT_SIZE)
}

impl Library {
    fn processes_of(&self, getter: Option<ReadProcesses>) -> Option<Vec<ProcessInfo>> {
        let getter = getter?;
        // SAFETY: the signature was vouched for at lookup, the count is the room in `list`, and both
        // outlive the call.
        fill_list(|list: &mut [ProcessInfo], count| unsafe { getter(self.device, count, list.as_mut_ptr()) })
            .ok()
    }
}

impl ProcessSource for Library {
    fn samples_since(&mut self, since: u64) -> Samples {
        let Some(getter) = self.samples else { return Samples::Unavailable };
        // SAFETY: as above, with the count before the list in this call.
        let read = fill_list(|list: &mut [UtilizationSample], count| unsafe {
            getter(self.device, list.as_mut_ptr(), count, since)
        });
        match read {
            Ok(list) => Samples::Read(
                list.iter()
                    .map(|raw| Sample {
                        pid: raw.pid,
                        at: raw.time_stamp,
                        sm: raw.sm,
                        memory: raw.memory,
                        encoder: raw.encoder,
                        decoder: raw.decoder,
                    })
                    .collect(),
            ),
            Err(NOT_FOUND) => Samples::NoneSince,
            Err(_) => Samples::Unavailable,
        }
    }

    fn processes(&mut self) -> Option<Vec<ProcessMemory>> {
        let compute = self.processes_of(self.compute_processes);
        let graphics = self.processes_of(self.graphics_processes);
        if compute.is_none() && graphics.is_none() {
            return None;
        }
        let all = compute.into_iter().flatten().chain(graphics.into_iter().flatten());
        Some(
            all.map(|info| ProcessMemory {
                pid: info.pid,
                bytes: (info.used_memory != VALUE_NOT_AVAILABLE).then_some(info.used_memory),
            })
            .collect(),
        )
    }
}

impl SensorSource for Library {
    fn read_fast(&mut self) -> FastReadings {
        FastReadings {
            temperature_celsius: self.word_of(self.temperature, SENSOR_GPU),
            load_percent: self.utilization().map(|busy| busy.gpu),
            performance_state: self.word(self.performance_state),
            power_milliwatts: self.word(self.power),
            graphics_megahertz: self.word_of(self.clock, CLOCK_GRAPHICS),
            memory_megahertz: self.word_of(self.clock, CLOCK_MEMORY),
        }
    }

    fn read_slow(&mut self) -> SlowReadings {
        let memory = self.memory();
        SlowReadings {
            fan_percent: self.word(self.fan),
            memory_used_bytes: memory.as_ref().map(|memory| memory.used),
            memory_total_bytes: memory.as_ref().map(|memory| memory.total),
            power_limit_milliwatts: self.word(self.power_limit),
            graphics_most_megahertz: self.word_of(self.most_clock, CLOCK_GRAPHICS),
            memory_most_megahertz: self.word_of(self.most_clock, CLOCK_MEMORY),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_library_is_simply_no_library() {
        assert!(Library::open(c"/nonexistent/no-such-library.so", Some("0000:01:00.0")).is_none());
        assert!(Library::open(c"/nonexistent/no-such-library.so", None).is_none());
    }
}
