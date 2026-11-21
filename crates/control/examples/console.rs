//! Opens a machine's console, the display HCS serves as Remote Desktop on a pipe, in Windows' own
//! Remote Desktop client, which can't open a pipe: this listens on a port of this PC's loopback only,
//! carries each connection to the pipe and back, and starts the client on that port.
//!
//!     cargo run -p guesthouse-control --example console -- NAME [VM-ID]
//!
//! With VM-ID, the relay sends Remote Desktop's pre-connection message naming it ahead of what the
//! client sends, as Hyper-V's own viewer does, as the console waits for it.
//! For trying the console before the app has a button for it.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::process::{Command, ExitCode};
use std::thread;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(name) = args.first() else {
        eprintln!("Give the machine's name, and if you like HCS's RuntimeId for it.");
        return ExitCode::FAILURE;
    };
    let pipe = format!(r"\\.\pipe\guesthouse-{name}-console");
    let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, 0)) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("A port to listen on: {error}");
            return ExitCode::FAILURE;
        }
    };
    let port = listener.local_addr().map(|address| address.port()).unwrap_or_default();
    println!("{pipe} is at 127.0.0.1:{port}");
    // The greeting is sent by the relay itself, ahead of what the client sends, so the client is
    // started plainly, with no .rdp file and none of the warnings one brings.
    let greeting = args.get(1).cloned();
    let started = Command::new("mstsc.exe").arg(format!("/v:127.0.0.1:{port}")).spawn();
    if let Err(error) = started {
        eprintln!("Remote Desktop: {error}");
    }
    for connection in listener.incoming() {
        let Ok(connection) = connection else { continue };
        let pipe = pipe.clone();
        let greeting = greeting.clone();
        thread::spawn(move || {
            if let Err(error) = carry(connection, &pipe, greeting.as_deref()) {
                eprintln!("{pipe}: {error}");
            }
        });
    }
    ExitCode::SUCCESS
}

/// Carries one connection to the pipe and back, until either end closes, and says each exchange as
/// it goes, which tells how far Remote Desktop got.
///
/// The pipe is opened for overlapped reads and writes: opened for plain, waiting ones, Windows
/// runs only one at a time on it, so a read waiting for the console would hold up the client's
/// next write, and both ends would wait for ever.
fn carry(connection: TcpStream, pipe: &str, greeting: Option<&str>) -> io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;
    let opened = OpenOptions::new().read(true).write(true).custom_flags(FILE_FLAG_OVERLAPPED).open(pipe)?;
    println!("connected to {pipe}");
    if let Some(id) = greeting {
        let mut message = preconnection(id);
        overlapped(&opened, &mut message, false)?;
        println!("sent the greeting for {id}, {} bytes", message.len());
    }
    let opened = std::sync::Arc::new(opened);
    let from_pipe = std::sync::Arc::clone(&opened);
    let (mut from_client, mut to_client) = (connection.try_clone()?, connection);
    let back = thread::spawn(move || -> io::Result<u64> {
        let mut buffer = vec![0u8; 1 << 16];
        let mut total = 0u64;
        loop {
            let read = overlapped(&from_pipe, &mut buffer, true)?;
            if read == 0 {
                return Ok(total);
            }
            to_client.write_all(&buffer[..read])?;
            total += read as u64;
            println!("console -> client {read} bytes, {total} in all");
        }
    });
    let sent = (|| -> io::Result<u64> {
        let mut buffer = vec![0u8; 1 << 16];
        let mut total = 0u64;
        loop {
            let read = from_client.read(&mut buffer)?;
            if read == 0 {
                return Ok(total);
            }
            let mut done = 0;
            while done < read {
                done += overlapped(&opened, &mut buffer[done..read], false)?;
            }
            total += read as u64;
            println!("client -> console {read} bytes, {total} in all");
        }
    })();
    let received = back.join().unwrap_or(Ok(0));
    println!("closed: to the console {sent:?}, back {received:?}");
    sent.map(|_| ())
}

/// One overlapped read (`read`) or write on the pipe, waited for: how many bytes went. A read of a
/// pipe whose other end has gone is 0, the end.
fn overlapped(pipe: &File, buffer: &mut [u8], read: bool) -> io::Result<usize> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_MORE_DATA, GetLastError,
    };
    use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile};
    use windows_sys::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
    use windows_sys::Win32::System::Threading::CreateEventW;
    // SAFETY: a new event of ours, closed below.
    let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    if event.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a zeroed OVERLAPPED is a valid one to start an operation with.
    let mut operation: OVERLAPPED = unsafe { std::mem::zeroed() };
    operation.hEvent = event;
    let handle = pipe.as_raw_handle();
    let length = buffer.len() as u32;
    let mut moved = 0u32;
    // SAFETY: the buffer and the OVERLAPPED outlive the operation, which is waited for below
    // before either goes.
    let started = unsafe {
        if read {
            ReadFile(handle, buffer.as_mut_ptr(), length, std::ptr::null_mut(), &mut operation)
        } else {
            WriteFile(handle, buffer.as_ptr(), length, std::ptr::null_mut(), &mut operation)
        }
    };
    // SAFETY: plain calls on our handle and OVERLAPPED; waiting until the operation is done.
    let result = unsafe {
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
    // SAFETY: ours, and the operation is over.
    unsafe { CloseHandle(event) };
    match result {
        Ok(()) => Ok(moved as usize),
        Err(ERROR_BROKEN_PIPE) if read => Ok(0),
        Err(code) => Err(io::Error::from_raw_os_error(code as i32)),
    }
}

/// Remote Desktop's pre-connection message, version 2 ([MS-RDPEPS] 2.2.1.2), which names the
/// machine the connection is for: its size, no flags, version 2, no ID, then the name's length in
/// UTF-16 units with its zero, and the name.
fn preconnection(name: &str) -> Vec<u8> {
    let text: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let size = 18 + 2 * text.len() as u32;
    let mut message = Vec::with_capacity(size as usize);
    for value in [size, 0, 2, 0] {
        message.extend_from_slice(&value.to_le_bytes());
    }
    message.extend_from_slice(&(text.len() as u16).to_le_bytes());
    for unit in text {
        message.extend_from_slice(&unit.to_le_bytes());
    }
    message
}
