//! virtdisk.dll: makes a VHDX, and attaches a VHDX or an ISO to the host so its volumes can be read
//! and written while no machine has it. Also finds the volumes on an attached disk.

use crate::error::{Error, Result, check_win32};
use crate::text::{from_wide, wide};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_NO_MORE_ITEMS, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    BusTypeFileBackedVirtual, CreateFileW, FILE_DEVICE_CD_ROM, FILE_DEVICE_DISK, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FindFirstVolumeW, FindNextVolumeW, FindVolumeClose, GetLogicalDrives,
    GetVolumePathNamesForVolumeNameW, OPEN_EXISTING,
};
use windows_sys::Win32::Storage::Vhd::{
    ATTACH_VIRTUAL_DISK_FLAG_NO_DRIVE_LETTER, ATTACH_VIRTUAL_DISK_FLAG_NONE,
    ATTACH_VIRTUAL_DISK_FLAG_READ_ONLY, ATTACH_VIRTUAL_DISK_PARAMETERS, ATTACH_VIRTUAL_DISK_PARAMETERS_0,
    ATTACH_VIRTUAL_DISK_PARAMETERS_0_0, ATTACH_VIRTUAL_DISK_VERSION_1, AttachVirtualDisk,
    CREATE_VIRTUAL_DISK_FLAG_NONE, CREATE_VIRTUAL_DISK_PARAMETERS, CREATE_VIRTUAL_DISK_VERSION_2,
    CreateVirtualDisk, DETACH_VIRTUAL_DISK_FLAG_NONE, DetachVirtualDisk, GET_VIRTUAL_DISK_INFO,
    GET_VIRTUAL_DISK_INFO_SIZE, GetVirtualDiskInformation, GetVirtualDiskPhysicalPath,
    MERGE_VIRTUAL_DISK_FLAG_NONE, MERGE_VIRTUAL_DISK_PARAMETERS, MERGE_VIRTUAL_DISK_VERSION_2,
    MergeVirtualDisk, OPEN_VIRTUAL_DISK_FLAG_NONE, OPEN_VIRTUAL_DISK_PARAMETERS,
    OPEN_VIRTUAL_DISK_RW_DEPTH_DEFAULT, OPEN_VIRTUAL_DISK_VERSION_1, OPEN_VIRTUAL_DISK_VERSION_2,
    OpenVirtualDisk, RESIZE_VIRTUAL_DISK_FLAG_NONE, RESIZE_VIRTUAL_DISK_PARAMETERS,
    RESIZE_VIRTUAL_DISK_VERSION_1, ResizeVirtualDisk, VIRTUAL_DISK_ACCESS_NONE, VIRTUAL_DISK_ACCESS_READ,
    VIRTUAL_STORAGE_TYPE, VIRTUAL_STORAGE_TYPE_DEVICE_ISO, VIRTUAL_STORAGE_TYPE_DEVICE_VHDX,
    VIRTUAL_STORAGE_TYPE_VENDOR_MICROSOFT,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::{
    IOCTL_STORAGE_GET_DEVICE_NUMBER, IOCTL_STORAGE_QUERY_PROPERTY, PropertyStandardQuery,
    STORAGE_DEVICE_DESCRIPTOR, STORAGE_DEVICE_NUMBER, STORAGE_PROPERTY_QUERY, StorageDeviceProperty,
};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegEnumValueW, RegOpenKeyExW,
};

/// A guest's disk reads in 512-byte sectors, as every Windows installer expects, over the 4 KiB
/// sectors a host's own disk has.
const LOGICAL_SECTOR: u32 = 512;
const PHYSICAL_SECTOR: u32 = 4096;
/// Room for a volume's GUID path, `\\?\Volume{...}\`, with some to spare.
const VOLUME_NAME_UNITS: usize = 64;
/// Room for a disk's path or a volume's mount points.
const PATH_UNITS: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Vhdx,
    Iso,
}

impl Kind {
    fn storage_type(self) -> VIRTUAL_STORAGE_TYPE {
        let device = match self {
            Kind::Vhdx => VIRTUAL_STORAGE_TYPE_DEVICE_VHDX,
            Kind::Iso => VIRTUAL_STORAGE_TYPE_DEVICE_ISO,
        };
        VIRTUAL_STORAGE_TYPE { DeviceId: device, VendorId: VIRTUAL_STORAGE_TYPE_VENDOR_MICROSOFT }
    }

