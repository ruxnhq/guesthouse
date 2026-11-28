//! The jail on a Windows host, for a program of ours that reads what a guest sends. Windows has no
//! Landlock or seccomp, and a process can't jail itself after it starts, so the parent starts it
//! jailed, with the same three promises as on Linux:
//!
//! - Files and network, as Landlock holds them: the program runs in an AppContainer with no
//!   capabilities. It may open only what's granted to that container, which is the program's own
//!   file and the system's DLLs, and it has no network at all, loopback included.
//! - System calls, as seccomp holds them: it can make no win32k calls, so the window manager's half
//!   of the kernel is out of reach, and it can't make code at run time, load DLLs from a share or a
//!   low-integrity file, take extension DLLs into itself, or use a handle that isn't valid.
//! - No new privileges: it can start no process, by policy and by its job, and it stops when the
//!   job's last handle closes, so it can't outlive the program that started it.
//!
//! Its standard input, output and error are the handles the parent gives, and nothing else is
//! inherited. That's how the guest's bytes get in and the checked text gets out.

use std::ffi::OsStr;
use std::io;
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::path::Path;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, HANDLE, HANDLE_FLAG_INHERIT, LocalFree, SetHandleInformation, WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::Authorization::{
    EXPLICIT_ACCESS_W, GRANT_ACCESS, GetNamedSecurityInfoW, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT,
    SetEntriesInAclW, SetNamedSecurityInfoW, TRUSTEE_IS_SID, TRUSTEE_IS_WELL_KNOWN_GROUP, TRUSTEE_W,
};
use windows_sys::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
};
use windows_sys::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, FreeSid, GetTokenInformation, NO_INHERITANCE, PSECURITY_DESCRIPTOR, PSID,
    SECURITY_CAPABILITIES, TOKEN_QUERY, TokenIsAppContainer,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_GENERIC_EXECUTE, FILE_GENERIC_READ};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOBOBJECT_BASIC_UI_RESTRICTIONS, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicUIRestrictions, JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CreateProcessW, DETACHED_PROCESS, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess, GetExitCodeProcess, GetProcessMitigationPolicy,
    INFINITE, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST, OpenProcessToken,
    PROC_THREAD_ATTRIBUTE_CHILD_PROCESS_POLICY, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
    PROCESS_INFORMATION, ProcessChildProcessPolicy, ProcessDynamicCodePolicy, ProcessSystemCallDisablePolicy,
    ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute,
    WaitForSingleObject,
};

/// Process mitigations, by the bits Windows' SDK gives them in the first word of
/// `PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY`. Each policy has four bits, and 1 in them is "always on".
mod mitigation {
    /// Using a handle that isn't valid stops the process, rather than failing the call.
    pub const STRICT_HANDLE_CHECKS: u64 = 1 << 24;
    /// No calls into win32k, the window manager and graphics half of the kernel.
    pub const NO_WIN32K_CALLS: u64 = 1 << 28;
    /// No DLLs pushed in from outside: AppInit DLLs, window hooks, IMEs and the like.
    pub const NO_EXTENSION_POINTS: u64 = 1 << 32;
    /// No pages made executable after load, so no code can be written and run.
    pub const NO_DYNAMIC_CODE: u64 = 1 << 36;
    /// No fonts but the system's.
    pub const SYSTEM_FONTS_ONLY: u64 = 1 << 48;
    /// No DLLs from a network share.
    pub const NO_REMOTE_IMAGES: u64 = 1 << 52;
    /// No DLLs from a file labelled low integrity, as anything a sandboxed program wrote would be.
    pub const NO_LOW_LABEL_IMAGES: u64 = 1 << 56;

    pub const ALL: u64 = STRICT_HANDLE_CHECKS
        | NO_WIN32K_CALLS
        | NO_EXTENSION_POINTS
        | NO_DYNAMIC_CODE
        | SYSTEM_FONTS_ONLY
        | NO_REMOTE_IMAGES
        | NO_LOW_LABEL_IMAGES;
}

