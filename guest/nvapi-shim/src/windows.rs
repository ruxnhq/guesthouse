//! The DLL's exports, and the Windows calls behind them.

use crate::logic::{self, Adapter, LogicalGpuData, Luid, registry};
use std::collections::HashSet;
use std::ffi::{OsStr, c_void};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringW;
use windows_sys::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW,
    GetModuleHandleExW, GetProcAddress, LOAD_WITH_ALTERED_SEARCH_PATH, LoadLibraryExW,
};
use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW};

/// NVIDIA's DLL, by the name it has in the driver's folder.
const REAL_NAME: &str = "nvapi64.dll";
/// The driver's folder as a GPU-PV guest is lent it, and as a normal install has it.
const STORE_FOLDERS: [&str; 2] =
    [r"System32\HostDriverStore\FileRepository", r"System32\DriverStore\FileRepository"];
const STORE_PREFIX: &str = "nv_dispi.inf_amd64_";
/// Settings under HKLM\SOFTWARE\Guesthouse, both DWORDs: 1 logs what the wrapper does, and 1 turns
/// its answers off, so it only hands calls to NVIDIA's.
const SETTINGS_KEY: &str = r"SOFTWARE\Guesthouse";
const LOG_VALUE: &str = "NvapiShimLog";
const OFF_VALUE: &str = "NvapiShimOff";
const LOG_FILE: &str = "guesthouse-nvapi-shim.log";

type QueryInterface = unsafe extern "C" fn(u32) -> *mut c_void;
type GetLogicalGpuInfo = unsafe extern "C" fn(*mut c_void, *mut LogicalGpuData) -> i32;
type GetPhysicalGpus = unsafe extern "C" fn(*mut c_void, *mut *mut c_void, *mut u32) -> i32;
type GetAdapterId = unsafe extern "C" fn(*mut c_void, *mut Luid) -> i32;

struct State {
    query: Option<QueryInterface>,
    /// NVIDIA's nvapi_Direct_GetMethod, or 0.
    direct: usize,
    /// The adapter to answer for, when the wrapper answers at all.
    guest: Option<Luid>,
    adapters: Vec<Adapter>,
    log: bool,
}

static STATE: OnceLock<State> = OnceLock::new();
static REAL_LOGICAL_GPU_INFO: AtomicUsize = AtomicUsize::new(0);
static REAL_ADAPTER_ID: AtomicUsize = AtomicUsize::new(0);
static ASKED: Mutex<Option<HashSet<u32>>> = Mutex::new(None);

fn wide(text: &str) -> Vec<u16> {
    OsStr::new(text).encode_wide().chain([0]).collect()
}

fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain([0]).collect()
}

fn say(enabled: bool, text: &str) {
    if !enabled {
        return;
    }
    let line = format!("guesthouse nvapi64 [{}]: {text}\n", std::process::id());
    // SAFETY: a zero-terminated wide string.
    unsafe { OutputDebugStringW(wide(&line).as_ptr()) };
    let file = OpenOptions::new().create(true).append(true).open(std::env::temp_dir().join(LOG_FILE));
    if let Ok(mut file) = file {
        let _ = file.write_all(line.as_bytes());
    }
}

fn setting(name: &str) -> Option<u32> {
    let (key, value) = (wide(SETTINGS_KEY), wide(name));
    let mut data = 0u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: the strings end in zeros, and the data and size point at a DWORD and its size.
    let error = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            (&raw mut data).cast(),
            &mut size,
        )
    };
    (error == 0).then_some(data)
}

fn windows_folder() -> String {
    std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_owned())
}

