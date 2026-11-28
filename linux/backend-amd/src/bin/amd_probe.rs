//! A standalone proof of the AMD host-execution path, run directly against the iGPU.
//!
//! It opens the card, allocates and maps buffers, writes a tiny SDMA command buffer that stores a
//! known word to a mapped buffer, submits it, waits for completion on an eventfd, and reads the word
//! back. It then measures submit-to-completion latency over many iterations and times a PRIME export
//! of one buffer. Nothing here involves a guest or any closed code.
//!
//! Run it with no arguments to use the one AMD card, or pass a PCI address to pick one of several.

#[cfg(target_os = "linux")]
#[path = "amd_probe/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
use linux::main;

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("amd-probe runs on Linux only.");
    std::process::ExitCode::FAILURE
}