/// Every one of a job's desktop limits: no clipboard either way, no display settings, no exiting
/// Windows, no global atoms, no handles to other processes' windows, no desktop switching and no
/// system settings. As in Windows' SDK.
const JOB_OBJECT_UILIMIT_ALL: u32 = 0x1FF;
/// The child-process policy that lets a process start none, as in Windows' SDK.
const PROCESS_CREATION_CHILD_PROCESS_RESTRICTED: u32 = 1;
/// The attributes a jailed start sets: its container, its handles, its mitigations and its
/// child-process policy.
const ATTRIBUTES: u32 = 4;
/// The most a jailed program may hold in memory, unless the caller says otherwise. What it does is
/// read lines, so this is generous.
const DEFAULT_MEMORY_LIMIT: usize = 64 << 20;

/// A jail for one kind of program, named by its AppContainer profile.
#[derive(Debug, Clone)]
pub struct Jail {
    profile: String,
    memory_limit: usize,
}

/// The standard handles a jailed program gets. Each is moved in and closed in the parent once the
/// program has its own copy.
#[derive(Debug, Default)]
pub struct Handles {
    pub stdin: Option<OwnedHandle>,
    pub stdout: Option<OwnedHandle>,
    pub stderr: Option<OwnedHandle>,
}

/// A program running in a jail. Dropping it closes the job, which stops the program.
#[derive(Debug)]
pub struct Jailed {
    process: OwnedHandle,
    _job: OwnedHandle,
}

impl Jail {
    /// `profile` names the AppContainer, which Windows keeps for the user between runs. It should
    /// be the same for every start of the same program, and different for different programs.
    pub fn new(profile: &str) -> Self {
        Jail { profile: profile.to_owned(), memory_limit: DEFAULT_MEMORY_LIMIT }
    }

    pub fn memory_limit(mut self, bytes: usize) -> Self {
        self.memory_limit = bytes;
        self
    }

    /// Starts `program` with `arguments` in the jail.
    pub fn spawn(&self, program: &Path, arguments: &[&OsStr], handles: Handles) -> io::Result<Jailed> {
        let container = Container::open(&self.profile)?;
        container.may_run(program)?;
        let job = job(self.memory_limit)?;

        let inherited: Vec<RawHandle> = unique([&handles.stdin, &handles.stdout, &handles.stderr]);
        for &handle in &inherited {
            // SAFETY: the handle is open, as an OwnedHandle the caller moved in.
            check(unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) })?;
        }

        let mut capabilities = SECURITY_CAPABILITIES {
            AppContainerSid: container.sid,
            Capabilities: null_mut(),
            CapabilityCount: 0,
            Reserved: 0,
        };
        let mitigations: u64 = mitigation::ALL;
        let child_policy: u32 = PROCESS_CREATION_CHILD_PROCESS_RESTRICTED;
        let attributes = AttributeList::new(ATTRIBUTES)?;
        attributes.set(
            PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
            &raw mut capabilities,
            size_of::<SECURITY_CAPABILITIES>(),
        )?;
        attributes.set(
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
            inherited.as_ptr(),
            size_of_val(inherited.as_slice()),
        )?;
        attributes.set(PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY, &raw const mitigations, size_of::<u64>())?;
        attributes.set(
            PROC_THREAD_ATTRIBUTE_CHILD_PROCESS_POLICY,
            &raw const child_policy,
            size_of::<u32>(),
        )?;

        // SAFETY: an all-zero STARTUPINFOEXW is valid, and the fields used are set below.
        let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = raw(&handles.stdin);
        startup.StartupInfo.hStdOutput = raw(&handles.stdout);
        startup.StartupInfo.hStdError = raw(&handles.stderr);
        startup.lpAttributeList = attributes.list;

        let application = wide(program.as_os_str());
        let mut command_line = wide(&command_line(program.as_os_str(), arguments));
        // SAFETY: an all-zero PROCESS_INFORMATION is valid for the call to fill.
        let mut started: PROCESS_INFORMATION = unsafe { zeroed() };
        // Suspended, so it's in its job before it runs a single instruction of its own. No console:
        // its standard handles are all it talks through.
        let flags = EXTENDED_STARTUPINFO_PRESENT | DETACHED_PROCESS | CREATE_SUSPENDED;
        // SAFETY: both strings end in a zero and outlive the call, the command line is writable as
        // the call needs, and the startup information and its attribute list are filled above.
        check(unsafe {
            CreateProcessW(
                application.as_ptr(),
                command_line.as_mut_ptr(),
                null(),
                null(),
                1,
                flags,
                null(),
                null(),
                &raw const startup.StartupInfo,
                &mut started,
            )
        })?;
        // SAFETY: CreateProcessW gave both handles to this process, to own.
        let (process, thread) = unsafe {
            (OwnedHandle::from_raw_handle(started.hProcess), OwnedHandle::from_raw_handle(started.hThread))
        };
        // SAFETY: both handles are open.
        if unsafe { AssignProcessToJobObject(job.as_raw_handle(), process.as_raw_handle()) } == 0 {
            let error = io::Error::last_os_error();
            // SAFETY: the process is ours and never ran.
            unsafe { TerminateProcess(process.as_raw_handle(), 1) };
            return Err(error);
        }
        // SAFETY: the thread is open, and suspended once, by CREATE_SUSPENDED.
        if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
            return Err(io::Error::last_os_error());
        }
        drop(handles);
        Ok(Jailed { process, _job: job })
    }
}

