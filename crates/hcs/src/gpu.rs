//! The host's cards that Windows can share with a guest, each found by its GPU partition adapter:
//! the device interface a card's driver offers when it can be partitioned. HCS takes a card by that
//! interface's path, and the guest needs the folder of the card's driver.

use crate::error::{Error, Result};
use crate::text::{from_wide, wide};
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_Device_IDW, CR_SUCCESS, DICS_FLAG_GLOBAL, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, DIREG_DRV,
    GUID_DEVCLASS_DISPLAY, HDEVINFO, MAX_DEVICE_ID_LEN, SETUP_DI_REGISTRY_PROPERTY, SP_DEVICE_INTERFACE_DATA,
    SP_DEVICE_INTERFACE_DETAIL_DATA_W, SP_DEVINFO_DATA, SPDRP_DEVICEDESC, SPDRP_FRIENDLYNAME, SPDRP_SERVICE,
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
    SetupDiGetDeviceInterfaceDetailW, SetupDiGetDeviceRegistryPropertyW, SetupDiOpenDevRegKey,
    SetupGetInfDriverStoreLocationW,
};
use windows_sys::Win32::Foundation::{INVALID_HANDLE_VALUE, MAX_PATH};
use windows_sys::Win32::System::Registry::{KEY_READ, RegCloseKey, RegQueryValueExW};
use windows_sys::core::GUID;

/// The device interface a card's driver offers when Windows can partition the card for guests.
pub const GPU_PARTITION_ADAPTER: GUID = GUID::from_u128(0x064092b3_625e_43bf_9eb5_dc845897dd59);
/// Room for a card's name or service in the registry.
const PROPERTY_UNITS: usize = 256;
/// Room for a driver's INF name and its folder.
const PATH_UNITS: usize = MAX_PATH as usize;

#[derive(Debug, Clone)]
pub struct Card {
    pub name: String,
    /// The device's instance ID, such as `PCI\VEN_10DE&DEV_...`.
    pub instance_id: String,
    /// The partition adapter's interface path, which HCS takes in a `GpuAssignment::List`.
    pub interface_path: String,
    /// The kernel driver's service, such as `nvlddmkm`.
    pub service: String,
    /// The driver's folder in the host's driver store, which the guest needs a copy of.
    pub driver_folder: Option<PathBuf>,
}

impl Card {
    pub fn is_nvidia(&self) -> bool {
        self.service.eq_ignore_ascii_case("nvlddmkm")
            || self.instance_id.to_ascii_uppercase().contains("VEN_10DE")
    }
}

