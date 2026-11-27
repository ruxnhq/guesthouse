//! `guesthouse-clock-helper hold|reset ADDRESS`, setuid root. See the library for what it decides.
//!
//! It runs as root for whoever starts it, so it trusts nothing of theirs: the environment is cleared
//! before anything else, it runs no other program and no shell, NVIDIA's library is loaded by its
//! standard name or one absolute path, and its own files are made by root in a folder only root can
//! write, never following a link.

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    linux::main()
}

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("guesthouse-clock-helper runs on Linux only.");
    std::process::ExitCode::FAILURE
}
