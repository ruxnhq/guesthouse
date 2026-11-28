//! A machine's serial port, as HCS serves it on a named pipe: one connection, read and written at
//! once. A pipe opened plainly is synchronous, and Windows lets one operation at a time through a
//! synchronous handle, so a write would wait behind a read until the guest next said something.
//! Opened overlapped, a read and a write can each be in flight.
//!
//! The pipe is only taken once the process Windows names as its server is known to be the one it
//! should be: the machine's worker, by the account it runs as. Windows names the process that made
//! the pipe's first instance, whichever instance answered. It's opened for identification only, so
//! whoever serves it may learn who opened it but never act as them.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_MORE_DATA, ERROR_PIPE_NOT_CONNECTED,
    GetLastError, HANDLE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows_sys::Win32::Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom};
use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_OVERLAPPED, ReadFile, SECURITY_IDENTIFICATION, WriteFile,
};
use windows_sys::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
use windows_sys::Win32::System::Threading::{
    CreateEventW, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};

/// How many random bytes name each boot's pipes.
const TAG_BYTES: usize = 16;

/// Who must be serving a pipe for it to be taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Server {
    /// Any process running as this account, by its SID's text: a machine's worker, whose account is
    /// `crate::worker::account`.
    Account(String),
    /// This one process, by its ID.
    Process(u32),
}

impl fmt::Display for Server {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Server::Account(sid) => write!(f, "a process running as {sid}"),
            Server::Process(id) => write!(f, "process {id}"),
        }
    }
}

/// Why a serial port wasn't opened.
#[derive(Debug)]
pub enum OpenError {
    /// The pipe couldn't be opened, as when it isn't there yet.
    Io(io::Error),
    /// The pipe was there, but served by a process other than the one asked for, so it was let go
    /// with nothing read or written.
    OtherServer {
        expected: Server,
        process: u32,
        /// The account it runs as, or why that couldn't be read. Only read when an account is
        /// expected.
        account: Option<Result<String, String>>,
    },
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpenError::Io(error) => error.fmt(f),
            OpenError::OtherServer { expected, process, account } => {
                write!(f, "it's served by process {process}")?;
                match account {
                    Some(Ok(account)) => write!(f, ", running as {account}")?,
                    Some(Err(why)) => write!(f, ", whose account couldn't be read ({why})")?,
                    None => {}
                }
                write!(f, ", not {expected}, so it was let go")
            }
        }
    }
}

impl std::error::Error for OpenError {}

impl From<io::Error> for OpenError {
    fn from(error: io::Error) -> Self {
        OpenError::Io(error)
    }
}

