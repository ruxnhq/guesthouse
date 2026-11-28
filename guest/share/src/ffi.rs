//! WinFsp's structures and callback table, written out by hand, as WinFsp's own DLL is loaded at
//! run time and never linked.
//!
//! The layouts are those of WinFsp's `winfsp/fsctl.h` and `winfsp/winfsp.h`, as the MIT-licensed
//! pre-generated bindings in `winfsp_wrs_sys` 0.4.1 (Scille) have them. Their sizes and field
//! offsets are asserted below, where the bindings' own layout tests give them. These are plain
//! `repr(C)` types with no Windows crate in them, so the assertions are checked on Linux too, for
//! 64-bit pointers, which is all Windows builds this for.

use std::ffi::c_void;

pub type NtStatus = i32;
pub type Pwstr = *mut u16;
pub type Pvoid = *mut c_void;
/// A callback WinFsp has and this program doesn't implement: the slot is there and is null.
pub type Unused = Option<unsafe extern "C" fn()>;

/// `FSP_FSCTL_DISK_DEVICE_NAME`, for `FspFileSystemCreate`: a disk, not a network file system.
pub const DISK_DEVICE_NAME: &str = "WinFsp.Disk";

/// `FSP_FSCTL_VOLUME_PARAMS`, as WinFsp 2 has it.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VolumeParams {
    pub version: u16,
    pub sector_size: u16,
    pub sectors_per_allocation_unit: u16,
    pub max_component_length: u16,
    pub volume_creation_time: u64,
    pub volume_serial_number: u32,
    pub transact_timeout: u32,
    pub irp_timeout: u32,
    pub irp_capacity: u32,
    pub file_info_timeout: u32,
    /// The first bit-field word: see the `FLAG_*` constants.
    pub flags: u32,
    pub prefix: [u16; 192],
    pub file_system_name: [u16; 16],
    /// The second bit-field word: the `*TimeoutValid` bits, none of which are set here.
    pub flags2: u32,
    pub volume_info_timeout: u32,
    pub dir_info_timeout: u32,
    pub security_timeout: u32,
    pub stream_info_timeout: u32,
    pub ea_timeout: u32,
    pub fsext_control_code: u32,
    pub reserved32: [u32; 1],
    pub reserved64: [u64; 2],
}

/// Bits of `VolumeParams::flags`, by position in the bindings' bit-field.
pub const FLAG_CASE_SENSITIVE_SEARCH: u32 = 1 << 0;
pub const FLAG_CASE_PRESERVED_NAMES: u32 = 1 << 1;
pub const FLAG_UNICODE_ON_DISK: u32 = 1 << 2;
pub const FLAG_READ_ONLY_VOLUME: u32 = 1 << 9;
/// The `FileContext` WinFsp hands the callbacks is the open handle's own (`UserContext2`). Without
/// it, it's the file's, which every handle to the file shares, and a context made for each open
/// and freed at each close, as this program's are, is freed twice once a file is open twice: a test
/// had Explorer crash all three drives' programs with heap corruption (0xc0000374) that way.
pub const FLAG_UM_FILE_CONTEXT_IS_USER_CONTEXT2: u32 = 1 << 16;

/// `FSP_FSCTL_VOLUME_INFO`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VolumeInfo {
    pub total_size: u64,
    pub free_size: u64,
    /// In bytes.
    pub volume_label_length: u16,
    pub volume_label: [u16; 32],
}

/// `FSP_FSCTL_FILE_INFO`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FileInfo {
    pub file_attributes: u32,
    pub reparse_tag: u32,
    pub allocation_size: u64,
    pub file_size: u64,
    pub creation_time: u64,
    pub last_access_time: u64,
    pub last_write_time: u64,
    pub change_time: u64,
    pub index_number: u64,
    pub hard_links: u32,
    pub ea_size: u32,
}

/// `FSP_FSCTL_DIR_INFO` up to its name, which follows it in memory.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct DirInfo {
    /// This header's 104 bytes and the name's, which has no terminator.
    pub size: u16,
    pub file_info: FileInfo,
    /// The union of `NextOffset` and 24 bytes of padding, which WinFsp owns.
    pub next_offset_or_padding: [u64; 3],
}

/// A `DirInfo` and room after it for the longest name a component may have.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct DirInfoBuf {
    pub info: DirInfo,
    pub name: [u16; MAX_NAME_UNITS],
}

/// The most UTF-16 units of a name, which `MaxComponentLength` also says.
pub const MAX_NAME_UNITS: usize = 255;