/// Every card on the host that can be shared with a guest now.
pub fn cards() -> Result<Vec<Card>> {
    // SAFETY: the class GUID outlives the call, and no enumerator or window is given.
    let set = unsafe {
        SetupDiGetClassDevsW(
            &GPU_PARTITION_ADAPTER,
            null(),
            null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    };
    if set == INVALID_HANDLE_VALUE as HDEVINFO {
        return Err(Error::last("Listing the host's GPU partition adapters"));
    }
    let set = DeviceSet(set);
    let mut found = Vec::new();
    for index in 0.. {
        let mut interface = SP_DEVICE_INTERFACE_DATA {
            cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
            InterfaceClassGuid: GUID::from_u128(0),
            Flags: 0,
            Reserved: 0,
        };
        // SAFETY: the set is open, and `interface` is sized as SetupAPI expects.
        if unsafe {
            SetupDiEnumDeviceInterfaces(set.0, null(), &GPU_PARTITION_ADAPTER, index, &mut interface)
        } == 0
        {
            break;
        }
        if let Some(card) = set.card(&interface) {
            found.push(card);
        }
    }
    Ok(found)
}

/// A SetupAPI device list, destroyed when dropped.
struct DeviceSet(HDEVINFO);

impl Drop for DeviceSet {
    fn drop(&mut self) {
        // SAFETY: the set is ours and isn't used after this.
        unsafe { SetupDiDestroyDeviceInfoList(self.0) };
    }
}

impl DeviceSet {
    fn card(&self, interface: &SP_DEVICE_INTERFACE_DATA) -> Option<Card> {
        let mut device = SP_DEVINFO_DATA {
            cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
            ClassGuid: GUID::from_u128(0),
            DevInst: 0,
            Reserved: 0,
        };
        let interface_path = self.interface_path(interface, &mut device)?;
        if !same_guid(&device.ClassGuid, &GUID_DEVCLASS_DISPLAY) {
            return None;
        }
        let mut instance = [0u16; MAX_DEVICE_ID_LEN as usize + 1];
        // SAFETY: the buffer is as long as the count given.
        if unsafe { CM_Get_Device_IDW(device.DevInst, instance.as_mut_ptr(), instance.len() as u32, 0) }
            != CR_SUCCESS
        {
            return None;
        }
        let name = self
            .property(&device, SPDRP_FRIENDLYNAME)
            .or_else(|| self.property(&device, SPDRP_DEVICEDESC))
            .unwrap_or_else(|| "A card with no name".to_owned());
        Some(Card {
            name,
            instance_id: from_wide(&instance),
            interface_path,
            service: self.property(&device, SPDRP_SERVICE).unwrap_or_default(),
            driver_folder: self.driver_folder(&device),
        })
    }

    /// The interface's path, and the device it belongs to filled in.
    fn interface_path(
        &self,
        interface: &SP_DEVICE_INTERFACE_DATA,
        device: &mut SP_DEVINFO_DATA,
    ) -> Option<String> {
        let mut needed = 0;
        // SAFETY: asking only for the size, with no buffer.
        unsafe {
            SetupDiGetDeviceInterfaceDetailW(self.0, interface, null_mut(), 0, &mut needed, null_mut())
        };
        if needed == 0 {
            return None;
        }
        // In u64s, so the buffer is aligned as the detail structure needs.
        let mut buffer = vec![0u64; (needed as usize).div_ceil(size_of::<u64>())];
        let detail = buffer.as_mut_ptr().cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
        // SAFETY: the buffer is at least as big as a detail structure, and aligned for one.
        unsafe { (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32 };
        // SAFETY: the buffer is `needed` bytes or more, as the size given says.
        let asked = unsafe {
            SetupDiGetDeviceInterfaceDetailW(self.0, interface, detail, needed, null_mut(), device)
        };
        if asked == 0 {
            return None;
        }
        let offset = std::mem::offset_of!(SP_DEVICE_INTERFACE_DETAIL_DATA_W, DevicePath);
        let units = (needed as usize - offset) / size_of::<u16>();
        // SAFETY: the path starts at its field's offset and fills the rest of the `needed` bytes.
        let path =
            unsafe { std::slice::from_raw_parts(detail.cast::<u8>().add(offset).cast::<u16>(), units) };
        Some(from_wide(path))
    }

    fn property(&self, device: &SP_DEVINFO_DATA, property: SETUP_DI_REGISTRY_PROPERTY) -> Option<String> {
        let mut buffer = [0u16; PROPERTY_UNITS];
        // SAFETY: the buffer is as many bytes long as the size given.
        let asked = unsafe {
            SetupDiGetDeviceRegistryPropertyW(
                self.0,
                device,
                property,
                null_mut(),
                buffer.as_mut_ptr().cast(),
                size_of_val(&buffer) as u32,
                null_mut(),
            )
        };
        let text = from_wide(&buffer);
        (asked != 0 && !text.is_empty()).then_some(text)
    }

    /// The driver store folder of the device's driver, from the INF it was installed from.
    fn driver_folder(&self, device: &SP_DEVINFO_DATA) -> Option<PathBuf> {
        // SAFETY: the set is open, and the device was filled in by SetupAPI.
        let key = unsafe { SetupDiOpenDevRegKey(self.0, device, DICS_FLAG_GLOBAL, 0, DIREG_DRV, KEY_READ) };
        if key.is_null() || key == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut inf = [0u16; PATH_UNITS];
        let mut bytes = size_of_val(&inf) as u32;
        let value = wide("InfPath");
        // SAFETY: the key is open, the value's name ends in a zero, and the buffer is `bytes` long.
        let read = unsafe {
            RegQueryValueExW(key, value.as_ptr(), null(), null_mut(), inf.as_mut_ptr().cast(), &mut bytes)
        };
        // SAFETY: the key is ours and isn't used after this.
        unsafe { RegCloseKey(key) };
        // A registry string needn't end in a zero, so the buffer's last unit is kept as one.
        inf[PATH_UNITS - 1] = 0;
        if read != 0 || inf[0] == 0 {
            return None;
        }
        let mut location = [0u16; PATH_UNITS];
        // SAFETY: the INF name ends in a zero, and the buffer is as long as the count given.
        let found = unsafe {
            SetupGetInfDriverStoreLocationW(
                inf.as_ptr(),
                null(),
                null(),
                location.as_mut_ptr(),
                location.len() as u32,
                null_mut(),
            )
        };
        if found == 0 {
            return None;
        }
        // The answer is the INF inside the folder.
        PathBuf::from(from_wide(&location)).parent().map(PathBuf::from)
    }
}

fn same_guid(a: &GUID, b: &GUID) -> bool {
    (a.data1, a.data2, a.data3, a.data4) == (b.data1, b.data2, b.data3, b.data4)
}
