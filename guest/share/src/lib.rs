//! The Windows guest's share program: it mounts a host folder, served by Hyper-V's Plan 9 device,
//! as a drive, with WinFsp as the file system and `guesthouse-p9` as the client.
//!
//! It runs as SYSTEM, from a scheduled task, and the drive it mounts is in the global namespace, so
//! every user of the guest sees it, not only a session's.
//!
//! The rules and arithmetic (`logic`) and WinFsp's table layouts (`ffi`) build and are tested
//! anywhere. The rest calls Windows and builds only there.

pub mod ffi;
pub mod logic;

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod fs;
#[cfg(windows)]
mod hv;
#[cfg(windows)]
mod winfsp;

/// Runs the program with its command line and gives the exit code.
#[cfg(windows)]
pub fn run(args: impl IntoIterator<Item = String>) -> i32 {
    app::run(args)
}

/// This program mounts a drive in a Windows guest, and does nothing elsewhere.
#[cfg(not(windows))]
pub fn run(args: impl IntoIterator<Item = String>) -> i32 {
    match logic::parse_args(args) {
        Ok(logic::Command::Help) => {
            print!("{}", logic::USAGE);
            0
        }
        _ => {
            eprintln!("guesthouse-guest-share runs in a Windows guest only");
            1
        }
    }
}
