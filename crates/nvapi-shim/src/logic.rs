//! What the wrapper decides, kept apart from Windows so it can be tested anywhere.

use std::ffi::c_void;

pub const STATUS_OK: i32 = 0;
/// What NVIDIA's NVAPI says to the calls it refuses on a paravirtualized card.
pub const STATUS_NOT_SUPPORTED: i32 = -104;

/// NvAPI_GPU_GetLogicalGpuInfo, which NGX needs to match the card to its Direct3D device.
pub const GET_LOGICAL_GPU_INFO: u32 = 0x842b_066e;
/// NvAPI_GetPhysicalGPUsFromLogicalGPU, which NVIDIA's answers on a paravirtualized card too.
pub const GET_PHYSICAL_GPUS_FROM_LOGICAL_GPU: u32 = 0xaea3_fa32;
/// NvAPI_GPU_GetAdapterIdFromPhysicalGpu, the card's adapter LUID, which CUDA's matching uses.
pub const GET_ADAPTER_ID_FROM_PHYSICAL_GPU: u32 = 0x0ff0_7fde;

pub const MAX_PHYSICAL_GPUS: usize = 64;
/// NV_LOGICAL_GPU_DATA version 1: its size, and the version in the high half.
pub const LOGICAL_GPU_DATA_V1: u32 = 0x1_0238;

pub const NVIDIA: u32 = 0x10DE;
/// D3DKMT_ADAPTERTYPE's Paravirtualized bit, which the guest's kernel sets on a GPU-PV adapter.
pub const ADAPTER_TYPE_PARAVIRTUALIZED: u32 = 1 << 7;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Luid {
    pub low: u32,
    pub high: i32,
}

/// NV_LOGICAL_GPU_DATA_V1.
#[repr(C)]
pub struct LogicalGpuData {
    pub version: u32,
    pub os_adapter_id: *mut Luid,
    pub physical_gpu_count: u32,
    pub physical_gpu_handles: [*mut c_void; MAX_PHYSICAL_GPUS],
    pub reserved: [u32; 8],
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(size_of::<LogicalGpuData>() == (LOGICAL_GPU_DATA_V1 & 0xFFFF) as usize);

/// An adapter Windows lists, as far as the wrapper needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Adapter {
    pub luid: Luid,
    pub adapter_type: Option<u32>,
    pub vendor: Option<u32>,
}

impl Adapter {
    fn is_paravirtualized_nvidia(&self) -> bool {
        self.adapter_type.is_some_and(|kind| kind & ADAPTER_TYPE_PARAVIRTUALIZED != 0)
            && self.vendor == Some(NVIDIA)
    }
}

/// The guest's NVIDIA adapter, the test NVIDIA's NVAPI uses to go into its GPU-PV mode. With none
/// there is nothing to fix, and with several the wrapper can't tell which card a handle is, so it
/// fixes nothing.
pub fn guest_adapter(adapters: &[Adapter]) -> Option<Luid> {
    let mut found = adapters.iter().filter(|adapter| adapter.is_paravirtualized_nvidia());
    let first = found.next()?;
    found.next().is_none().then_some(first.luid)
}

/// Whether a GetLogicalGpuInfo answer is one to fill in: NVIDIA's refused it, and the caller gave
/// the version this knows and somewhere to put the LUID.
///
/// # Safety
/// `data` is null or points at as much of an NV_LOGICAL_GPU_DATA as its version word.
pub unsafe fn logical_info_to_fill(status: i32, data: *const LogicalGpuData) -> bool {
    if status != STATUS_NOT_SUPPORTED || data.is_null() {
        return false;
    }
    // SAFETY: the caller says `data` points at a struct that starts with its version.
    let version = unsafe { (*data).version };
    // SAFETY: with that version the struct is a whole NV_LOGICAL_GPU_DATA_V1.
    version == LOGICAL_GPU_DATA_V1 && !unsafe { (*data).os_adapter_id }.is_null()
}

/// Fills in what NVIDIA's would have: the card's physical GPUs and its adapter's LUID. False, with
/// nothing written, if there are no physical GPUs or more than the struct holds.
///
/// # Safety
/// `data` is a whole NV_LOGICAL_GPU_DATA_V1 whose `os_adapter_id` points at a LUID.
pub unsafe fn fill_logical_info(data: *mut LogicalGpuData, luid: Luid, physical: &[*mut c_void]) -> bool {
    if physical.is_empty() || physical.len() > MAX_PHYSICAL_GPUS {
        return false;
    }
    // SAFETY: the caller says `data` is a whole V1 struct and its LUID pointer is good.
    let data = unsafe { &mut *data };
    // SAFETY: as above.
    unsafe { data.os_adapter_id.write_unaligned(luid) };
    data.physical_gpu_count = physical.len() as u32;
    data.physical_gpu_handles = [std::ptr::null_mut(); MAX_PHYSICAL_GPUS];
    data.physical_gpu_handles[..physical.len()].copy_from_slice(physical);
    true
}