    /// The kind of device Windows makes of the disk once it's attached.
    fn device_type(self) -> u32 {
        match self {
            Kind::Vhdx => FILE_DEVICE_DISK,
            Kind::Iso => FILE_DEVICE_CD_ROM,
        }
    }
}

/// Makes a VHDX that grows as it's written, up to `size_bytes`. Fails if the file is there already.
pub fn create_vhdx(path: &Path, size_bytes: u64) -> Result<()> {
    let storage = Kind::Vhdx.storage_type();
    // SAFETY: an all-zero parameter block is valid, and the fields this needs are set below.
    let mut parameters: CREATE_VIRTUAL_DISK_PARAMETERS = unsafe { std::mem::zeroed() };
    parameters.Version = CREATE_VIRTUAL_DISK_VERSION_2;
    parameters.Anonymous.Version2.MaximumSize = size_bytes;
    parameters.Anonymous.Version2.SectorSizeInBytes = LOGICAL_SECTOR;
    parameters.Anonymous.Version2.PhysicalSectorSizeInBytes = PHYSICAL_SECTOR;
    let text = wide(path);
    let mut handle: HANDLE = null_mut();
    // SAFETY: every pointer is to a value that outlives the call, the path ends in a zero, and
    // version 2 takes no access mask.
    let error = unsafe {
        CreateVirtualDisk(
            &storage,
            text.as_ptr(),
            VIRTUAL_DISK_ACCESS_NONE,
            null_mut(),
            CREATE_VIRTUAL_DISK_FLAG_NONE,
            0,
            &parameters,
            null(),
            &mut handle,
        )
    };
    check_win32(error, &format!("Making the disk {}", path.display()))?;
    // SAFETY: the handle is ours and isn't used after this.
    unsafe { CloseHandle(handle) };
    Ok(())
}

/// Makes a VHDX that holds only what's written after it's made, and reads the rest from `parent`,
/// which must not change from then on. Its size is the parent's. Fails if the file is there already.
pub fn create_differencing_vhdx(path: &Path, parent: &Path) -> Result<()> {
    let storage = Kind::Vhdx.storage_type();
    let parent_text = wide(parent);
    // SAFETY: an all-zero parameter block is valid, and the fields this needs are set below. A size
    // and sector sizes of zero take the parent's, as a differencing disk must.
    let mut parameters: CREATE_VIRTUAL_DISK_PARAMETERS = unsafe { std::mem::zeroed() };
    parameters.Version = CREATE_VIRTUAL_DISK_VERSION_2;
    parameters.Anonymous.Version2.ParentPath = parent_text.as_ptr();
    let text = wide(path);
    let mut handle: HANDLE = null_mut();
    // SAFETY: every pointer is to a value that outlives the call, both paths end in a zero, and
    // version 2 takes no access mask.
    let error = unsafe {
        CreateVirtualDisk(
            &storage,
            text.as_ptr(),
            VIRTUAL_DISK_ACCESS_NONE,
            null_mut(),
            CREATE_VIRTUAL_DISK_FLAG_NONE,
            0,
            &parameters,
            null(),
            &mut handle,
        )
    };
    check_win32(error, &format!("Making the disk {} on {}", path.display(), parent.display()))?;
    // SAFETY: the handle is ours and isn't used after this.
    unsafe { CloseHandle(handle) };
    Ok(())
}

/// An open virtual disk. Dropping it detaches it from the host if it was attached here.
pub struct VirtualDisk {
    handle: HANDLE,
    kind: Kind,
    path: PathBuf,
    attached: bool,
}

impl VirtualDisk {
    pub fn open(path: &Path, kind: Kind) -> Result<Self> {
        let storage = kind.storage_type();
        // SAFETY: an all-zero parameter block is valid, and the fields this needs are set below.
        let mut parameters: OPEN_VIRTUAL_DISK_PARAMETERS = unsafe { std::mem::zeroed() };
        // An ISO opens only read-only with version 1, and a VHDX with version 2 takes no mask.
        let access = match kind {
            Kind::Iso => {
                parameters.Version = OPEN_VIRTUAL_DISK_VERSION_1;
                parameters.Anonymous.Version1.RWDepth = OPEN_VIRTUAL_DISK_RW_DEPTH_DEFAULT;
                VIRTUAL_DISK_ACCESS_READ
            }
            Kind::Vhdx => {
                parameters.Version = OPEN_VIRTUAL_DISK_VERSION_2;
                VIRTUAL_DISK_ACCESS_NONE
            }
        };
        let text = wide(path);
        let mut handle: HANDLE = null_mut();
        // SAFETY: every pointer is to a value that outlives the call, and the path ends in a zero.
        let error = unsafe {
            OpenVirtualDisk(
                &storage,
                text.as_ptr(),
                access,
                OPEN_VIRTUAL_DISK_FLAG_NONE,
                &parameters,
                &mut handle,
            )
        };
        check_win32(error, &format!("Opening the disk {}", path.display()))?;
        Ok(VirtualDisk { handle, kind, path: path.to_owned(), attached: false })
    }