/// This DLL's own file, so it never loads itself as NVIDIA's.
fn own_path() -> Option<PathBuf> {
    let mut module = null_mut();
    let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
    let address = own_path as *const c_void;
    // SAFETY: the address is a function in this module, and the handle goes in `module`.
    if unsafe { GetModuleHandleExW(flags, address.cast(), &mut module) } == 0 {
        return None;
    }
    let mut buffer = vec![0u16; 1024];
    // SAFETY: the buffer holds as many units as its length says.
    let length = unsafe { GetModuleFileNameW(module, buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
    (length > 0 && length < buffer.len()).then(|| PathBuf::from(String::from_utf16_lossy(&buffer[..length])))
}

fn same_file(first: &Path, second: &Path) -> bool {
    first.to_string_lossy().eq_ignore_ascii_case(&second.to_string_lossy())
}

/// NVIDIA's DLL in each NVIDIA folder of the driver stores, the newest first.
fn store_copies() -> Vec<PathBuf> {
    let windows = PathBuf::from(windows_folder());
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for store in STORE_FOLDERS {
        let Ok(entries) = fs::read_dir(windows.join(store)) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            let file = entry.path().join(REAL_NAME);
            if !name.starts_with(STORE_PREFIX) {
                continue;
            }
            if let Ok(modified) = fs::metadata(&file).and_then(|metadata| metadata.modified()) {
                found.push((modified, file));
            }
        }
        // The guest's lent folder is the card's own, so a copy there beats a normal install's.
        if !found.is_empty() {
            break;
        }
    }
    found.sort_by_key(|found| std::cmp::Reverse(found.0));
    found.into_iter().map(|(_, file)| file).collect()
}

/// Loads NVIDIA's DLL: from the folder the guest's kernel names for the card first, then from the
/// driver stores.
fn load_real(stores: &[String], log: bool) -> Option<*mut c_void> {
    let own = own_path();
    let windows = windows_folder();
    let named = stores.iter().map(|store| PathBuf::from(logic::usable_path(store, &windows)).join(REAL_NAME));
    for file in named.chain(store_copies()) {
        if own.as_deref().is_some_and(|own| same_file(own, &file)) || !file.is_file() {
            continue;
        }
        // SAFETY: a zero-terminated path; the altered search path makes its own folder the one its
        // imports are found in.
        let module =
            unsafe { LoadLibraryExW(wide_path(&file).as_ptr(), null_mut(), LOAD_WITH_ALTERED_SEARCH_PATH) };
        if !module.is_null() {
            say(log, &format!("loaded NVIDIA's from {}", file.display()));
            return Some(module);
        }
        say(log, &format!("couldn't load {}", file.display()));
    }
    say(log, "found no NVIDIA nvapi64.dll to load");
    None
}

fn load() -> State {
    let log = setting(LOG_VALUE) == Some(1);
    let off = setting(OFF_VALUE) == Some(1);
    let found = d3dkmt::adapters();
    let adapters: Vec<Adapter> = found.iter().map(|found| found.adapter).collect();
    let stores: Vec<String> = found.iter().filter_map(|found| found.store.clone()).collect();
    let guest = if off { None } else { logic::guest_adapter(&adapters) };
    say(
        log,
        &format!(
            "adapters {adapters:x?}, answering for {guest:x?}{}",
            if off { " (turned off)" } else { "" }
        ),
    );
    let Some(module) = load_real(&stores, log) else {
        return State { query: None, direct: 0, guest, adapters, log };
    };
    let export = |name: &str| {
        let name = format!("{name}\0");
        // SAFETY: a loaded module and a zero-terminated name.
        unsafe { GetProcAddress(module, name.as_ptr()) }.map_or(0, |function| function as usize)
    };
    let query = export("nvapi_QueryInterface");
    // SAFETY: NVIDIA's nvapi_QueryInterface has this signature.
    let query = (query != 0).then(|| unsafe { std::mem::transmute::<usize, QueryInterface>(query) });
    State { query, direct: export("nvapi_Direct_GetMethod"), guest, adapters, log }
}

fn state() -> &'static State {
    STATE.get_or_init(load)
}

fn note_asked(state: &State, id: u32, real: *mut c_void) {
    let Ok(mut asked) = ASKED.lock() else { return };
    if asked.get_or_insert_with(HashSet::new).insert(id) {
        say(
            state.log,
            &format!("asked for {id:#010x}, NVIDIA's has {}", if real.is_null() { "none" } else { "it" }),
        );
    }
}

fn query_interface(id: u32) -> *mut c_void {
    let state = state();
    let Some(query) = state.query else { return null_mut() };
    // SAFETY: NVIDIA's own export, called as NVIDIA's callers do.
    let real = unsafe { query(id) };
    note_asked(state, id, real);
    if real.is_null() || state.guest.is_none() {
        return real;
    }
    match id {
        logic::GET_LOGICAL_GPU_INFO => {
            REAL_LOGICAL_GPU_INFO.store(real as usize, Ordering::Release);
            logical_gpu_info as *mut c_void
        }
        logic::GET_ADAPTER_ID_FROM_PHYSICAL_GPU => {
            REAL_ADAPTER_ID.store(real as usize, Ordering::Release);
            adapter_id as *mut c_void
        }
        _ => real,
    }
}

/// NVAPI's one export that every function is found through.
///
/// # Safety
/// Called as NVIDIA's is.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvapi_QueryInterface(id: u32) -> *mut c_void {
    catch_unwind(|| query_interface(id)).unwrap_or(null_mut())
}

