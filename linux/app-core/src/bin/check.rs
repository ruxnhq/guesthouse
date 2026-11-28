//! `guesthouse-check`: what this PC can run, for `install.sh`. It's a program of its own because it has
//! to run from a download, before GTK may be on this PC. It changes nothing, and opens no device.
//!
//! Each line is `check`, the state, the title, what was found and the fix. A `gtk` line has the version
//! of GTK, if there is one, so the script can choose the app that runs on it. Then come `gpu`, `amd` and
//! `intel`, each `yes` or `no`, all split by tabs. The script prints these words and never runs them.
//!
//! This format is stable. Scripts, `install.sh` and the Proxmox host check read it, and it only changes
//! in a release whose notes say so. The states are `ok`, `warning` and `failed`, and a tab or line break inside
//! a field becomes a space. A new kind of line may come after these, so a reader skips the words it
//! doesn't know. A test holds it exactly. docs/check-output.md has the whole format.
//!
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
