//! How long a message takes from arrival to reply, with no GPU and no socket involved.
//!
//! This is a plain timing loop rather than a benchmark framework, as it needs only a median and a
//! worst case. Run it with `cargo bench -p guesthouse-core`. The number covers parsing, bookkeeping
//! and building the reply. It does not cover the socket, QEMU or the guest.

#[cfg(target_os = "linux")]
#[path = "message_path/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
use linux::main;

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("message_path runs on Linux only.");
    std::process::exit(1);
}
