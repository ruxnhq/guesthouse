// SPDX-License-Identifier: GPL-2.0-or-later
//! A stand-in for `guesthouse-pvg-host`, for trying the agent with no graphics. It takes the
//! The program is for macOS; on any other system it does nothing, so the workspace builds everywhere.

#[cfg(target_os = "macos")]
#[path = "fake_pvg_host/real.rs"]
mod real;

#[cfg(target_os = "macos")]
fn main() {
    real::main()
}

#[cfg(not(target_os = "macos"))]
fn main() {}