/// `FSP_FILE_SYSTEM`, as far as this program reads it. Its `UserContext` is the one field a
/// program may use; the rest is WinFsp's.
#[repr(C)]
pub struct FileSystem {
    pub version: u16,
    pub user_context: Pvoid,
    pub volume_name: [u16; 256],
    pub volume_handle: Pvoid,
    pub enter_operation: Pvoid,
    pub leave_operation: Pvoid,
    pub operations: [Pvoid; 22],
    pub interface: *const Interface,
    pub dispatcher_thread: Pvoid,
    pub dispatcher_thread_count: u32,
    pub dispatcher_result: NtStatus,
    pub mount_point: Pwstr,
    pub mount_handle: Pvoid,
    pub debug_log: u32,
    pub op_guard_strategy: u32,
    pub op_guard_lock: Pvoid,
    pub um_file_context_is_user_context2: u8,
    pub um_file_context_is_full_context: u8,
    pub bits: u16,
    pub padding: u32,
}

type Fs = *mut FileSystem;

/// `FSP_FILE_SYSTEM_INTERFACE`: every slot, in WinFsp's order, so the table is the size WinFsp
/// reads. The ones this program doesn't implement are `Unused`.
#[repr(C)]
pub struct Interface {
    pub get_volume_info: Option<unsafe extern "C" fn(Fs, *mut VolumeInfo) -> NtStatus>,
    pub set_volume_label: Unused,
    pub get_security_by_name:
        Option<unsafe extern "C" fn(Fs, Pwstr, *mut u32, Pvoid, *mut usize) -> NtStatus>,
    #[allow(clippy::type_complexity)]
    pub create: Option<
        unsafe extern "C" fn(Fs, Pwstr, u32, u32, u32, Pvoid, u64, *mut Pvoid, *mut FileInfo) -> NtStatus,
    >,
    pub open: Option<unsafe extern "C" fn(Fs, Pwstr, u32, u32, *mut Pvoid, *mut FileInfo) -> NtStatus>,
    pub overwrite: Option<unsafe extern "C" fn(Fs, Pvoid, u32, u8, u64, *mut FileInfo) -> NtStatus>,
    pub cleanup: Option<unsafe extern "C" fn(Fs, Pvoid, Pwstr, u32)>,
    pub close: Option<unsafe extern "C" fn(Fs, Pvoid)>,
    pub read: Option<unsafe extern "C" fn(Fs, Pvoid, Pvoid, u64, u32, *mut u32) -> NtStatus>,
    #[allow(clippy::type_complexity)]
    pub write:
        Option<unsafe extern "C" fn(Fs, Pvoid, Pvoid, u64, u32, u8, u8, *mut u32, *mut FileInfo) -> NtStatus>,
    pub flush: Option<unsafe extern "C" fn(Fs, Pvoid, *mut FileInfo) -> NtStatus>,
    pub get_file_info: Option<unsafe extern "C" fn(Fs, Pvoid, *mut FileInfo) -> NtStatus>,
    #[allow(clippy::type_complexity)]
    pub set_basic_info:
        Option<unsafe extern "C" fn(Fs, Pvoid, u32, u64, u64, u64, u64, *mut FileInfo) -> NtStatus>,
    pub set_file_size: Option<unsafe extern "C" fn(Fs, Pvoid, u64, u8, *mut FileInfo) -> NtStatus>,
    pub can_delete: Option<unsafe extern "C" fn(Fs, Pvoid, Pwstr) -> NtStatus>,
    pub rename: Option<unsafe extern "C" fn(Fs, Pvoid, Pwstr, Pwstr, u8) -> NtStatus>,
    pub get_security: Option<unsafe extern "C" fn(Fs, Pvoid, Pvoid, *mut usize) -> NtStatus>,
    pub set_security: Unused,
    pub read_directory:
        Option<unsafe extern "C" fn(Fs, Pvoid, Pwstr, Pwstr, Pvoid, u32, *mut u32) -> NtStatus>,
    pub resolve_reparse_points: Unused,
    pub get_reparse_point: Unused,
    pub set_reparse_point: Unused,
    pub delete_reparse_point: Unused,
    pub get_stream_info: Unused,
    pub get_dir_info_by_name: Unused,
    pub control: Unused,
    pub set_delete: Unused,
    pub create_ex: Unused,
    pub overwrite_ex: Unused,
    pub get_ea: Unused,
    pub set_ea: Unused,
    pub obsolete0: Unused,
    pub dispatcher_stopped: Unused,
    pub reserved: [Unused; 31],
}

/// `FspCleanupDelete`, a flag of `Cleanup`.
pub const CLEANUP_DELETE: u32 = 1;

/// `FILE_DIRECTORY_FILE`, `FILE_NON_DIRECTORY_FILE`, `FILE_DELETE_ON_CLOSE`: bits of a create's
/// options.
pub const FILE_DIRECTORY_FILE: u32 = 0x1;
pub const FILE_NON_DIRECTORY_FILE: u32 = 0x40;