    /// Attaches the disk to the host until this is dropped. With `letters`, Windows gives its
    /// volumes drive letters as it would any disk's.
    pub fn attach(&mut self, letters: bool) -> Result<()> {
        let mut flags = ATTACH_VIRTUAL_DISK_FLAG_NONE;
        if self.kind == Kind::Iso {
            flags |= ATTACH_VIRTUAL_DISK_FLAG_READ_ONLY;
        }
        if !letters {
            flags |= ATTACH_VIRTUAL_DISK_FLAG_NO_DRIVE_LETTER;
        }
        let parameters = ATTACH_VIRTUAL_DISK_PARAMETERS {
            Version: ATTACH_VIRTUAL_DISK_VERSION_1,
            Anonymous: ATTACH_VIRTUAL_DISK_PARAMETERS_0 {
                Version1: ATTACH_VIRTUAL_DISK_PARAMETERS_0_0 { Reserved: 0 },
            },
        };
        // SAFETY: the handle is open, and the parameters outlive the call.
        let error = unsafe { AttachVirtualDisk(self.handle, null_mut(), flags, 0, &parameters, null()) };
        check_win32(error, &format!("Attaching the disk {}", self.path.display()))?;
        self.attached = true;
        Ok(())
    }

    /// The disk's device as the host sees it while attached, such as `\\.\PhysicalDrive3`.
    pub fn physical_path(&self) -> Result<String> {
        let mut buffer = vec![0u16; PATH_UNITS];
        let mut bytes = (buffer.len() * size_of::<u16>()) as u32;
        // SAFETY: the buffer is as many bytes long as `bytes` says.
        let error = unsafe { GetVirtualDiskPhysicalPath(self.handle, &mut bytes, buffer.as_mut_ptr()) };
        check_win32(error, &format!("Finding where {} is attached", self.path.display()))?;
        Ok(from_wide(&buffer))
    }

    /// The number Windows gave the attached disk, as in `\\.\PhysicalDrive3`.
    pub fn device_number(&self) -> Result<u32> {
        let path = self.physical_path()?;
        device_number_in(&path).ok_or_else(|| {
            Error::new(format!("Reading the number of {path}"), -1, Some("it ends in no number".into()))
        })
    }

    /// The volumes on the attached disk.
    pub fn volumes(&self) -> Result<Vec<Volume>> {
        volumes_on(self.kind.device_type(), self.device_number()?)
    }

    /// The largest the disk may grow to, in bytes, as the guest sees it.
    pub fn size(&self) -> Result<u64> {
        let mut info = GET_VIRTUAL_DISK_INFO { Version: GET_VIRTUAL_DISK_INFO_SIZE, ..Default::default() };
        let mut bytes = size_of::<GET_VIRTUAL_DISK_INFO>() as u32;
        // SAFETY: the buffer is the whole structure, as `bytes` says, and the size's part of it
        // needs nothing past it.
        let error = unsafe { GetVirtualDiskInformation(self.handle, &mut bytes, &mut info, null_mut()) };
        check_win32(error, &format!("Reading the size of {}", self.path.display()))?;
        // SAFETY: asked for its size, the disk filled in the size's part of the union.
        Ok(unsafe { info.Anonymous.Size.VirtualSize })
    }

    /// Writes what this differencing disk holds into its parent, so the parent then reads as this
    /// disk does. Any other disk made on the parent no longer reads as it did, so this is only for a
    /// parent nothing else is made on. The disk itself is left as it was.
    pub fn merge_into_parent(&self) -> Result<()> {
        let mut parameters =
            MERGE_VIRTUAL_DISK_PARAMETERS { Version: MERGE_VIRTUAL_DISK_VERSION_2, ..Default::default() };
        // Depth 1 is this disk and 2 its parent.
        parameters.Anonymous.Version2.MergeSourceDepth = 1;
        parameters.Anonymous.Version2.MergeTargetDepth = 2;
        // SAFETY: the handle is open, the parameters outlive the call, and it waits to finish.
        let error =
            unsafe { MergeVirtualDisk(self.handle, MERGE_VIRTUAL_DISK_FLAG_NONE, &parameters, null()) };
        check_win32(error, &format!("Merging {} into its parent", self.path.display()))
    }

