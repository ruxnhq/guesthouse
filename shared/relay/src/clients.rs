//! The table of devices, and the thread that carries each one's answers back.

use crate::relay::{LOOK_AGAIN, Shared};
use crate::sockets;
use crate::stats::ClientInfo;
use std::collections::HashMap;
use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{
    AtomicU64,
    Ordering::{Acquire, Relaxed},
};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// One device and the socket that speaks for it towards the guest's port.
pub(crate) struct Client {
    peer: SocketAddr,
    pub(crate) upstream: UdpSocket,
    /// When the device last sent something, in milliseconds since the relay started.
    last_heard_ms: AtomicU64,
    up_packets: AtomicU64,
    down_packets: AtomicU64,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Client {
    pub(crate) fn heard(&self, since_start: Duration) {
        self.last_heard_ms.store(since_start.as_millis() as u64, Relaxed);
    }

    pub(crate) fn up(&self) {
        self.up_packets.fetch_add(1, Relaxed);
    }

    fn idle(&self, since_start: Duration) -> Duration {
        since_start.saturating_sub(Duration::from_millis(self.last_heard_ms.load(Relaxed)))
    }
}

#[derive(Default)]
pub(crate) struct Table {
    by_peer: Mutex<HashMap<SocketAddr, Arc<Client>>>,
}

impl Table {
    fn lock(&self) -> MutexGuard<'_, HashMap<SocketAddr, Arc<Client>>> {
        // Nothing under the lock can panic half way, so a poisoned table is still whole.
        self.by_peer.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn info(&self, since_start: Duration) -> Vec<ClientInfo> {
        let mut found: Vec<ClientInfo> = self
            .lock()
            .values()
            .map(|client| ClientInfo {
                address: client.peer,
                idle: client.idle(since_start),
                up_packets: client.up_packets.load(Relaxed),
                down_packets: client.down_packets.load(Relaxed),
            })
            .collect();
        found.sort_by_key(|client| std::cmp::Reverse(client.idle));
        found
    }

    /// Waits for every client's thread, which stop on their own once the relay's stop flag is set.
    pub(crate) fn join_all(&self) {
        let clients: Vec<Arc<Client>> = self.lock().drain().map(|(_, client)| client).collect();
        for client in clients {
            let thread = client.thread.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
            if let Some(thread) = thread {
                let _ = thread.join();
            }
        }
    }
}

/// The client for a device, made when its first datagram comes. None when the relay is full.
pub(crate) fn for_peer(shared: &Arc<Shared>, peer: SocketAddr) -> Option<Arc<Client>> {
    let mut table = shared.clients.lock();
    if let Some(client) = table.get(&peer) {
        return Some(Arc::clone(client));
    }
    if table.len() >= shared.config.max_clients {
        shared.stats.refused();
        return None;
    }
    match start(shared, peer) {
        Ok(client) => {
            table.insert(peer, Arc::clone(&client));
            shared.stats.client_started();
            Some(client)
        }
        Err(_) => {
            shared.stats.error();
            None
        }
    }
}

fn start(shared: &Arc<Shared>, peer: SocketAddr) -> io::Result<Arc<Client>> {
    let upstream = sockets::towards(shared.config.upstream)?;
    upstream.set_read_timeout(Some(LOOK_AGAIN))?;
    let client = Arc::new(Client {
        peer,
        upstream,
        last_heard_ms: AtomicU64::new(shared.started.elapsed().as_millis() as u64),
        up_packets: AtomicU64::new(0),
        down_packets: AtomicU64::new(0),
        thread: Mutex::new(None),
    });
    let handle = {
        let (shared, client) = (Arc::clone(shared), Arc::clone(&client));
        thread::Builder::new()
            .name("relay-out".into())
            .stack_size(128 * 1024)
            .spawn(move || carry_out(&shared, &client))?
    };
    *client.thread.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(handle);
    Ok(client)
}

/// The thread of one device: answers from the guest's port go back to it. When the device has been
/// quiet for too long it takes itself out of the table and ends.
///
/// A datagram that arrives from the device in the moment between this thread deciding to end and
/// leaving the table can be lost. That takes the device having been silent for the whole idle time.
fn carry_out(shared: &Arc<Shared>, client: &Arc<Client>) {
    let mut buffer = vec![0u8; 65_535];
    loop {
        match client.upstream.recv_from(&mut buffer) {
            Ok((_, from)) if shared.config.only_upstream_answers && from != shared.config.upstream => {
                shared.stats.stranger();
            }
            Ok((length, _)) => match shared.lan.send_to(&buffer[..length], client.peer) {
                Ok(_) => {
                    client.down_packets.fetch_add(1, Relaxed);
                    shared.stats.down(length);
                }
                Err(_) => shared.stats.error(),
            },
            Err(error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {
                if shared.stop.load(Acquire) {
                    return;
                }
                if client.idle(shared.started.elapsed()) >= shared.config.idle_timeout {
                    expire(shared, client);
                    return;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            // Windows says here that an earlier send was refused, as `ConnectionReset`, when nothing
            // listens on the guest's port. The socket still works, and the next datagram may find it.
            Err(_) => shared.stats.error(),
        }
    }
}

fn expire(shared: &Arc<Shared>, client: &Arc<Client>) {
    let mut table = shared.clients.lock();
    if table.get(&client.peer).is_some_and(|held| Arc::ptr_eq(held, client)) {
        table.remove(&client.peer);
        shared.stats.client_expired();
    }
}
