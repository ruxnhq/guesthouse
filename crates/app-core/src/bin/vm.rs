//! `guesthouse-vm COMMAND [MACHINE]`: the machines from a terminal, with no window and no GTK, so it
//! runs from the release folder on a server. The work is in [`guesthouse_app_core::headless`].

#[cfg(target_os = "linux")]
#[path = "vm/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
use linux::main;

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("guesthouse-vm runs on Linux only.");
    std::process::ExitCode::FAILURE
}