impl Jailed {
    /// Waits for the program to end, and gives its exit code.
    pub fn wait(&self) -> io::Result<u32> {
        // SAFETY: the process handle is open.
        if unsafe { WaitForSingleObject(self.process.as_raw_handle(), INFINITE) } != WAIT_OBJECT_0 {
            return Err(io::Error::last_os_error());
        }
        let mut code = 0;
        // SAFETY: as above, and `code` is a place for the exit code.
        check(unsafe { GetExitCodeProcess(self.process.as_raw_handle(), &mut code) })?;
        Ok(code)
    }

    pub fn kill(&self) -> io::Result<()> {
        // SAFETY: the process handle is open.
        check(unsafe { TerminateProcess(self.process.as_raw_handle(), 1) })
    }
}

/// What the jail gave the process that asks, read back from Windows rather than taken on trust.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Applied {
    pub app_container: bool,
    pub no_win32k_calls: bool,
    pub no_dynamic_code: bool,
    pub no_child_processes: bool,
}

impl Applied {
    pub fn whole(&self) -> bool {
        self.app_container && self.no_win32k_calls && self.no_dynamic_code && self.no_child_processes
    }
}

/// What this process got, for a jailed program to check, or to say.
pub fn applied() -> Applied {
    Applied {
        app_container: in_app_container(),
        // Each policy's first flag is the one that matters here: win32k calls refused, dynamic code
        // refused, child processes refused.
        no_win32k_calls: first_flag(ProcessSystemCallDisablePolicy),
        no_dynamic_code: first_flag(ProcessDynamicCodePolicy),
        no_child_processes: first_flag(ProcessChildProcessPolicy),
    }
}

fn first_flag(policy: i32) -> bool {
    let mut flags: u32 = 0;
    // SAFETY: each of these policies is one 32-bit word of flags, and the buffer is that word.
    let read = unsafe {
        GetProcessMitigationPolicy(GetCurrentProcess(), policy, (&raw mut flags).cast(), size_of::<u32>())
    };
    read != 0 && flags & 1 != 0
}

fn in_app_container() -> bool {
    let mut token: HANDLE = null_mut();
    // SAFETY: the pseudo-handle for this process needs no closing, and `token` is a place for one.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return false;
    }
    // SAFETY: OpenProcessToken gave the handle to this process, to own.
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut is: u32 = 0;
    let mut written = 0;
    // SAFETY: the answer is one 32-bit word, and the buffer is that word.
    let read = unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenIsAppContainer,
            (&raw mut is).cast(),
            size_of::<u32>() as u32,
            &mut written,
        )
    };
    read != 0 && is != 0
}

/// An AppContainer's SID, freed when dropped.
struct Container {
    sid: PSID,
}

