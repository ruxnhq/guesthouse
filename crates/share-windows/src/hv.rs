//! The Hyper-V socket to the host: `AF_HYPERV`, `HV_PROTOCOL_RAW`, to the parent partition at the
//! service ID a vsock port makes. It is `Read + Write`, so the 9P client runs over it.

use crate::logic::{HV_GUID_PARENT, vsock_service_guid};
use std::io::{self, Read, Write};
use std::mem::{MaybeUninit, size_of};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use windows_sys::Win32::Networking::WinSock::{
    AF_HYPERV, INVALID_SOCKET, SD_BOTH, SOCK_STREAM, SOCKADDR, SOCKET, SOCKET_ERROR, WSAGetLastError,
    WSAStartup, closesocket, connect, recv, send, shutdown, socket,
};
use windows_sys::Win32::System::Hypervisor::{HV_PROTOCOL_RAW, SOCKADDR_HV};
use windows_sys::core::GUID;

/// Winsock 2.2.
const WINSOCK_VERSION: u16 = 0x0202;
/// The most one `send` or `recv` is given, which Winsock takes as an `i32`.
const CHUNK: usize = 64 * 1024;
/// How long a connect is retried: the guest may start before the host's share is ready.
pub const CONNECT_PATIENCE: Duration = Duration::from_secs(60);

/// A connected Hyper-V socket to the host. It closes when dropped.
pub struct HvStream {
    socket: SOCKET,
}

impl Drop for HvStream {
    fn drop(&mut self) {
        // SAFETY: the socket is ours and nothing uses it after this.
        unsafe {
            shutdown(self.socket, SD_BOTH);
            closesocket(self.socket);
        }
    }
}

fn last_error() -> io::Error {
    // SAFETY: reads the calling thread's last Winsock error.
    io::Error::from_raw_os_error(unsafe { WSAGetLastError() })
}

fn start_winsock() -> io::Result<()> {
    static STARTED: OnceLock<i32> = OnceLock::new();
    let failed = *STARTED.get_or_init(|| {
        let mut data = MaybeUninit::zeroed();
        // SAFETY: `data` is a place for Winsock to describe itself in.
        unsafe { WSAStartup(WINSOCK_VERSION, data.as_mut_ptr()) }
    });
    if failed == 0 { Ok(()) } else { Err(io::Error::from_raw_os_error(failed)) }
}

fn guid((data1, data2, data3, data4): (u32, u16, u16, [u8; 8])) -> GUID {
    GUID { data1, data2, data3, data4 }
}

impl HvStream {
    /// Connects once to vsock `port` on the host.
    pub fn connect(port: u32) -> io::Result<HvStream> {
        start_winsock()?;
        // SAFETY: plain call; the answer is checked.
        let raw = unsafe { socket(i32::from(AF_HYPERV), SOCK_STREAM, HV_PROTOCOL_RAW as i32) };
        if raw == INVALID_SOCKET {
            return Err(last_error());
        }
        let stream = HvStream { socket: raw };
        let address = SOCKADDR_HV {
            Family: AF_HYPERV,
            Reserved: 0,
            VmId: guid(HV_GUID_PARENT),
            ServiceId: guid(vsock_service_guid(port)),
        };
        // SAFETY: `address` is a SOCKADDR_HV of the size given, and outlives the call.
        let result =
            unsafe { connect(raw, (&raw const address).cast::<SOCKADDR>(), size_of::<SOCKADDR_HV>() as i32) };
        if result == SOCKET_ERROR { Err(last_error()) } else { Ok(stream) }
    }

    /// Connects, trying again every second for up to `patience`, and gives the last error if it
    /// never works. `note` hears each failure.
    pub fn connect_patiently(
        port: u32,
        patience: Duration,
        mut note: impl FnMut(&io::Error),
    ) -> io::Result<HvStream> {
        let start = Instant::now();
        loop {
            match HvStream::connect(port) {
                Ok(stream) => return Ok(stream),
                Err(error) => {
                    note(&error);
                    if start.elapsed() >= patience {
                        return Err(error);
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        }
    }
}

impl Read for HvStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let length = buffer.len().min(CHUNK);
        // SAFETY: `buffer` holds at least `length` bytes.
        let read = unsafe { recv(self.socket, buffer.as_mut_ptr(), length as i32, 0) };
        if read == SOCKET_ERROR { Err(last_error()) } else { Ok(read as usize) }
    }
}

impl Write for HvStream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let length = bytes.len().min(CHUNK);
        // SAFETY: `bytes` holds at least `length` bytes.
        let written = unsafe { send(self.socket, bytes.as_ptr(), length as i32, 0) };
        if written == SOCKET_ERROR { Err(last_error()) } else { Ok(written as usize) }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