/// A new random word for one boot's pipe names: 16 bytes from the system's random number
/// generator, as hex.
pub fn pipe_tag() -> io::Result<String> {
    let mut bytes = [0u8; TAG_BYTES];
    // SAFETY: the buffer has room for the bytes asked for; no algorithm handle with this flag.
    let status = unsafe {
        BCryptGenRandom(null_mut(), bytes.as_mut_ptr(), TAG_BYTES as u32, BCRYPT_USE_SYSTEM_PREFERRED_RNG)
    };
    if status != 0 {
        return Err(io::Error::other(format!("Windows gave no random bytes (status {status:#x})")));
    }
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// The serial port's pipe, opened for reading and writing at once.
pub struct SerialPort {
    pipe: File,
}

impl SerialPort {
    /// Connects to the port's pipe, if `server` serves it. Whoever serves it may learn who opened
    /// it, and nothing more.
    pub fn open(pipe: &str, server: &Server) -> Result<SerialPort, OpenError> {
        let pipe = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(FILE_FLAG_OVERLAPPED)
            .security_qos_flags(SECURITY_IDENTIFICATION)
            .open(pipe)?;
        let process = server_process(&pipe)?;
        let served = match server {
            Server::Process(id) => (process == *id, None),
            Server::Account(sid) => {
                let account = process_account(process).map_err(|error| error.to_string());
                (account.as_deref() == Ok(sid.as_str()), Some(account))
            }
        };
        match served {
            (true, _) => Ok(SerialPort { pipe }),
            (false, account) => Err(OpenError::OtherServer { expected: server.clone(), process, account }),
        }
    }

    /// Reads what the guest has written, waiting for it: 0 once the guest has closed the port.
    pub fn read(&self, buffer: &mut [u8]) -> io::Result<usize> {
        self.operation(buffer.as_mut_ptr(), buffer.len(), true)
    }

    /// Writes all of `bytes` to the guest, while a read may be waiting.
    pub fn write_all(&self, mut bytes: &[u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            let written = self.operation(bytes.as_ptr().cast_mut(), bytes.len(), false)?;
            if written == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            bytes = &bytes[written..];
        }
        Ok(())
    }

    /// One overlapped read or write, waited for: how many bytes went. A pipe whose other end has
    /// gone reads as 0, the end.
    fn operation(&self, buffer: *mut u8, length: usize, read: bool) -> io::Result<usize> {
        let length = u32::try_from(length).unwrap_or(u32::MAX);
        // SAFETY: a new manual-reset event of ours, closed below.
        let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if event.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a zeroed OVERLAPPED is a valid one to start an operation with.
        let mut operation: OVERLAPPED = unsafe { std::mem::zeroed() };
        operation.hEvent = event;
        let handle = self.pipe.as_raw_handle();
        let mut moved = 0u32;
        // SAFETY: the buffer, which the caller lends for `length` bytes, and the OVERLAPPED both
        // outlive the operation, which is waited for below before this returns.
        let result = unsafe {
            let started = if read {
                ReadFile(handle, buffer, length, std::ptr::null_mut(), &mut operation)
            } else {
                WriteFile(handle, buffer, length, std::ptr::null_mut(), &mut operation)
            };
            let error = if started == 0 { GetLastError() } else { 0 };
            if started == 0 && error != ERROR_IO_PENDING && error != ERROR_MORE_DATA {
                Err(error)
            } else if GetOverlappedResult(handle, &operation, &mut moved, 1) == 0 {
                match GetLastError() {
                    ERROR_MORE_DATA => Ok(()),
                    other => Err(other),
                }
            } else {
                Ok(())
            }
        };
        // SAFETY: the event is ours and no operation waits on it any more.
        unsafe { CloseHandle(event) };
        match result {
            Ok(()) => Ok(moved as usize),
            Err(ERROR_BROKEN_PIPE | ERROR_PIPE_NOT_CONNECTED) if read => Ok(0),
            Err(error) => Err(io::Error::from_raw_os_error(error as i32)),
        }
    }
}

/// The ID of the process serving the pipe `pipe` is open on.
fn server_process(pipe: &File) -> io::Result<u32> {
    let mut process = 0u32;
    // SAFETY: the handle is open, and the ID comes back in a place for it.
    if unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle(), &mut process) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(process)
}

/// The SID's text of the account process `process` runs as, if it can be read.
fn process_account(process: u32) -> io::Result<String> {
    // SAFETY: plain call; the handle is closed below.
    let opened = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process) };
    if opened.is_null() {
        return Err(io::Error::last_os_error());
    }
    let mut token: HANDLE = null_mut();
    // SAFETY: the process's handle is open, and the token comes back in a place for it.
    let got = unsafe { OpenProcessToken(opened, TOKEN_QUERY, &mut token) } != 0;
    let error = io::Error::last_os_error();
    // SAFETY: ours, and not used again.
    unsafe { CloseHandle(opened) };
    if !got {
        return Err(error);
    }
    let account = token_user(token);
    // SAFETY: ours, and not used again.
    unsafe { CloseHandle(token) };
    account.ok_or_else(|| io::Error::other("its token names no user"))
}

/// The SID's text of the user `token` is for.
fn token_user(token: HANDLE) -> Option<String> {
    let mut needed = 0u32;
    // SAFETY: asking only for the size, with no buffer.
    unsafe { GetTokenInformation(token, TokenUser, null_mut(), 0, &mut needed) };
    if needed == 0 {
        return None;
    }
    // u64s, so the TOKEN_USER at its start is aligned.
    let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
    // SAFETY: the buffer has room for `needed` bytes.
    if unsafe { GetTokenInformation(token, TokenUser, buffer.as_mut_ptr().cast(), needed, &mut needed) } == 0
    {
        return None;
    }
    // SAFETY: Windows wrote a TOKEN_USER at the buffer's start, whose SID is in the buffer too.
    let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let mut text: *mut u16 = null_mut();
    // SAFETY: the SID is valid; Windows allocates the text, which is freed below.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return None;
    }
    // SAFETY: Windows wrote a string ending in a zero there.
    let length = (0..).take_while(|&at| unsafe { *text.add(at) } != 0).count();
    // SAFETY: `length` u16s are there, as just counted.
    let words = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
    // SAFETY: Windows allocated it with LocalAlloc, and it's not used again.
    unsafe { LocalFree(text.cast()) };
    Some(words)
}

#[cfg(test)]
mod tests;
