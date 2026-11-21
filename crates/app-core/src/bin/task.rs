//! `guesthouse-task ACTION MACHINE [ARGUMENT...]`: one slow job for the app, in the slot named by
//! `GUESTHOUSE_SLOT`. It outlives the app, and writes each step to a small file the app reads.

#[cfg(target_os = "linux")]
#[path = "task/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
use linux::main;

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("guesthouse-task runs on Linux only.");
    std::process::ExitCode::FAILURE
}
