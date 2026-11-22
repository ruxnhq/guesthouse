//! Stand-in clients that show how amdgpu shares its DMA engine. See `amd_share/linux.rs`.
#[cfg(target_os = "linux")]
#[path = "amd_share/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
use linux::main;

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("amd-share runs on Linux only.");
    std::process::ExitCode::FAILURE
}
