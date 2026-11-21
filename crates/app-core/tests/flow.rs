//! The whole flow, as the app drives it: a machine is started by a real task, runs, and is stopped.
//!
//! This program also stands in for QEMU and the daemon. The task runs copies of it under their names,
//! and a copy only answers as they would, so no VM runs. Set `GUESTHOUSE_TEST_DAEMON` to a build of
//! `guesthouse-daemon` with the `stand-in` feature to go through the real daemon instead.

#[cfg(target_os = "linux")]
#[path = "flow/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
use linux::main;

#[cfg(not(target_os = "linux"))]
fn main() {
    println!("flow tests Linux, so it does nothing here.");
}