    /// Lets the disk grow to `size_bytes`. A disk is only made larger here: the guest's partitions
    /// sit at the start, so a smaller size could cut into them.
    pub fn grow(&self, size_bytes: u64) -> Result<()> {
        let mut parameters =
            RESIZE_VIRTUAL_DISK_PARAMETERS { Version: RESIZE_VIRTUAL_DISK_VERSION_1, ..Default::default() };
        parameters.Anonymous.Version1.NewSize = size_bytes;
        // SAFETY: the handle is open, the parameters outlive the call, and it waits to finish.
        let error =
            unsafe { ResizeVirtualDisk(self.handle, RESIZE_VIRTUAL_DISK_FLAG_NONE, &parameters, null()) };
        check_win32(error, &format!("Growing {}", self.path.display()))
    }
}

impl Drop for VirtualDisk {
    fn drop(&mut self) {
        if self.attached {
            // SAFETY: the handle is open, and the disk was attached through it.
            unsafe { DetachVirtualDisk(self.handle, DETACH_VIRTUAL_DISK_FLAG_NONE, 0) };
        }
        // SAFETY: the handle is ours and isn't used after this.
        unsafe { CloseHandle(self.handle) };
    }
}

/// The number at the end of a device's path, as in `\\.\PhysicalDrive3` or `\\.\CDROM1`.
fn device_number_in(path: &str) -> Option<u32> {
    let digits = path.len() - path.bytes().rev().take_while(u8::is_ascii_digit).count();
    path[digits..].parse().ok()
}

/// Where Windows keeps each volume's drive letter, including volumes that aren't attached now.
const MOUNTED_DEVICES: &str = r"SYSTEM\MountedDevices";
/// How a drive letter's entry there is named, before its letter.
const DOS_DEVICE: &str = r"\DosDevices\";
/// Room for an entry's name there.
const VALUE_NAME_UNITS: usize = 256;

/// Drive letters that can't be given to a new volume: those in use, a drive with no media in it
/// included, and those Windows keeps for a volume that isn't attached now. A path test misses both
/// of the last two, and diskpart refuses such a letter.
pub fn letters_taken() -> Vec<char> {
    // SAFETY: GetLogicalDrives only reads which letters are in use.
    let in_use = unsafe { GetLogicalDrives() };
    let mut taken: Vec<char> =
        (0..26u8).filter(|bit| in_use & (1 << bit) != 0).map(|bit| (b'A' + bit) as char).collect();
    taken.extend(reserved_letters());
    taken.sort_unstable();
    taken.dedup();
    taken
}

/// The letters MountedDevices keeps. Reading it needs an administrator, so without one this is
/// empty, and only the letters in use count.
fn reserved_letters() -> Vec<char> {
    let path = wide(MOUNTED_DEVICES);
    let mut key: HKEY = null_mut();
    // SAFETY: the path ends in a zero, and `key` is a place for the key's handle.
    if unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, path.as_ptr(), 0, KEY_READ, &mut key) } != 0 {
        return Vec::new();
    }
    let mut found = Vec::new();
    for index in 0.. {
        let mut name = [0u16; VALUE_NAME_UNITS];
        let mut length = name.len() as u32;
        // SAFETY: the key is open, the name buffer is as long as `length` says, and the type and
        // data aren't asked for.
        let read = unsafe {
            RegEnumValueW(
                key,
                index,
                name.as_mut_ptr(),
                &mut length,
                null(),
                null_mut(),
                null_mut(),
                null_mut(),
            )
        };
        if read == ERROR_NO_MORE_ITEMS {
            break;
        }
        if read != 0 {
            continue;
        }
        if let Some(letter) = reserved_letter(&from_wide(&name)) {
            found.push(letter);
        }
    }
    // SAFETY: the key is ours and isn't used after this.
    unsafe { RegCloseKey(key) };
    found
}