impl Container {
    /// The container `profile`, made for this user the first time it's asked for.
    fn open(profile: &str) -> io::Result<Self> {
        let name = wide(OsStr::new(profile));
        let mut sid: PSID = null_mut();
        // SAFETY: the strings end in a zero and outlive the call, no capabilities are given, and
        // `sid` is a place for the container's SID.
        let made = unsafe {
            CreateAppContainerProfile(name.as_ptr(), name.as_ptr(), name.as_ptr(), null(), 0, &mut sid)
        };
        if made == hresult_from_win32(ERROR_ALREADY_EXISTS) {
            // SAFETY: as above.
            let derived = unsafe { DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid) };
            check_hresult(derived)?;
        } else {
            check_hresult(made)?;
        }
        Ok(Container { sid })
    }

    /// Lets the container read and run `program`, which it couldn't otherwise open: a user's files
    /// grant nothing to AppContainers, and only system folders do.
    fn may_run(&self, program: &Path) -> io::Result<()> {
        let path = wide(program.as_os_str());
        let mut old: *mut ACL = null_mut();
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: the path ends in a zero, and the two out pointers are places for Windows' answer.
        let read = unsafe {
            GetNamedSecurityInfoW(
                path.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                &mut old,
                null_mut(),
                &mut descriptor,
            )
        };
        check_win32(read)?;
        let grant = EXPLICIT_ACCESS_W {
            grfAccessPermissions: FILE_GENERIC_READ | FILE_GENERIC_EXECUTE,
            grfAccessMode: GRANT_ACCESS,
            grfInheritance: NO_INHERITANCE,
            Trustee: TRUSTEE_W {
                pMultipleTrustee: null_mut(),
                MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_WELL_KNOWN_GROUP,
                ptstrName: self.sid.cast(),
            },
        };
        let mut new: *mut ACL = null_mut();
        // SAFETY: one entry, the old list from the file, and a place for the merged list.
        let merged = unsafe { SetEntriesInAclW(1, &grant, old, &mut new) };
        let result = check_win32(merged).and_then(|()| {
            // SAFETY: the path ends in a zero, and the new list came from SetEntriesInAclW.
            check_win32(unsafe {
                SetNamedSecurityInfoW(
                    path.as_ptr().cast_mut(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    null_mut(),
                    null_mut(),
                    new,
                    null_mut(),
                )
            })
        });
        // SAFETY: both were allocated by Windows with LocalAlloc for the caller to free, and the old
        // list lives inside the descriptor, so it isn't freed on its own.
        unsafe {
            LocalFree(new.cast());
            LocalFree(descriptor);
        }
        result
    }
}

impl Drop for Container {
    fn drop(&mut self) {
        // SAFETY: the SID came from CreateAppContainerProfile or its derivation, both of which say
        // to free it with FreeSid.
        unsafe { FreeSid(self.sid) };
    }
}

/// A job that stops its process when its last handle closes, lets it start nothing, and keeps it
/// from the desktop: no clipboard, no other windows, no display settings.
fn job(memory_limit: usize) -> io::Result<OwnedHandle> {
    // SAFETY: no security attributes and no name.
    let handle = unsafe { CreateJobObjectW(null(), null()) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: CreateJobObjectW gave the handle to this process, to own.
    let job = unsafe { OwnedHandle::from_raw_handle(handle) };
    // SAFETY: an all-zero limit block is valid, and the fields used are set below.
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
        | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION
        | JOB_OBJECT_LIMIT_PROCESS_MEMORY;
    limits.BasicLimitInformation.ActiveProcessLimit = 1;
    limits.ProcessMemoryLimit = memory_limit;
    // SAFETY: the job is open, and the block is the type and size the class names.
    check(unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    })?;
    let desktop = JOBOBJECT_BASIC_UI_RESTRICTIONS { UIRestrictionsClass: JOB_OBJECT_UILIMIT_ALL };
    // SAFETY: as above.
    check(unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectBasicUIRestrictions,
            (&raw const desktop).cast(),
            size_of::<JOBOBJECT_BASIC_UI_RESTRICTIONS>() as u32,
        )
    })?;
    Ok(job)
}

/// A process's attribute list, deleted when dropped. What's set in it must outlive the start.
struct AttributeList {
    list: LPPROC_THREAD_ATTRIBUTE_LIST,
    _buffer: Vec<u64>,
}

impl AttributeList {
    fn new(count: u32) -> io::Result<Self> {
        let mut size = 0;
        // SAFETY: asking only for the size, with no list. It fails, as it should, saying how much.
        unsafe { InitializeProcThreadAttributeList(null_mut(), count, 0, &mut size) };
        let mut buffer = vec![0u64; size.div_ceil(size_of::<u64>())];
        let list = buffer.as_mut_ptr().cast();
        // SAFETY: the buffer is at least `size` bytes, and aligned for the list.
        check(unsafe { InitializeProcThreadAttributeList(list, count, 0, &mut size) })?;
        Ok(AttributeList { list, _buffer: buffer })
    }

