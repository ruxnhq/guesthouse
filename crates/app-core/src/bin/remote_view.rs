//! `guesthouse-remote-view MACHINE ...`: carries a machine's remote view between the network and the
//! loopback port QEMU forwards the guest's port to. It outlives the app, and ends with the machine.

#[cfg(target_os = "linux")]
#[path = "remote_view/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
use linux::main;

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("guesthouse-remote-view runs on Linux only.");
    std::process::ExitCode::FAILURE
}
