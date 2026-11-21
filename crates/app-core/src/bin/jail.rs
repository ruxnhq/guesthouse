//! `guesthouse-jail PLAN -- PROGRAM ARGUMENTS`: runs a program, QEMU, in the jail the plan describes.
//! The words are the app's to write, in `vm_args`, and `guesthouse_jail::launch` says what they mean.

#[cfg(target_os = "linux")]
#[path = "jail/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
use linux::main;

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("guesthouse-jail runs on Linux only.");
    std::process::ExitCode::FAILURE
}
