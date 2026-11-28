//! A machine's serial port, as HCS serves it on a named pipe: one connection, read and written at
//! once. A pipe opened plainly is synchronous, and Windows lets one operation at a time through a
//! synchronous handle, so a write would wait behind a read until the guest next said something.
//! Opened overlapped, a read and a write can each be in flight.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_MORE_DATA, ERROR_PIPE_NOT_CONNECTED, GetLastError,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_OVERLAPPED, ReadFile, WriteFile};
use windows_sys::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::CreateEventW;

/// The serial port's pipe, opened for reading and writing at once.
pub struct SerialPort {
    pipe: File,
}

impl SerialPort {
    /// Connects to the port's pipe.
    #[allow(clippy::disallowed_methods)] // a named pipe, not a file on a disk
    pub fn open(pipe: &str) -> io::Result<SerialPort> {
        let pipe = OpenOptions::new().read(true).write(true).custom_flags(FILE_FLAG_OVERLAPPED).open(pipe)?;
        Ok(SerialPort { pipe })
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
