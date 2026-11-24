//! One connection from the host to a service in a guest, over an AF_HYPERV socket. Reading and
//! writing may go on at once from two threads, as a relay needs, and `close` ends both from a third.

use super::vsock_service_id;
use std::io::{self, Read, Write};
use std::mem::{MaybeUninit, size_of};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use windows_sys::Win32::Networking::WinSock::{
    AF_HYPERV, FIONBIO, INVALID_SOCKET, POLLWRNORM, SD_BOTH, SO_ERROR, SOCK_STREAM, SOCKADDR, SOCKET,
    SOCKET_ERROR, SOL_SOCKET, WSAGetLastError, WSAPOLLFD, WSAPoll, WSAStartup, closesocket, connect,
    getsockopt, ioctlsocket, recv, send, shutdown, socket,
};
use windows_sys::Win32::System::Hypervisor::{HV_PROTOCOL_RAW, SOCKADDR_HV};
use windows_sys::core::GUID;

/// Winsock 2.2.
const WINSOCK_VERSION: u16 = 0x0202;
/// The most one `send` or `recv` is given, which Winsock takes as an `i32`.
const CHUNK: usize = 64 * 1024;

/// A connected Hyper-V socket. Clones share the one socket, which closes when the last is dropped.
#[derive(Clone)]
pub struct HvStream {
    socket: Arc<Owned>,
}

struct Owned(SOCKET);

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the socket is ours and nothing uses it once the last clone is gone.
        unsafe { closesocket(self.0) };
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

impl HvStream {
    /// Connects to vsock `port` in the machine whose runtime ID is `vm_id`, giving up after
    /// `timeout`. A machine with nothing listening there, or one that isn't up, is an error.
    pub fn connect(vm_id: u128, port: u32, timeout: Duration) -> io::Result<HvStream> {
        start_winsock()?;
        // SAFETY: plain call; the answer is checked.
        let raw = unsafe { socket(i32::from(AF_HYPERV), SOCK_STREAM, HV_PROTOCOL_RAW as i32) };
        if raw == INVALID_SOCKET {
            return Err(last_error());
        }
        let owned = Arc::new(Owned(raw));
        let address = SOCKADDR_HV {
            Family: AF_HYPERV,
            Reserved: 0,
            VmId: GUID::from_u128(vm_id),
            ServiceId: GUID::from_u128(vsock_service_id(port)),
        };
        set_blocking(raw, false)?;
        // SAFETY: `address` is a SOCKADDR_HV of the size given, and outlives the call.
        let started =
            unsafe { connect(raw, (&raw const address).cast::<SOCKADDR>(), size_of::<SOCKADDR_HV>() as i32) };
        if started == SOCKET_ERROR {
            // SAFETY: reads the calling thread's last Winsock error.
            let code = unsafe { WSAGetLastError() };
            if code != windows_sys::Win32::Networking::WinSock::WSAEWOULDBLOCK {
                return Err(io::Error::from_raw_os_error(code));
            }
            wait_connected(raw, timeout)?;
        }
        set_blocking(raw, true)?;
        Ok(HvStream { socket: owned })
    }

    /// Ends the connection in both directions, so a read or write waiting on it returns.
    pub fn close(&self) {
        // SAFETY: the socket is open while a clone is.
        unsafe { shutdown(self.socket.0, SD_BOTH) };
    }

    fn read_some(&self, buffer: &mut [u8]) -> io::Result<usize> {
        let length = buffer.len().min(CHUNK);
        // SAFETY: `buffer` holds at least `length` bytes.
        let read = unsafe { recv(self.socket.0, buffer.as_mut_ptr(), length as i32, 0) };
        if read == SOCKET_ERROR { Err(last_error()) } else { Ok(read as usize) }
    }

    fn write_some(&self, bytes: &[u8]) -> io::Result<usize> {
        let length = bytes.len().min(CHUNK);
        // SAFETY: `bytes` holds at least `length` bytes.
        let written = unsafe { send(self.socket.0, bytes.as_ptr(), length as i32, 0) };
        if written == SOCKET_ERROR { Err(last_error()) } else { Ok(written as usize) }
    }
}

fn set_blocking(socket: SOCKET, blocking: bool) -> io::Result<()> {
    let mut nonblocking = u32::from(!blocking);
    // SAFETY: `nonblocking` is the flag the call reads.
    if unsafe { ioctlsocket(socket, FIONBIO, &mut nonblocking) } == SOCKET_ERROR {
        return Err(last_error());
    }
    Ok(())
}

/// Waits for a connect begun on a non-blocking socket to finish, and says how it ended.
fn wait_connected(socket: SOCKET, timeout: Duration) -> io::Result<()> {
    let mut poll = WSAPOLLFD { fd: socket, events: POLLWRNORM, revents: 0 };
    let milliseconds = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
    // SAFETY: one entry, as many as is said.
    let ready = unsafe { WSAPoll(&mut poll, 1, milliseconds) };
    match ready {
        SOCKET_ERROR => return Err(last_error()),
        0 => return Err(io::ErrorKind::TimedOut.into()),
        _ => {}
    }
    // The socket's own answer says whether it connected, as a failed connect also wakes the poll.
    let mut error = 0i32;
    let mut length = size_of::<i32>() as i32;
    // SAFETY: `error` and `length` are the places the call fills, of the size it's told.
    let asked =
        unsafe { getsockopt(socket, SOL_SOCKET, SO_ERROR, (&raw mut error).cast::<u8>(), &mut length) };
    if asked == SOCKET_ERROR {
        return Err(last_error());
    }
    if error != 0 {
        return Err(io::Error::from_raw_os_error(error));
    }
    Ok(())
}

impl Read for HvStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.read_some(buffer)
    }
}

impl Write for HvStream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.write_some(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