#[cfg(target_pointer_width = "64")]
mod layout {
    use super::*;
    use std::mem::{align_of, offset_of, size_of};

    const _: () = assert!(size_of::<VolumeParams>() == 504 && align_of::<VolumeParams>() == 8);
    const _: () = assert!(offset_of!(VolumeParams, version) == 0);
    const _: () = assert!(offset_of!(VolumeParams, sector_size) == 2);
    const _: () = assert!(offset_of!(VolumeParams, sectors_per_allocation_unit) == 4);
    const _: () = assert!(offset_of!(VolumeParams, max_component_length) == 6);
    const _: () = assert!(offset_of!(VolumeParams, volume_creation_time) == 8);
    const _: () = assert!(offset_of!(VolumeParams, volume_serial_number) == 16);
    const _: () = assert!(offset_of!(VolumeParams, transact_timeout) == 20);
    const _: () = assert!(offset_of!(VolumeParams, irp_timeout) == 24);
    const _: () = assert!(offset_of!(VolumeParams, irp_capacity) == 28);
    const _: () = assert!(offset_of!(VolumeParams, file_info_timeout) == 32);
    const _: () = assert!(offset_of!(VolumeParams, flags) == 36);
    const _: () = assert!(offset_of!(VolumeParams, prefix) == 40);
    const _: () = assert!(offset_of!(VolumeParams, file_system_name) == 424);
    const _: () = assert!(offset_of!(VolumeParams, flags2) == 456);
    const _: () = assert!(offset_of!(VolumeParams, volume_info_timeout) == 460);
    const _: () = assert!(offset_of!(VolumeParams, dir_info_timeout) == 464);
    const _: () = assert!(offset_of!(VolumeParams, security_timeout) == 468);
    const _: () = assert!(offset_of!(VolumeParams, stream_info_timeout) == 472);
    const _: () = assert!(offset_of!(VolumeParams, ea_timeout) == 476);
    const _: () = assert!(offset_of!(VolumeParams, fsext_control_code) == 480);
    const _: () = assert!(offset_of!(VolumeParams, reserved32) == 484);
    const _: () = assert!(offset_of!(VolumeParams, reserved64) == 488);

    const _: () = assert!(size_of::<VolumeInfo>() == 88 && align_of::<VolumeInfo>() == 8);
    const _: () = assert!(offset_of!(VolumeInfo, total_size) == 0);
    const _: () = assert!(offset_of!(VolumeInfo, free_size) == 8);
    const _: () = assert!(offset_of!(VolumeInfo, volume_label_length) == 16);
    const _: () = assert!(offset_of!(VolumeInfo, volume_label) == 18);

    const _: () = assert!(size_of::<FileInfo>() == 72 && align_of::<FileInfo>() == 8);
    const _: () = assert!(offset_of!(FileInfo, file_attributes) == 0);
    const _: () = assert!(offset_of!(FileInfo, reparse_tag) == 4);
    const _: () = assert!(offset_of!(FileInfo, allocation_size) == 8);
    const _: () = assert!(offset_of!(FileInfo, file_size) == 16);
    const _: () = assert!(offset_of!(FileInfo, creation_time) == 24);
    const _: () = assert!(offset_of!(FileInfo, last_access_time) == 32);
    const _: () = assert!(offset_of!(FileInfo, last_write_time) == 40);
    const _: () = assert!(offset_of!(FileInfo, change_time) == 48);
    const _: () = assert!(offset_of!(FileInfo, index_number) == 56);
    const _: () = assert!(offset_of!(FileInfo, hard_links) == 64);
    const _: () = assert!(offset_of!(FileInfo, ea_size) == 68);

    // WinFsp's FSP_FSCTL_DIR_INFO is 104 bytes up to FileNameBuf, with FileInfo at 8.
    const _: () = assert!(size_of::<DirInfo>() == 104 && align_of::<DirInfo>() == 8);
    const _: () = assert!(offset_of!(DirInfo, size) == 0);
    const _: () = assert!(offset_of!(DirInfo, file_info) == 8);
    const _: () = assert!(offset_of!(DirInfo, next_offset_or_padding) == 80);
    const _: () = assert!(offset_of!(DirInfoBuf, name) == 104);

