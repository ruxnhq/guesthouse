//! A Windows host's half of Guesthouse: Windows' Host Compute System, which runs a VM with only the
//! Virtual Machine Platform feature on, so Windows 11 Home can run one too. Hyper-V and its tools
//! are never needed.
//!
//! `schema` is the machine document and the other documents HCS and its network take, as types, and
//! it builds on every system so its tests run anywhere. The rest is Windows only:
//!
//! - `system`: computecore.dll, which creates, starts, changes and stops a machine.
//! - `network`: computenetwork.dll, the network a machine's card is plugged into.
//! - `disk`: virtdisk.dll, which makes, attaches and detaches a VHDX or an ISO, and the volumes on them.
//! - `gpu`: the host's cards that can be shared with a guest, and their drivers' folders.
//! - `drivers`: the copy of a card's driver a guest needs, put on its disk while the machine is off.
//! - `host`: whether this process is elevated, which HCS needs, and Ctrl+C.
//!
//! The JSON shapes, the order a machine gets its GPU in, and where a guest looks for its host's
//! driver were first worked out in App Sandbox, as `THIRD-PARTY.md` says.

pub mod schema;

#[cfg(windows)]
pub mod disk;
#[cfg(windows)]
pub mod drivers;
#[cfg(windows)]
mod error;
#[cfg(windows)]
pub mod gpu;
#[cfg(windows)]
pub mod host;
#[cfg(windows)]
pub mod network;
#[cfg(windows)]
pub mod serial;
#[cfg(windows)]
pub mod system;
#[cfg(windows)]
mod text;

#[cfg(windows)]
pub use error::{Error, Result};
