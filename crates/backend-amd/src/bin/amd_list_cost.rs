//! Times amdgpu's submission call with host pages on its buffer list. See `amd_list_cost/linux.rs`.
#[cfg(target_os = "linux")]
#[path = "amd_list_cost/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
use linux::main;

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("amd-list-cost runs on Linux only.");
    std::process::ExitCode::FAILURE
}
