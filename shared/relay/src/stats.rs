//! What a relay counts, and how it is read back.

use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Duration;

/// Totals since the relay was started. A [`RelayStatus`](crate::RelayStatus) carries them as they are.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counters {
    /// Datagrams from devices that were passed on, and their bytes.
    pub up_packets: u64,
    pub up_bytes: u64,
    /// Datagrams from the guest's port that were passed on to a device, and their bytes.
    pub down_packets: u64,
    pub down_bytes: u64,
    /// Devices that got a socket of their own, and how many of them have gone since.
    pub clients_started: u64,
    pub clients_expired: u64,
    /// Datagrams from a new device that were dropped because the relay already had as many as it allows.
    pub refused_full: u64,
    /// Datagrams to a device's socket from anywhere but the guest's port, which were dropped. Only
    /// counted where the relay checks, as [`Config::only_upstream_answers`] says. Left out by a relay
    /// older than the count, which reads as none.
    ///
    /// [`Config::only_upstream_answers`]: crate::Config::only_upstream_answers
    #[serde(default)]
    pub refused_stranger: u64,
    /// Sends and receives that failed. On Windows that includes each datagram to a guest's port nobody
    /// listens on, as the device's socket's next receive fails with it. Linux says nothing of those.
    pub errors: u64,
}

#[derive(Default)]
pub(crate) struct Stats {
    up_packets: AtomicU64,
    up_bytes: AtomicU64,
    down_packets: AtomicU64,
    down_bytes: AtomicU64,
    clients_started: AtomicU64,
    clients_expired: AtomicU64,
    refused_full: AtomicU64,
    refused_stranger: AtomicU64,
    errors: AtomicU64,
}

impl Stats {
    pub(crate) fn up(&self, bytes: usize) {
        self.up_packets.fetch_add(1, Relaxed);
        self.up_bytes.fetch_add(bytes as u64, Relaxed);
    }

    pub(crate) fn down(&self, bytes: usize) {
        self.down_packets.fetch_add(1, Relaxed);
        self.down_bytes.fetch_add(bytes as u64, Relaxed);
    }

    pub(crate) fn client_started(&self) {
        self.clients_started.fetch_add(1, Relaxed);
    }

    pub(crate) fn client_expired(&self) {
        self.clients_expired.fetch_add(1, Relaxed);
    }

    pub(crate) fn refused(&self) {
        self.refused_full.fetch_add(1, Relaxed);
    }

    pub(crate) fn stranger(&self) {
        self.refused_stranger.fetch_add(1, Relaxed);
    }

    pub(crate) fn error(&self) {
        self.errors.fetch_add(1, Relaxed);
    }

    pub(crate) fn read(&self) -> Counters {
        Counters {
            up_packets: self.up_packets.load(Relaxed),
            up_bytes: self.up_bytes.load(Relaxed),
            down_packets: self.down_packets.load(Relaxed),
            down_bytes: self.down_bytes.load(Relaxed),
            clients_started: self.clients_started.load(Relaxed),
            clients_expired: self.clients_expired.load(Relaxed),
            refused_full: self.refused_full.load(Relaxed),
            refused_stranger: self.refused_stranger.load(Relaxed),
            errors: self.errors.load(Relaxed),
        }
    }
}

/// One device the relay is carrying datagrams for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientInfo {
    pub address: SocketAddr,
    /// How long since the device last sent anything.
    pub idle: Duration,
    pub up_packets: u64,
    pub down_packets: u64,
}