    fn set<T>(&self, attribute: u32, value: *const T, size: usize) -> io::Result<()> {
        // SAFETY: the list is initialised, and the value is `size` bytes that the caller keeps
        // alive until the process has started.
        check(unsafe {
            UpdateProcThreadAttribute(
                self.list,
                0,
                attribute as usize,
                value.cast(),
                size,
                null_mut(),
                null(),
            )
        })
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        // SAFETY: the list was initialised, and its buffer is still alive.
        unsafe { DeleteProcThreadAttributeList(self.list) };
    }
}

/// The distinct handles among those given, as a handle may be given twice, as output and error.
fn unique(handles: [&Option<OwnedHandle>; 3]) -> Vec<RawHandle> {
    let mut found: Vec<RawHandle> = Vec::new();
    for handle in handles.into_iter().flatten() {
        let raw = handle.as_raw_handle();
        if !found.contains(&raw) {
            found.push(raw);
        }
    }
    found
}

fn raw(handle: &Option<OwnedHandle>) -> HANDLE {
    handle.as_ref().map_or(null_mut(), |handle| handle.as_raw_handle())
}

fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(std::iter::once(0)).collect()
}

/// A command line as Windows' C runtime splits it back into the same arguments.
fn command_line(program: &OsStr, arguments: &[&OsStr]) -> std::ffi::OsString {
    let mut line = std::ffi::OsString::new();
    for (index, argument) in std::iter::once(program).chain(arguments.iter().copied()).enumerate() {
        if index > 0 {
            line.push(" ");
        }
        line.push(quoted(&argument.to_string_lossy()));
    }
    line
}

fn quoted(argument: &str) -> String {
    if !argument.is_empty() && !argument.contains([' ', '\t', '"']) {
        return argument.to_owned();
    }
    let mut out = String::from("\"");
    let mut backslashes = 0;
    for character in argument.chars() {
        match character {
            '\\' => backslashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.push_str(&"\\".repeat(backslashes));
                out.push(character);
                backslashes = 0;
            }
        }
    }
    out.push_str(&"\\".repeat(backslashes * 2));
    out.push('"');
    out
}

/// Win32's own fold of an error code into an HRESULT.
fn hresult_from_win32(error: u32) -> i32 {
    ((error & 0xFFFF) | (7 << 16) | 0x8000_0000) as i32
}

fn check(succeeded: i32) -> io::Result<()> {
    if succeeded != 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

fn check_win32(error: u32) -> io::Result<()> {
    if error == 0 { Ok(()) } else { Err(io::Error::from_raw_os_error(error as i32)) }
}

fn check_hresult(code: i32) -> io::Result<()> {
    if code >= 0 { Ok(()) } else { Err(io::Error::other(format!("error {:#010x}", code as u32))) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_survive_the_command_line() {
        let line =
            command_line(OsStr::new(r"C:\a b\x.exe"), &[OsStr::new("plain"), OsStr::new(r#"say "hi""#)]);
        assert_eq!(line, r#""C:\a b\x.exe" plain "say \"hi\"""#);
        assert_eq!(quoted(r"C:\path with space\"), r#""C:\path with space\\""#);
        assert_eq!(quoted(""), r#""""#);
    }

    #[test]
    fn every_mitigation_has_its_own_bit() {
        let bits = [
            mitigation::STRICT_HANDLE_CHECKS,
            mitigation::NO_WIN32K_CALLS,
            mitigation::NO_EXTENSION_POINTS,
            mitigation::NO_DYNAMIC_CODE,
            mitigation::SYSTEM_FONTS_ONLY,
            mitigation::NO_REMOTE_IMAGES,
            mitigation::NO_LOW_LABEL_IMAGES,
        ];
        assert_eq!(bits.iter().fold(0, |all, bit| all | bit), mitigation::ALL);
        assert_eq!(bits.iter().map(|bit| bit.count_ones()).sum::<u32>(), bits.len() as u32);
    }
}
