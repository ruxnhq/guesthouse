//! `guesthouse-report`: gathers a bug report about one machine and this PC into one file, shows what's
//! in it, and sends it to the developer if the tester says yes. It's a program of its own, with no
//! GTK, so it works when the app won't open or never could.

#[cfg(target_os = "linux")]
#[path = "report/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
use linux::main;

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("guesthouse-report runs on Linux only.");
    std::process::ExitCode::FAILURE
}