    const _: () = assert!(size_of::<FileSystem>() == 792 && align_of::<FileSystem>() == 8);
    const _: () = assert!(offset_of!(FileSystem, version) == 0);
    const _: () = assert!(offset_of!(FileSystem, user_context) == 8);
    const _: () = assert!(offset_of!(FileSystem, volume_name) == 16);
    const _: () = assert!(offset_of!(FileSystem, volume_handle) == 528);
    const _: () = assert!(offset_of!(FileSystem, enter_operation) == 536);
    const _: () = assert!(offset_of!(FileSystem, leave_operation) == 544);
    const _: () = assert!(offset_of!(FileSystem, operations) == 552);
    const _: () = assert!(offset_of!(FileSystem, interface) == 728);
    const _: () = assert!(offset_of!(FileSystem, dispatcher_thread) == 736);
    const _: () = assert!(offset_of!(FileSystem, dispatcher_thread_count) == 744);
    const _: () = assert!(offset_of!(FileSystem, dispatcher_result) == 748);
    const _: () = assert!(offset_of!(FileSystem, mount_point) == 752);
    const _: () = assert!(offset_of!(FileSystem, mount_handle) == 760);
    const _: () = assert!(offset_of!(FileSystem, debug_log) == 768);
    const _: () = assert!(offset_of!(FileSystem, op_guard_strategy) == 772);
    const _: () = assert!(offset_of!(FileSystem, op_guard_lock) == 776);
    const _: () = assert!(offset_of!(FileSystem, um_file_context_is_user_context2) == 784);
    const _: () = assert!(offset_of!(FileSystem, um_file_context_is_full_context) == 785);

    const _: () = assert!(size_of::<Interface>() == 512 && align_of::<Interface>() == 8);
    const _: () = assert!(offset_of!(Interface, get_volume_info) == 0);
    const _: () = assert!(offset_of!(Interface, set_volume_label) == 8);
    const _: () = assert!(offset_of!(Interface, get_security_by_name) == 16);
    const _: () = assert!(offset_of!(Interface, create) == 24);
    const _: () = assert!(offset_of!(Interface, open) == 32);
    const _: () = assert!(offset_of!(Interface, overwrite) == 40);
    const _: () = assert!(offset_of!(Interface, cleanup) == 48);
    const _: () = assert!(offset_of!(Interface, close) == 56);
    const _: () = assert!(offset_of!(Interface, read) == 64);
    const _: () = assert!(offset_of!(Interface, write) == 72);
    const _: () = assert!(offset_of!(Interface, flush) == 80);
    const _: () = assert!(offset_of!(Interface, get_file_info) == 88);
    const _: () = assert!(offset_of!(Interface, set_basic_info) == 96);
    const _: () = assert!(offset_of!(Interface, set_file_size) == 104);
    const _: () = assert!(offset_of!(Interface, can_delete) == 112);
    const _: () = assert!(offset_of!(Interface, rename) == 120);
    const _: () = assert!(offset_of!(Interface, get_security) == 128);
    const _: () = assert!(offset_of!(Interface, set_security) == 136);
    const _: () = assert!(offset_of!(Interface, read_directory) == 144);
    const _: () = assert!(offset_of!(Interface, resolve_reparse_points) == 152);
    const _: () = assert!(offset_of!(Interface, get_reparse_point) == 160);
    const _: () = assert!(offset_of!(Interface, set_reparse_point) == 168);
    const _: () = assert!(offset_of!(Interface, delete_reparse_point) == 176);
    const _: () = assert!(offset_of!(Interface, get_stream_info) == 184);
    const _: () = assert!(offset_of!(Interface, get_dir_info_by_name) == 192);
    const _: () = assert!(offset_of!(Interface, control) == 200);
    const _: () = assert!(offset_of!(Interface, set_delete) == 208);
    const _: () = assert!(offset_of!(Interface, create_ex) == 216);
    const _: () = assert!(offset_of!(Interface, overwrite_ex) == 224);
    const _: () = assert!(offset_of!(Interface, get_ea) == 232);
    const _: () = assert!(offset_of!(Interface, set_ea) == 240);
    const _: () = assert!(offset_of!(Interface, obsolete0) == 248);
    const _: () = assert!(offset_of!(Interface, dispatcher_stopped) == 256);
    const _: () = assert!(offset_of!(Interface, reserved) == 264);
}

impl VolumeParams {
    /// A table of zeros, with the version WinFsp 2 expects.
    pub fn zeroed() -> VolumeParams {
        // SAFETY: every field is an integer or an array of them, for which zero is a value.
        let mut p: VolumeParams = unsafe { std::mem::zeroed() };
        p.version = std::mem::size_of::<VolumeParams>() as u16;
        p
    }
}

impl Interface {
    /// A table with no callback in it.
    pub fn empty() -> Interface {
        // SAFETY: every field is an `Option` of a function pointer, for which zero is `None`.
        unsafe { std::mem::zeroed() }
    }
}

impl DirInfoBuf {
    /// A zeroed buffer, so WinFsp's own padding is zero.
    pub fn zeroed() -> DirInfoBuf {
        // SAFETY: integers only.
        unsafe { std::mem::zeroed() }
    }
}
