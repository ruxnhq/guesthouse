//! SSH to an Ubuntu guest with no network: while the machine runs, the host listens on a port of
//! its own loopback and relays each connection to the guest's sshd over a Hyper-V socket, which the
//! guest's `guesthouse-ssh.socket` takes on vsock port 22 and hands to sshd.
//!
//! The relay knows nothing of Windows. The guest is behind [`Dial`], so the tests use a TCP server
//! as the guest, and `windows::ssh` is the real one. Only the loopback address is ever listened on,
//! so another PC can't reach it, but any user of this PC can, and sshd still asks them for the
//! guest's own credentials.

use crate::events::Sink;
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// The vsock port the guest's sshd is reached on: sshd's own, as the guest's socket unit says.
pub const GUEST_PORT: u32 = 22;
/// The first port a machine is given on the host's loopback, and the last.
pub const PORTS: (u16, u16) = (2201, 2999);
/// The most connections one machine's relay holds open at once, so a local program that opens
/// them without end can't take every thread of the service.
pub const MOST_CONNECTIONS: usize = 32;
/// How often the listener looks for a connection while it also looks for being stopped.
const LOOK: Duration = Duration::from_millis(50);

/// One connection to the guest.
pub trait GuestSocket: Read + Write + Send + 'static {
    /// Another handle on the same connection, to read on one thread while another writes.
    fn try_clone(&self) -> io::Result<Box<dyn GuestSocket>>;
    /// Ends the connection both ways, so a read or write waiting on it returns.
    fn close(&self);
}

/// How the relay reaches the guest: a new connection for each one it relays.
pub trait Dial: Send + Sync + 'static {
    fn dial(&self) -> io::Result<Box<dyn GuestSocket>>;
}

/// Whether `port` can be listened on now, on the loopback.
pub fn port_free(port: u16) -> bool {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok()
}

/// The ports to try for a machine, lowest first.
pub fn ports() -> impl Iterator<Item = u16> {
    PORTS.0..=PORTS.1
}

/// A relay from `127.0.0.1:port` to a guest, running until it's dropped.
pub struct Listener {
    port: u16,
    stop: Arc<AtomicBool>,
    open: Arc<Open>,
    thread: Option<JoinHandle<()>>,
}

/// The connections being relayed, each with what closes it.
#[derive(Default)]
struct Open {
    closers: Mutex<HashMap<u64, Box<dyn Fn() + Send>>>,
    next: AtomicU64,
}

impl Open {
    fn closers(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Box<dyn Fn() + Send>>> {
        self.closers.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn close_all(&self) {
        for close in self.closers().values() {
            close();
        }
    }
}

impl Listener {
    /// Listens on `127.0.0.1:port`, and relays each connection to what `dial` reaches. A port that's
    /// taken is an error, for the caller to say.
    pub fn start(port: u16, dial: Arc<dyn Dial>, sink: Arc<dyn Sink>) -> io::Result<Listener> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let open = Arc::new(Open::default());
        let thread = {
            let (stop, open) = (Arc::clone(&stop), Arc::clone(&open));
            thread::spawn(move || accept_loop(&listener, &*dial, &sink, &stop, &open))
        };
        Ok(Listener { port, stop, open, thread: Some(thread) })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Stops listening and closes every connection. The port is free when this returns.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.open.close_all();
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop();
    }
}

fn accept_loop(
    listener: &TcpListener,
    dial: &dyn Dial,
    sink: &Arc<dyn Sink>,
    stop: &AtomicBool,
    open: &Arc<Open>,
) {
    // Said once while the guest can't be reached, and again after it has been.
    let failing = Arc::new(AtomicBool::new(false));
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((client, from)) => {
                // Only this PC's own loopback is ever listened on, so this only fails on a mistake.
                if !from.ip().is_loopback() || open.closers().len() >= MOST_CONNECTIONS {
                    continue;
                }
                relay_one(client, dial, sink, open, &failing);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(LOOK),
            Err(error) => {
                sink.problem(format!("SSH on this PC's loopback stopped listening: {error}"));
                return;
            }
        }
    }
}

/// Dials the guest for `client` and relays between them on threads of their own. A guest that can't
/// be reached closes `client` at once, as a port with nothing behind it would.
fn relay_one(
    client: TcpStream,
    dial: &dyn Dial,
    sink: &Arc<dyn Sink>,
    open: &Arc<Open>,
    failing: &AtomicBool,
) {
    let _ = client.set_nonblocking(false);
    let _ = client.set_nodelay(true);
    let guest = match dial.dial() {
        Ok(guest) => {
            failing.store(false, Ordering::SeqCst);
            guest
        }
        Err(error) => {
            if !failing.swap(true, Ordering::SeqCst) {
                sink.note(format!(
                    "A connection to its SSH port found the guest not listening yet, or at all: {error}. \
                     This is said once until it's reached."
                ));
            }
            return;
        }
    };
    let (Ok(client_reader), Ok(guest_reader)) = (client.try_clone(), guest.try_clone()) else { return };
    // Either end closing closes both, which lets the other direction's read return.
    let (Some(close_up), Some(close_down), Some(close_all)) =
        (closer(&client, &*guest), closer(&client, &*guest), closer(&client, &*guest))
    else {
        return;
    };
    let id = open.next.fetch_add(1, Ordering::SeqCst);
    open.closers().insert(id, close_all);
    let open = Arc::clone(open);
    thread::spawn(move || {
        let up = thread::spawn(move || {
            copy(client_reader, guest);
            close_up();
        });
        copy(guest_reader, client);
        close_down();
        let _ = up.join();
        open.closers().remove(&id);
    });
}

/// What closes a connection, both ends of it.
fn closer(client: &TcpStream, guest: &dyn GuestSocket) -> Option<Box<dyn Fn() + Send>> {
    let (client, guest) = (client.try_clone().ok()?, guest.try_clone().ok()?);
    Some(Box::new(move || {
        let _ = client.shutdown(Shutdown::Both);
        guest.close();
    }))
}

/// Copies from `from` to `to` until either ends.
fn copy(mut from: impl Read, mut to: impl Write) {
    let mut buffer = [0u8; 16 * 1024];
    loop {
        match from.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(read) => {
                if to.write_all(&buffer[..read]).is_err() {
                    return;
                }
            }
        }
    }
}

/// The address a relay listens on, for a port.
pub fn address(port: u16) -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, port))
}

#[cfg(test)]
mod tests;