/// The LUID to give for GetAdapterIdFromPhysicalGpu in place of NVIDIA's answer: the guest's
/// adapter's, when NVIDIA's refused it or named an adapter this guest doesn't have, as on
/// Microsoft's host, where it gives the host's. `None` keeps NVIDIA's, so a bad handle still fails.
pub fn adapter_id_to_give(status: i32, given: Luid, guest: Luid, adapters: &[Adapter]) -> Option<Luid> {
    let known = adapters.iter().any(|adapter| adapter.luid == given);
    let replace = status == STATUS_NOT_SUPPORTED || (status == STATUS_OK && !known);
    replace.then_some(guest)
}

/// A path as the guest's kernel gives it, as a program opens it.
pub fn usable_path(raw: &str, windows_folder: &str) -> String {
    let raw = raw.trim_end_matches('\0');
    let lower = raw.to_ascii_lowercase();
    if lower.starts_with(r"\systemroot\") {
        format!("{}{}", windows_folder.trim_end_matches('\\'), &raw[r"\SystemRoot".len()..])
    } else if let Some(rest) = raw.strip_prefix(r"\??\") {
        rest.to_owned()
    } else {
        raw.to_owned()
    }
}

/// The registry lookup the guest's kernel answers with the driver's folder: D3DDDI_QUERYREGISTRY_INFO
/// with a fixed block, then room for the value.
pub mod registry {
    pub const KIND_DRIVER_STORE_PATH: u32 = 2;
    pub const FLAG_TRANSLATE_PATH: u32 = 1;
    const KIND: usize = 0;
    const FLAGS: usize = 4;
    const VALUE_TYPE: usize = 528;
    const OUTPUT_SIZE: usize = 536;
    const STATUS: usize = 540;
    pub const OUTPUT: usize = 544;
    const STATUS_SUCCESS: u32 = 0;
    const STATUS_OVERFLOW: u32 = 1;
    const TYPE_STRING: u32 = 1;

