//! WinFsp's DLL, found through the registry and loaded at run time: nothing here links against
//! WinFsp, so the program builds and starts (`--help`, `--check`) on a machine without it.

use crate::ffi::{DirInfo, FileSystem, Interface, NtStatus, Pvoid, Pwstr, VolumeParams};
use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};

type Create =
    unsafe extern "C" fn(Pwstr, *const VolumeParams, *const Interface, *mut *mut FileSystem) -> NtStatus;
type SetMountPoint = unsafe extern "C" fn(*mut FileSystem, Pwstr) -> NtStatus;
type StartDispatcher = unsafe extern "C" fn(*mut FileSystem, u32) -> NtStatus;
type FsOnly = unsafe extern "C" fn(*mut FileSystem);
type AddDirInfo = unsafe extern "C" fn(*mut DirInfo, Pvoid, u32, *mut u32) -> u8;

/// The WinFsp functions this program calls.
pub struct Api {
    create: Create,
    set_mount_point: SetMountPoint,
    start_dispatcher: StartDispatcher,
    stop_dispatcher: FsOnly,
    delete: FsOnly,
    add_dir_info: AddDirInfo,
}

/// A wide string, with its terminator.
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// `HKLM\SOFTWARE\WOW6432Node\WinFsp`'s `InstallDir`, where WinFsp's installer says it went.
fn install_dir() -> Result<PathBuf, String> {
    let key = wide(r"SOFTWARE\WOW6432Node\WinFsp");
    let value = wide("InstallDir");
    let mut bytes = 0u32;
    // SAFETY: both names are terminated, and with no buffer this only says how large one must be.
    let found = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            null_mut(),
            &mut bytes,
        )
    };
    if found != ERROR_SUCCESS {
        return Err(format!("WinFsp is not installed (its registry key gave error {found})"));
    }
    let mut buffer = vec![0u16; (bytes as usize).div_ceil(2) + 1];
    let mut bytes = (buffer.len() * 2) as u32;
    // SAFETY: `buffer` holds `bytes` bytes.
    let found = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if found != ERROR_SUCCESS {
        return Err(format!("WinFsp's InstallDir could not be read (error {found})"));
    }
    let units = buffer.iter().position(|&u| u == 0).unwrap_or(buffer.len());
    Ok(PathBuf::from(OsString::from_wide(&buffer[..units])))
}

impl Api {
    /// Finds WinFsp and loads its functions.
    pub fn load() -> Result<Api, String> {
        let dll = install_dir()?.join("bin").join("winfsp-x64.dll");
        let name: Vec<u16> = dll.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: `name` is terminated. Loading WinFsp's own DLL runs its start-up code, as meant.
        let module = unsafe { LoadLibraryW(name.as_ptr()) };
        if module.is_null() {
            return Err(format!("{} could not be loaded", dll.display()));
        }
        macro_rules! function {
            ($name:literal, $ty:ty) => {{
                // SAFETY: the module is loaded and the name is a terminated string. The pointer is
                // WinFsp's function of that name, whose signature the type here is written from.
                match unsafe { GetProcAddress(module, concat!($name, "\0").as_ptr()) } {
                    Some(f) => unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, $ty>(f) },
                    None => return Err(format!("winfsp-x64.dll has no {}", $name)),
                }
            }};
        }
        Ok(Api {
            create: function!("FspFileSystemCreate", Create),
            set_mount_point: function!("FspFileSystemSetMountPoint", SetMountPoint),
            start_dispatcher: function!("FspFileSystemStartDispatcher", StartDispatcher),
            stop_dispatcher: function!("FspFileSystemStopDispatcher", FsOnly),
            delete: function!("FspFileSystemDelete", FsOnly),
            add_dir_info: function!("FspFileSystemAddDirInfo", AddDirInfo),
        })
    }

    /// Creates a file system object for a disk with these parameters and callbacks. The table must
    /// outlive it.
    ///
    /// # Safety
    /// `interface`'s callbacks must be sound for WinFsp to call from its threads.
    pub unsafe fn create(
        &self,
        device: &str,
        params: &VolumeParams,
        interface: &'static Interface,
    ) -> Result<*mut FileSystem, NtStatus> {
        let mut device = wide(device);
        let mut fs = null_mut();
        // SAFETY: the pointers are valid for the call, and the table lives forever.
        let status = unsafe { (self.create)(device.as_mut_ptr(), params, interface, &mut fs) };
        if status >= 0 { Ok(fs) } else { Err(status) }
    }

    /// Puts the file system at `mount_point`, a drive such as `S:`.
    ///
    /// # Safety
    /// `fs` must be a live file system from `create`.
    pub unsafe fn set_mount_point(&self, fs: *mut FileSystem, mount_point: &str) -> Result<(), NtStatus> {
        let mut mount_point = wide(mount_point);
        // SAFETY: as said.
        let status = unsafe { (self.set_mount_point)(fs, mount_point.as_mut_ptr()) };
        if status >= 0 { Ok(()) } else { Err(status) }
    }

    /// Starts the threads that call the callbacks.
    ///
    /// # Safety
    /// `fs` must be a live file system from `create`.
    pub unsafe fn start_dispatcher(&self, fs: *mut FileSystem) -> Result<(), NtStatus> {
        // SAFETY: as said. Zero threads is WinFsp's default number.
        let status = unsafe { (self.start_dispatcher)(fs, 0) };
        if status >= 0 { Ok(()) } else { Err(status) }
    }

    /// Stops the dispatcher, which waits for its threads, so it must not be called from a callback.
    ///
    /// # Safety
    /// `fs` must be a live file system from `create`.
    pub unsafe fn stop_dispatcher(&self, fs: *mut FileSystem) {
        // SAFETY: as said.
        unsafe { (self.stop_dispatcher)(fs) }
    }

    /// Destroys the file system, which also unmounts it.
    ///
    /// # Safety
    /// `fs` must be from `create`, with its dispatcher stopped, and is not used again.
    pub unsafe fn delete(&self, fs: *mut FileSystem) {
        // SAFETY: as said.
        unsafe { (self.delete)(fs) }
    }

    /// Adds one entry to a `ReadDirectory` buffer; `entry` null ends the listing. False means the
    /// buffer is full and the entry did not go in.
    ///
    /// # Safety
    /// `entry` is null or a `DirInfo` with its name after it, and `buffer` is the `length` bytes
    /// of the callback, with `transferred` its count.
    pub unsafe fn add_dir_info(
        &self,
        entry: *mut DirInfo,
        buffer: Pvoid,
        length: u32,
        transferred: *mut u32,
    ) -> bool {
        // SAFETY: as said.
        unsafe { (self.add_dir_info)(entry, buffer, length, transferred) != 0 }
    }
}
