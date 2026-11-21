//! `guesthouse-check`: what this PC can run, for `install.sh`. It's a program of its own because it has
//! to run from a download, before GTK may be on this PC. It changes nothing, and opens no device.
//!
//! Each line is `check`, the state, the title, what was found and the fix. A `gtk` line has the version
//! of GTK, if there is one, so the script can choose the app that runs on it. Then come `gpu`, `amd` and
//! `intel`, each `yes` or `no`, all split by tabs. The script prints these words and never runs them.
//! `--firmware` prints the firmware a plain VM would boot, and nothing else.

#[cfg(target_os = "linux")]
#[path = "check/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
use linux::main;

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("guesthouse-check runs on Linux only.");
    std::process::ExitCode::FAILURE
}
