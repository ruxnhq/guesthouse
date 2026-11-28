//! A UDP relay between the network and a guest's port, for a machine's remote view, and the network side
//! of that view which a host runs it in.
//!
//! On the Linux host QEMU forwards a guest's port to a loopback port on this PC, and nothing else can
//! reach that. On the Windows host the guest has an address of its own on a network only this PC routes
//! to. Either way, a [`Relay`] listens on a port that other devices can reach and passes each device's
//! datagrams on to the guest's port, and the answers back. It never looks inside a datagram: what goes
//! through is encoded video one way and small input packets the other.
//!
//! It is built for a steady delay rather than for speed. Each device gets a socket of its own towards the
//! guest's port, because QEMU's forward tells flows apart by the sender's address and port. Each of those
//! sockets has a thread of its own that waits in the kernel for the next datagram, so a burst of video
//! for one device never holds up another device's, and the thread that carries input in never has video
//! to carry out. Nothing in the path of a datagram waits for more than a lookup in a small table.
//!
//! What it does not do is decide who may connect. The guest's host does that with its own keys, and
//! this only keeps the number of devices and the idle ones in check.
//!
//! [`Lan`] is the network side a host keeps for a machine: the relay opened and closed, the machine
//! announced while it listens, and its state in the words of [`Request`] and [`RelayReply`], which are
//! what the Linux host's app and its helper say to each other.

mod address;
mod clients;
mod control;
mod lan;
mod relay;
mod sockets;
mod stats;

#[cfg(unix)]
pub mod signal;

pub use address::{lan_address, udp_port_free};
pub use control::{RelayClient, RelayReply, RelayStatus, Request, ShareState};
pub use lan::{Lan, LanSettings};
pub use relay::{Config, Relay};
pub use stats::{ClientInfo, Counters};
