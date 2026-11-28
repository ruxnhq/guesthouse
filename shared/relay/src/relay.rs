//! The relay: one socket on the network, and a table of the devices using it.

use crate::clients::{self, Table};
use crate::sockets;
use crate::stats::{ClientInfo, Counters, Stats};
use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// A UDP datagram never holds more than this.
const LARGEST_DATAGRAM: usize = 65_535;
/// How long a thread waits in the kernel before it looks at whether it should go on. It only bounds how
/// long stopping and expiring take, and costs nothing while datagrams are arriving.
pub(crate) const LOOK_AGAIN: Duration = Duration::from_millis(100);
/// Plenty for a few devices, and few enough that a stranger on the network cannot fill the PC with
/// sockets and threads.
const DEFAULT_MAX_CLIENTS: usize = 16;
/// Remoteman's transport sends a keep-alive every second and gives up on a connection after ten
/// seconds of silence, so a device that has sent nothing for much longer than that is gone.
const DEFAULT_IDLE: Duration = Duration::from_secs(60);
/// Each device has an address of its own on a network, and one behind a router may share one with a
/// few more. One address takes no more places than this, so it can't take every place.
const DEFAULT_MAX_PER_ADDRESS: usize = 4;
/// The guest answers a device's first datagrams at once. A place nothing has answered for this long
/// is given up, and the device gets a new one with its next datagram.
const DEFAULT_UNANSWERED: Duration = Duration::from_secs(5);
const THREAD_STACK: usize = 128 * 1024;

#[derive(Debug, Clone)]
pub struct Config {
    /// The address and port that other devices send to. Port 0 takes any free one.
    pub listen: SocketAddr,
    /// The guest's port the datagrams go to: a loopback port, or an address of the guest's own.
    pub upstream: SocketAddr,
    /// A device that has sent nothing for this long is dropped, and gets a new socket if it comes back.
    pub idle_timeout: Duration,
    /// The most devices at once. A datagram from one more is dropped.
    pub max_clients: usize,
    /// The most devices at once from one address, each on a port of its own.
    pub max_per_address: usize,
    /// A device the guest has sent nothing back to for this long since its place was made loses it.
    pub unanswered_timeout: Duration,
    /// Whether a device's socket takes answers only from `upstream` itself, and drops the rest. On by
    /// default for an upstream off loopback, whose sockets are on every network. Off for loopback, as
    /// QEMU's forward can answer from another port, and only this PC can reach those sockets.
    pub only_upstream_answers: bool,
}

impl Config {
    pub fn new(listen: SocketAddr, upstream: SocketAddr) -> Self {
        Self {
            listen,
            upstream,
            idle_timeout: DEFAULT_IDLE,
            max_clients: DEFAULT_MAX_CLIENTS,
            max_per_address: DEFAULT_MAX_PER_ADDRESS,
            unanswered_timeout: DEFAULT_UNANSWERED,
            only_upstream_answers: !upstream.ip().is_loopback(),
        }
    }
}

/// What the threads of one relay share.
pub(crate) struct Shared {
    pub(crate) config: Config,
    pub(crate) lan: UdpSocket,
    pub(crate) clients: Table,
    pub(crate) stats: Stats,
    pub(crate) stop: AtomicBool,
    pub(crate) started: Instant,
}

/// A running relay. Dropping it stops it and gives the port back.
pub struct Relay {
    shared: Arc<Shared>,
    local: SocketAddr,
    uplink: Option<JoinHandle<()>>,
}

impl Relay {
    pub fn start(config: Config) -> io::Result<Self> {
        let lan = sockets::bound(config.listen)?;
        lan.set_read_timeout(Some(LOOK_AGAIN))?;
        let local = lan.local_addr()?;
        let shared = Arc::new(Shared {
            config,
            lan,
            clients: Table::default(),
            stats: Stats::default(),
            stop: AtomicBool::new(false),
            started: Instant::now(),
        });
        let uplink = {
            let shared = Arc::clone(&shared);
            thread::Builder::new()
                .name("relay-in".into())
                .stack_size(THREAD_STACK)
                .spawn(move || carry_in(&shared))?
        };
        Ok(Self { shared, local, uplink: Some(uplink) })
    }

    /// The address devices send to, with the port filled in when 0 was asked for.
    pub fn local_addr(&self) -> SocketAddr {
        self.local
    }

    pub fn counters(&self) -> Counters {
        self.shared.stats.read()
    }

    /// The devices being carried now, oldest first.
    pub fn clients(&self) -> Vec<ClientInfo> {
        self.shared.clients.info(self.shared.started.elapsed())
    }

    /// Stops the relay and waits until every thread of it has gone, so the port is free again.
    pub fn stop(mut self) {
        self.stop_now();
    }

    fn stop_now(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(uplink) = self.uplink.take() {
            let _ = uplink.join();
        }
        self.shared.clients.join_all();
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.stop_now();
    }
}

/// The thread that takes datagrams from the network and hands each to its device's socket.
fn carry_in(shared: &Arc<Shared>) {
    let mut buffer = vec![0u8; LARGEST_DATAGRAM];
    while !shared.stop.load(Ordering::Acquire) {
        let (length, from) = match shared.lan.recv_from(&mut buffer) {
            Ok(got) => got,
            Err(error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {
                continue;
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            // Windows says here, as `ConnectionReset`, that an answer to a device that has gone was
            // refused. The socket still works for every other device.
            Err(_) => {
                shared.stats.error();
                continue;
            }
        };
        let Some(client) = clients::for_peer(shared, from) else { continue };
        client.heard(shared.started.elapsed());
        match client.upstream.send_to(&buffer[..length], shared.config.upstream) {
            Ok(_) => {
                client.up();
                shared.stats.up(length);
            }
            Err(_) => shared.stats.error(),
        }
    }
}
