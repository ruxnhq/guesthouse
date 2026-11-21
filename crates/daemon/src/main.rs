//! `guesthouse-daemon [PROJECT_FOLDER]`: serves one VM, named by `GUESTHOUSE_SLOT`.
//!
//! The real backends are behind the `nvidia`, `amd` and `intel` features. Each run opens one card:
//! the AMD card named in `GUESTHOUSE_AMD_CARD`, the Intel card named in `GUESTHOUSE_INTEL_CARD` or the
//! NVIDIA card named in `GUESTHOUSE_NVIDIA_CARD` if the app named one, and else the NVIDIA card setup
//! chose. A build without `nvidia` takes its card
//! from the AMD setup file when none is named, or with no `amd` either, from the Intel one. A build
//! with no backend can only say so. The `stand-in` feature answers messages with nothing behind
//! them, to try the sockets by hand.

#[cfg(target_os = "linux")]
#[path = "main/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
use linux::main;

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("guesthouse-daemon runs on Linux only.");
    std::process::ExitCode::FAILURE
}