fn fixed_logical_gpu_info(status: i32, logical: *mut c_void, data: *mut LogicalGpuData) -> i32 {
    let state = state();
    let (Some(guest), Some(query)) = (state.guest, state.query) else { return status };
    // SAFETY: the caller of GetLogicalGpuInfo gives a struct as its version word says, or null.
    if !unsafe { logic::logical_info_to_fill(status, data) } {
        return status;
    }
    // SAFETY: as in query_interface.
    let get = unsafe { query(logic::GET_PHYSICAL_GPUS_FROM_LOGICAL_GPU) };
    if get.is_null() {
        say(state.log, "GetLogicalGpuInfo refused, and NVIDIA's has no GetPhysicalGPUsFromLogicalGPU");
        return status;
    }
    // SAFETY: that function's signature.
    let get: GetPhysicalGpus = unsafe { std::mem::transmute(get) };
    let mut handles = [null_mut(); logic::MAX_PHYSICAL_GPUS];
    let mut count = 0u32;
    // SAFETY: room for as many handles as NVAPI ever gives.
    let got = unsafe { get(logical, handles.as_mut_ptr(), &mut count) };
    let physical = handles.get(..count as usize).filter(|_| got == logic::STATUS_OK);
    // SAFETY: logical_info_to_fill checked the struct's version and LUID pointer.
    if let Some(physical) = physical
        && unsafe { logic::fill_logical_info(data, guest, physical) }
    {
        say(
            state.log,
            &format!("GetLogicalGpuInfo: NVIDIA's said {status}, gave {count} GPU(s) and {guest:x?}"),
        );
        return logic::STATUS_OK;
    }
    say(
        state.log,
        &format!("GetLogicalGpuInfo: NVIDIA's said {status}, its physical GPUs {got}, left as is"),
    );
    status
}

unsafe extern "C" fn logical_gpu_info(logical: *mut c_void, data: *mut LogicalGpuData) -> i32 {
    // SAFETY: stored before this function was handed out, from NVIDIA's own for this id.
    let real: GetLogicalGpuInfo =
        unsafe { std::mem::transmute(REAL_LOGICAL_GPU_INFO.load(Ordering::Acquire)) };
    // SAFETY: the caller's arguments, as they came.
    let status = unsafe { real(logical, data) };
    catch_unwind(AssertUnwindSafe(|| fixed_logical_gpu_info(status, logical, data))).unwrap_or(status)
}

fn fixed_adapter_id(status: i32, luid: *mut Luid) -> i32 {
    let state = state();
    let Some(guest) = state.guest else { return status };
    if luid.is_null() {
        return status;
    }
    // SAFETY: the caller gave room for a LUID, which NVIDIA's filled if it succeeded.
    let given = if status == logic::STATUS_OK { unsafe { luid.read_unaligned() } } else { Luid::default() };
    let Some(answer) = logic::adapter_id_to_give(status, given, guest, &state.adapters) else {
        return status;
    };
    // SAFETY: as above.
    unsafe { luid.write_unaligned(answer) };
    say(
        state.log,
        &format!("GetAdapterIdFromPhysicalGpu: NVIDIA's said {status} with {given:x?}, gave {answer:x?}"),
    );
    logic::STATUS_OK
}

unsafe extern "C" fn adapter_id(physical: *mut c_void, luid: *mut Luid) -> i32 {
    // SAFETY: as in logical_gpu_info.
    let real: GetAdapterId = unsafe { std::mem::transmute(REAL_ADAPTER_ID.load(Ordering::Acquire)) };
    // SAFETY: the caller's arguments, as they came.
    let status = unsafe { real(physical, luid) };
    catch_unwind(AssertUnwindSafe(|| fixed_adapter_id(status, luid))).unwrap_or(status)
}

extern "C" fn direct_target() -> usize {
    catch_unwind(|| state().direct).unwrap_or(0)
}

/// NVIDIA's second export, whose signature isn't known: it finds NVIDIA's and jumps there with every
/// argument register and the stack as the caller left them. With no NVIDIA one it returns 0.
///
/// # Safety
/// Called as NVIDIA's is.
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nvapi_Direct_GetMethod() {
    core::arch::naked_asm!(
        "push rcx",
        "push rdx",
        "push r8",
        "push r9",
        // With the four pushes this keeps the stack 16-byte aligned for the call, with its 32 bytes
        // of shadow space below the saved vector registers.
        "sub rsp, 0x68",
        "movdqu [rsp + 0x20], xmm0",
        "movdqu [rsp + 0x30], xmm1",
        "movdqu [rsp + 0x40], xmm2",
        "movdqu [rsp + 0x50], xmm3",
        "call {target}",
        "movdqu xmm0, [rsp + 0x20]",
        "movdqu xmm1, [rsp + 0x30]",
        "movdqu xmm2, [rsp + 0x40]",
        "movdqu xmm3, [rsp + 0x50]",
        "add rsp, 0x68",
        "pop r9",
        "pop r8",
        "pop rdx",
        "pop rcx",
        "test rax, rax",
        "jz 2f",
        "jmp rax",
        "2:",
        "ret",
        target = sym direct_target,
    );
}

