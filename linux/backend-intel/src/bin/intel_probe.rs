//! `intel-probe [PCI_ADDRESS] [--record FILE] [--run [--break]]`: what the host's xe driver says
//! about an Intel card, in a form to paste into an issue.
//!
//! By itself it reads sysfs and asks xe its read-only device queries, and nothing else: no address
//! space, buffer or queue is made, so nothing runs on the card and the desktop is not touched. With
//! no address it takes the first Intel card on xe. `--record FILE` also writes the facts to FILE as
//! a recording, which a lab's daemon can stand in for the card with (see `recording`).
//!
//! `--run` then runs the host's self-test on the card: a few tiny buffers and batches of its own,
//! freed at the end, with a PASS or FAIL line for each step (see `xe::selftest`). `--break` adds
//! the two steps that make a fault and a hang on purpose, and asks for a typed yes first.

#[cfg(target_os = "linux")]
#[path = "intel_probe/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
use linux::main;

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("intel-probe runs on Linux only.");
    std::process::ExitCode::FAILURE
}