    fn put(buffer: &mut [u8], at: usize, value: u32) {
        buffer[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn get(buffer: &[u8], at: usize) -> Option<u32> {
        Some(u32::from_le_bytes(buffer.get(at..at + 4)?.try_into().ok()?))
    }

    /// The query for the driver's folder, as the guest sees it, with `room` bytes for the answer.
    pub fn driver_store_query(room: usize) -> Vec<u8> {
        let mut buffer = vec![0; OUTPUT + room];
        put(&mut buffer, KIND, KIND_DRIVER_STORE_PATH);
        put(&mut buffer, FLAGS, FLAG_TRANSLATE_PATH);
        put(&mut buffer, VALUE_TYPE, TYPE_STRING);
        buffer
    }

    #[derive(Debug, PartialEq, Eq)]
    pub enum Answer {
        Path(String),
        /// The answer needs this many bytes of room.
        NeedsRoom(usize),
        Failed,
    }

    pub fn read_answer(buffer: &[u8]) -> Answer {
        let (Some(status), Some(size)) = (get(buffer, STATUS), get(buffer, OUTPUT_SIZE)) else {
            return Answer::Failed;
        };
        match status {
            STATUS_OVERFLOW => Answer::NeedsRoom(size as usize),
            STATUS_SUCCESS => {
                let Some(bytes) = buffer.get(OUTPUT..OUTPUT + size as usize) else { return Answer::Failed };
                let units: Vec<u16> =
                    bytes.as_chunks::<2>().0.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
                let text = String::from_utf16_lossy(&units);
                let text = text.trim_end_matches('\0');
                if text.is_empty() { Answer::Failed } else { Answer::Path(text.to_owned()) }
            }
            _ => Answer::Failed,
        }
    }

    #[cfg(test)]
    pub fn answered(status: u32, value: &str, room: usize) -> Vec<u8> {
        let mut buffer = driver_store_query(room);
        let units: Vec<u8> = value.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
        put(&mut buffer, STATUS, status);
        put(&mut buffer, OUTPUT_SIZE, units.len() as u32);
        if status == STATUS_SUCCESS {
            buffer[OUTPUT..OUTPUT + units.len()].copy_from_slice(&units);
        }
        buffer
    }
}

#[cfg(test)]
mod tests {
    use super::registry::{Answer, answered, driver_store_query, read_answer};
    use super::*;

    const GUEST: Luid = Luid { low: 0x1_2016, high: 0 };
    const OTHER: Luid = Luid { low: 0x2_1d54, high: 0 };

    fn adapter(luid: Luid, adapter_type: u32, vendor: u32) -> Adapter {
        Adapter { luid, adapter_type: Some(adapter_type), vendor: Some(vendor) }
    }

    #[test]
    fn the_one_paravirtualized_nvidia_adapter_is_the_guests() {
        let basic = adapter(OTHER, 0x2, 0x1414);
        let card = adapter(GUEST, 0x83, NVIDIA);
        assert_eq!(guest_adapter(&[basic, card]), Some(GUEST));
    }

    #[test]
    fn bare_metal_and_two_cards_leave_everything_alone() {
        let bare = adapter(GUEST, 0x3, NVIDIA);
        assert_eq!(guest_adapter(&[bare]), None);
        let second = adapter(OTHER, 0x83, NVIDIA);
        assert_eq!(guest_adapter(&[adapter(GUEST, 0x83, NVIDIA), second]), None);
    }

    #[test]
    fn another_makers_paravirtualized_adapter_is_not_the_card() {
        assert_eq!(guest_adapter(&[adapter(GUEST, 0x83, 0x8086)]), None);
        let unknown = Adapter { luid: GUEST, adapter_type: Some(0x83), vendor: None };
        assert_eq!(guest_adapter(&[unknown]), None);
    }

    fn data(luid: &mut Luid) -> LogicalGpuData {
        LogicalGpuData {
            version: LOGICAL_GPU_DATA_V1,
            os_adapter_id: luid,
            physical_gpu_count: 0,
            physical_gpu_handles: [std::ptr::null_mut(); MAX_PHYSICAL_GPUS],
            reserved: [0; 8],
        }
    }

    #[test]
    fn only_a_refused_v1_answer_with_room_for_the_luid_is_filled() {
        let mut luid = Luid::default();
        let mut info = data(&mut luid);
        unsafe {
            assert!(logical_info_to_fill(STATUS_NOT_SUPPORTED, &info));
            assert!(!logical_info_to_fill(STATUS_OK, &info));
            assert!(!logical_info_to_fill(-5, &info));
            assert!(!logical_info_to_fill(STATUS_NOT_SUPPORTED, std::ptr::null()));
            info.version = 0x2_0238;
            assert!(!logical_info_to_fill(STATUS_NOT_SUPPORTED, &info));
            info.version = LOGICAL_GPU_DATA_V1;
            info.os_adapter_id = std::ptr::null_mut();
            assert!(!logical_info_to_fill(STATUS_NOT_SUPPORTED, &info));
        }
    }

    #[test]
    fn filling_gives_the_physical_gpus_and_the_guests_luid() {
        let mut luid = Luid::default();
        let mut info = data(&mut luid);
        info.physical_gpu_handles[3] = 0x99 as *mut c_void;
        let handle = 0x1234 as *mut c_void;
        assert!(unsafe { fill_logical_info(&mut info, GUEST, &[handle]) });
        assert_eq!(info.physical_gpu_count, 1);
        assert_eq!(info.physical_gpu_handles[0], handle);
        assert!(info.physical_gpu_handles[1..].iter().all(|handle| handle.is_null()));
        assert_eq!(luid, GUEST);
    }

    #[test]
    fn no_physical_gpus_fills_nothing() {
        let mut luid = Luid::default();
        let mut info = data(&mut luid);
        assert!(!unsafe { fill_logical_info(&mut info, GUEST, &[]) });
        assert_eq!(info.physical_gpu_count, 0);
        assert_eq!(luid, Luid::default());
    }

    #[test]
    fn nvidias_adapter_id_stays_when_it_names_one_of_the_guests_adapters() {
        let adapters = [adapter(GUEST, 0x83, NVIDIA), adapter(OTHER, 0x2, 0x1414)];
        assert_eq!(adapter_id_to_give(STATUS_OK, GUEST, GUEST, &adapters), None);
        assert_eq!(adapter_id_to_give(STATUS_NOT_SUPPORTED, GUEST, GUEST, &adapters), Some(GUEST));
        let hosts = Luid { low: 0x777, high: 0 };
        assert_eq!(adapter_id_to_give(STATUS_OK, hosts, GUEST, &adapters), Some(GUEST));
        // NVAPI_INVALID_HANDLE: the caller's mistake, which stays one.
        assert_eq!(adapter_id_to_give(-8, Luid::default(), GUEST, &adapters), None);
    }

    #[test]
    fn kernel_paths_become_paths_a_program_opens() {
        let windows = r"C:\Windows";
        let store = r"\System32\HostDriverStore\FileRepository\nv_dispi.inf_amd64_gpupv";
        let want = format!("{windows}{store}");
        assert_eq!(usable_path(&format!(r"\SystemRoot{store}"), windows), want);
        assert_eq!(usable_path(&format!(r"\??\{want}"), windows), want);
        assert_eq!(usable_path(&format!("{want}\0"), windows), want);
    }

    #[test]
    fn the_driver_store_query_asks_for_a_translated_path() {
        let query = driver_store_query(8);
        assert_eq!(query.len(), registry::OUTPUT + 8);
        assert_eq!(&query[0..8], &[2, 0, 0, 0, 1, 0, 0, 0]);
    }

    #[test]
    fn the_answer_is_read_or_asks_for_room() {
        let path = r"C:\Windows\System32\HostDriverStore\FileRepository\x";
        assert_eq!(read_answer(&answered(0, path, 200)), Answer::Path(path.to_owned()));
        let size = (path.len() + 1) * 2;
        assert_eq!(read_answer(&answered(1, path, 4)), Answer::NeedsRoom(size));
        assert_eq!(read_answer(&answered(2, path, 200)), Answer::Failed);
        assert_eq!(read_answer(&[0; 10]), Answer::Failed);
    }
}