/// The letter in a MountedDevices entry's name, such as `\DosDevices\E:`.
fn reserved_letter(name: &str) -> Option<char> {
    let rest = name.strip_prefix(DOS_DEVICE)?;
    let mut characters = rest.chars();
    let letter = characters.next()?.to_ascii_uppercase();
    (letter.is_ascii_uppercase() && characters.as_str() == ":").then_some(letter)
}

/// A volume on a disk.
#[derive(Debug, Clone)]
pub struct Volume {
    /// `\\?\Volume{...}\`, which reaches the volume's files with or without a drive letter.
    pub root: PathBuf,
    /// Drive letters and folders it's mounted on, such as `E:\`.
    pub mount_points: Vec<PathBuf>,
}

/// The volumes on the device of this type and number.
pub fn volumes_on(device_type: u32, number: u32) -> Result<Vec<Volume>> {
    let mut found = Vec::new();
    let mut name = [0u16; VOLUME_NAME_UNITS];
    // SAFETY: the buffer is as long as the count given.
    let search = unsafe { FindFirstVolumeW(name.as_mut_ptr(), name.len() as u32) };
    if search == INVALID_HANDLE_VALUE {
        return Err(Error::last("Listing the host's volumes"));
    }
    loop {
        let root = from_wide(&name);
        if device_of(&root) == Some((device_type, number)) {
            found.push(Volume { mount_points: mount_points(&name), root: PathBuf::from(root) });
        }
        // SAFETY: as above, with the search handle still open.
        if unsafe { FindNextVolumeW(search, name.as_mut_ptr(), name.len() as u32) } == 0 {
            break;
        }
    }
    // SAFETY: the search handle is ours and isn't used after this.
    unsafe { FindVolumeClose(search) };
    Ok(found)
}

/// The type and number of the device a volume is on, given its GUID path.
fn device_of(root: &str) -> Option<(u32, u32)> {
    // A volume opens as a device without its last backslash.
    let device = open_device(root.trim_end_matches('\\'))?;
    let mut number = STORAGE_DEVICE_NUMBER { DeviceType: 0, DeviceNumber: 0, PartitionNumber: 0 };
    let mut written = 0;
    // SAFETY: the output is a STORAGE_DEVICE_NUMBER of the size given.
    let asked = unsafe {
        DeviceIoControl(
            device.0,
            IOCTL_STORAGE_GET_DEVICE_NUMBER,
            null(),
            0,
            (&raw mut number).cast(),
            size_of::<STORAGE_DEVICE_NUMBER>() as u32,
            &mut written,
            null_mut(),
        )
    };
    (asked != 0).then_some((number.DeviceType, number.DeviceNumber))
}

/// Whether the host's disk of this number is a file attached as a disk, such as a VHDX. A disk is
/// only ever wiped after this says yes, as a wrong number there would wipe a real disk.
pub fn is_file_backed(disk_number: u32) -> Result<bool> {
    let path = format!(r"\\.\PhysicalDrive{disk_number}");
    let device = open_device(&path).ok_or_else(|| Error::last(format!("Opening {path}")))?;
    let query = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageDeviceProperty,
        QueryType: PropertyStandardQuery,
        AdditionalParameters: [0],
    };
    // SAFETY: an all-zero descriptor is valid, and the call fills what fits of it.
    let mut descriptor: STORAGE_DEVICE_DESCRIPTOR = unsafe { std::mem::zeroed() };
    let mut written = 0;
    // SAFETY: the input is a STORAGE_PROPERTY_QUERY and the output a STORAGE_DEVICE_DESCRIPTOR, each
    // of the size given.
    let asked = unsafe {
        DeviceIoControl(
            device.0,
            IOCTL_STORAGE_QUERY_PROPERTY,
            (&raw const query).cast(),
            size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            (&raw mut descriptor).cast(),
            size_of::<STORAGE_DEVICE_DESCRIPTOR>() as u32,
            &mut written,
            null_mut(),
        )
    };
    if asked == 0 {
        return Err(Error::last(format!("Asking what {path} is")));
    }
    Ok(descriptor.BusType == BusTypeFileBackedVirtual)
}

/// A device opened only to ask it questions, closed when dropped.
struct Device(HANDLE);

fn open_device(path: &str) -> Option<Device> {
    let text = wide(path);
    // SAFETY: the path ends in a zero, and no access is asked for, so nothing can be read or written.
    let handle = unsafe {
        CreateFileW(
            text.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null(),
            OPEN_EXISTING,
            0,
            null_mut(),
        )
    };
    (handle != INVALID_HANDLE_VALUE).then_some(Device(handle))
}

