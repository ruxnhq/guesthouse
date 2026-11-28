// SPDX-License-Identifier: GPL-2.0-or-later
//! `guesthouse-report`: gathers a bug report about one machine and this Mac into one file, shows
//! The program is for macOS; on any other system it does nothing, so the workspace builds everywhere.

#[cfg(target_os = "macos")]
#[path = "guesthouse_report/real.rs"]
mod real;

#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    real::main()
}

#[cfg(not(target_os = "macos"))]
fn main() -> std::process::ExitCode {
    std::process::ExitCode::SUCCESS
}
