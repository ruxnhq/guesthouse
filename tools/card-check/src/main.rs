//! `guesthouse-card-check`: one program for anyone with an NVIDIA card, to find out what Guesthouse
//! would make of it. It reads the card through controls that only read, says what the app knows of
//! it, and then, if asked, tries the channels a VM would use. It writes one file to post.
//!
//! It needs no root, no Rust and no Windows driver. It changes nothing on the system: the objects it
//! makes in the driver go when it ends.

#[cfg(target_os = "linux")]
#[path = "main/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
use linux::main;

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("guesthouse-card-check runs on Linux only.");
    std::process::ExitCode::FAILURE
}
