//! Builds the guest's adapter block from the real AMD card and sets its key fields beside what
//! amdgpu reports on its own in sysfs, so a field read from the wrong INFO offset shows up.
//!
//! The sysfs side is independent of the INFO queries the block is built from: memory sizes from
//! amdgpu's mem_info files, the graphics block's version from IP discovery, and compute units from
//! the compute driver's topology. Run it with no arguments to use the one AMD card, or pass a PCI
//! address to pick one of several. It never opens anything but that card's render node.

#[cfg(target_os = "linux")]
#[path = "amd_adapter_check/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
use linux::main;

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("amd-adapter-check runs on Linux only.");
    std::process::ExitCode::FAILURE
}
