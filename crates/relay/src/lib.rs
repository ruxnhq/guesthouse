//! A UDP relay between the network and a loopback port, for a machine's remote view.
//!
//! QEMU forwards a guest's port to a loopback port on this PC, and nothing else can reach that. A
//! [`Relay`] listens on a port that other devices can reach and passes each device's datagrams on to the
//! loopback port, and the answers back. It never looks inside a datagram: what goes through is encoded
//! video one way and small input packets the other.
//!
//! It is built for a steady delay rather than for speed. Each device gets a socket of its own towards the
//! loopback port, because QEMU's forward tells flows apart by the sender's address and port. Each of those
//! sockets has a thread of its own that waits in the kernel for the next datagram, so a burst of video
//! for one device never holds up another device's, and the thread that carries input in never has video
//! to carry out. Nothing in the path of a datagram waits for more than a lookup in a small table.
//!
//! What it does not do is decide who may connect. The guest's host does that with its own keys, and
//! this only keeps the number of devices and the idle ones in check.

#[cfg(target_os = "linux")]
mod clients;
#[cfg(target_os = "linux")]
mod relay;
#[cfg(target_os = "linux")]
mod sockets;
#[cfg(target_os = "linux")]
mod stats;

#[cfg(target_os = "linux")]
pub mod signal;

#[cfg(target_os = "linux")]
pub use relay::{Config, Relay};
#[cfg(target_os = "linux")]
pub use stats::{ClientInfo, Counters};
