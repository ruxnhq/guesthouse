// SPDX-License-Identifier: GPL-2.0-or-later
//! A stand-in for QEMU, for trying the agent with no guest. It takes the arguments the agent
//! The program is for macOS; on any other system it does nothing, so the workspace builds everywhere.

#[cfg(target_os = "macos")]
#[path = "fake_qemu/real.rs"]
mod real;

#[cfg(target_os = "macos")]
fn main() {
    real::main()
}

#[cfg(not(target_os = "macos"))]
fn main() {}