/// The adapters Windows lists, through the same kernel calls NVIDIA's NVAPI makes.
mod d3dkmt {
    use super::{Adapter, Luid, registry};
    use std::ffi::c_void;
    use std::ptr::null_mut;

    const QUERY_ADAPTER_TYPE: u32 = 15;
    const QUERY_DEVICE_IDS: u32 = 31;
    const QUERY_REGISTRY: u32 = 48;
    const TRIES: usize = 4;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct AdapterInfo {
        handle: u32,
        luid: Luid,
        sources: u32,
        precise_present_regions: i32,
    }

    #[repr(C)]
    struct EnumAdapters2 {
        count: u32,
        adapters: *mut AdapterInfo,
    }

    #[repr(C)]
    struct QueryAdapterInfo {
        adapter: u32,
        kind: u32,
        data: *mut c_void,
        size: u32,
    }

    #[repr(C)]
    struct CloseAdapter {
        adapter: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct DeviceIds {
        physical_adapter: u32,
        vendor: u32,
        device: u32,
        subsystem_vendor: u32,
        subsystem: u32,
        revision: u32,
        bus: u32,
    }

    #[link(name = "gdi32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn D3DKMTEnumAdapters2(enumerate: *mut EnumAdapters2) -> i32;
        fn D3DKMTQueryAdapterInfo(query: *const QueryAdapterInfo) -> i32;
        fn D3DKMTCloseAdapter(close: *const CloseAdapter) -> i32;
    }

    pub struct Found {
        pub adapter: Adapter,
        /// The driver's folder the guest's kernel names, asked of NVIDIA adapters only.
        pub store: Option<String>,
    }

    fn query<T>(adapter: u32, kind: u32, data: &mut T) -> bool {
        let query =
            QueryAdapterInfo { adapter, kind, data: (data as *mut T).cast(), size: size_of::<T>() as u32 };
        // SAFETY: the data is as big as the size says.
        unsafe { D3DKMTQueryAdapterInfo(&query) >= 0 }
    }

    fn driver_store(adapter: u32) -> Option<String> {
        let mut room = 520;
        for _ in 0..TRIES {
            // In u64s, so the buffer is aligned as the struct wants.
            let bytes = registry::driver_store_query(room);
            let mut buffer = vec![0u64; bytes.len().div_ceil(8)];
            // SAFETY: the u64 buffer holds at least the query's bytes.
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer.as_mut_ptr().cast::<u8>(), bytes.len())
            };
            let query = QueryAdapterInfo {
                adapter,
                kind: QUERY_REGISTRY,
                data: buffer.as_mut_ptr().cast(),
                size: bytes.len() as u32,
            };
            // SAFETY: the buffer is as big as the size says.
            let status = unsafe { D3DKMTQueryAdapterInfo(&query) };
            if status < 0 {
                return None;
            }
            // SAFETY: the buffer holds bytes.len() bytes.
            let answer = unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), bytes.len()) };
            match registry::read_answer(answer) {
                registry::Answer::Path(path) => return Some(path),
                registry::Answer::NeedsRoom(size) if size > room => room = size,
                _ => return None,
            }
        }
        None
    }

    fn listed() -> Vec<AdapterInfo> {
        for _ in 0..TRIES {
            let mut enumerate = EnumAdapters2 { count: 0, adapters: null_mut() };
            // SAFETY: with no array it only counts.
            if unsafe { D3DKMTEnumAdapters2(&mut enumerate) } < 0 {
                return Vec::new();
            }
            let mut adapters = vec![AdapterInfo::default(); enumerate.count as usize];
            enumerate.adapters = adapters.as_mut_ptr();
            // SAFETY: the array holds as many as the count says. It fails if more came since.
            if unsafe { D3DKMTEnumAdapters2(&mut enumerate) } >= 0 {
                adapters.truncate(enumerate.count as usize);
                return adapters;
            }
        }
        Vec::new()
    }

    pub fn adapters() -> Vec<Found> {
        let mut found = Vec::new();
        for info in listed() {
            let mut adapter_type = 0u32;
            let adapter_type =
                query(info.handle, QUERY_ADAPTER_TYPE, &mut adapter_type).then_some(adapter_type);
            let mut ids = DeviceIds::default();
            let vendor = query(info.handle, QUERY_DEVICE_IDS, &mut ids).then_some(ids.vendor);
            let store = (vendor == Some(super::logic::NVIDIA)).then(|| driver_store(info.handle)).flatten();
            found.push(Found { adapter: Adapter { luid: info.luid, adapter_type, vendor }, store });
            // SAFETY: a handle EnumAdapters2 opened.
            unsafe { D3DKMTCloseAdapter(&CloseAdapter { adapter: info.handle }) };
        }
        found
    }
}
