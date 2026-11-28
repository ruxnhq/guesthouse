//! `round_trip SOCKET`: times submissions through a running daemon, playing QEMU on its vgpu socket.
//!
//! The time includes the socket and both processes waking up, so it can compare two daemons on the
//! same machine, whatever language they are in. It does not include QEMU, VMBus or the guest.

#[cfg(target_os = "linux")]
#[path = "round_trip/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
use linux::main;

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("round_trip runs on Linux only.");
    std::process::exit(1);
}