impl Drop for Device {
    fn drop(&mut self) {
        // SAFETY: the handle is ours and isn't used after this.
        unsafe { CloseHandle(self.0) };
    }
}

fn mount_points(volume_name: &[u16]) -> Vec<PathBuf> {
    let mut buffer = vec![0u16; PATH_UNITS];
    let mut needed = 0;
    // SAFETY: the volume name ends in a zero, and the buffer is as long as the count given.
    let asked = unsafe {
        GetVolumePathNamesForVolumeNameW(
            volume_name.as_ptr(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            &mut needed,
        )
    };
    if asked == 0 {
        return Vec::new();
    }
    // The answer is a list of strings, each ending in a zero, with one more zero at the end.
    buffer
        .split(|&unit| unit == 0)
        .take_while(|part| !part.is_empty())
        .map(|part| from_wide(part).into())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The host's first disk is a real one on nearly every PC, so this is run by hand only.
    #[test]
    #[ignore = "reads the host's own first disk"]
    fn the_hosts_first_disk_is_not_a_file() {
        assert!(!is_file_backed(0).unwrap());
    }

    #[test]
    fn a_reserved_letter_is_read_from_its_entrys_name() {
        assert_eq!(reserved_letter(r"\DosDevices\E:"), Some('E'));
        assert_eq!(reserved_letter(r"\DosDevices\z:"), Some('Z'));
        assert_eq!(reserved_letter(r"\??\Volume{1234}"), None);
        assert_eq!(reserved_letter(r"\DosDevices\E:x"), None);
    }

    #[test]
    fn the_system_drives_letter_is_taken() {
        let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        assert!(letters_taken().contains(&system.chars().next().unwrap().to_ascii_uppercase()));
    }

    #[test]
    fn a_device_number_is_read_from_the_end_of_its_path() {
        assert_eq!(device_number_in(r"\\.\PhysicalDrive3"), Some(3));
        assert_eq!(device_number_in(r"\\.\CDROM12"), Some(12));
        assert_eq!(device_number_in(r"\\.\PhysicalDrive"), None);
    }
    /// Makes, reads and grows a disk file, with nothing attached, which needs no elevation.
    #[test]
    fn a_disk_grows_and_says_its_new_size() {
        const GB: u64 = 1 << 30;
        let path = std::env::temp_dir().join(format!("gh-grow-{}.vhdx", std::process::id()));
        let _ = std::fs::remove_file(&path);
        create_vhdx(&path, GB).unwrap();
        let opened = VirtualDisk::open(&path, Kind::Vhdx).unwrap();
        assert_eq!(opened.size().unwrap(), GB);
        opened.grow(3 * GB).unwrap();
        drop(opened);
        assert_eq!(VirtualDisk::open(&path, Kind::Vhdx).unwrap().size().unwrap(), 3 * GB);
        std::fs::remove_file(&path).unwrap();
    }

    /// A chain as snapshots make it, with nothing attached: a child made on a disk, the child moved
    /// aside and still finding its parent, a grandchild made on it, and the grandchild merged into it.
    #[test]
    fn a_differencing_chain_is_made_moved_and_merged() {
        const GB: u64 = 1 << 30;
        let folder = std::env::temp_dir().join(format!("gh-chain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(folder.join("snapshots")).unwrap();
        let (base, child, moved, grandchild) = (
            folder.join("base.vhdx"),
            folder.join("child.vhdx"),
            folder.join("snapshots").join("1.vhdx"),
            folder.join("grandchild.vhdx"),
        );
        create_vhdx(&base, 2 * GB).unwrap();
        create_differencing_vhdx(&child, &base).unwrap();
        assert_eq!(VirtualDisk::open(&child, Kind::Vhdx).unwrap().size().unwrap(), 2 * GB);
        std::fs::rename(&child, &moved).unwrap();
        VirtualDisk::open(&moved, Kind::Vhdx).expect("a moved child still finds its parent");
        create_differencing_vhdx(&grandchild, &moved).unwrap();
        VirtualDisk::open(&grandchild, Kind::Vhdx).unwrap().merge_into_parent().unwrap();
        std::fs::rename(&moved, &grandchild).unwrap();
        assert_eq!(VirtualDisk::open(&grandchild, Kind::Vhdx).unwrap().size().unwrap(), 2 * GB);
        assert!(create_differencing_vhdx(&folder.join("orphan.vhdx"), &folder.join("missing.vhdx")).is_err());
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
