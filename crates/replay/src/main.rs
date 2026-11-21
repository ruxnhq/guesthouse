//! `guesthouse-replay RECORDING`: replays a recording and says whether every reply matched.

#[cfg(target_os = "linux")]
#[path = "main/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
use linux::main;

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("guesthouse-replay runs on Linux only.");
    std::process::ExitCode::FAILURE
}
