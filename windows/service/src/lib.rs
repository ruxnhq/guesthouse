//! The Guesthouse service on a Windows host. HCS, its network and attaching a disk all need an
//! administrator, and Guesthouse's app runs as its user, so the service does that work for it: it
//! runs as the system, keeps the machines in a folder only the system and administrators may open,
//! and answers what the app asks on its pipe, in the words of `guesthouse-control`.
//!
//! Most of it builds on every system, so its tests run anywhere: [`store`], where the machines are
//! kept; [`machines`], what each is doing and who is watching; and [`requests`], the answer to each
//! request, with the work and the pipe behind traits. [`windows`] is the rest: the service manager,
//! the pipe, and the work done by `guesthouse-host-windows`.

pub mod machines;
pub mod report_parts;
pub mod requests;
pub mod store;
#[cfg(windows)]
pub mod windows;
