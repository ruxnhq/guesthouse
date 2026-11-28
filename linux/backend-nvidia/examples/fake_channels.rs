//! `fake_channels`: makes each kind of channel against the pretend RM, and prints every RM call and
//! every word left in the channels' memory, to compare with another implementation of the same rules.

#[cfg(target_os = "linux")]
#[path = "fake_channels/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
use linux::main;

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("fake_channels runs on Linux only.");
    std::process::exit(1);
}
