//! The daemon's jail on this PC's kernel, with no GPU. A child applies the daemon's own jail and tries
//! what a guest that had taken it over would. With the `stand-in` feature, the real program is run
//! too, and has to be serving while jailed.

#[cfg(target_os = "linux")]
#[path = "jail/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
use linux::main;

#[cfg(not(target_os = "linux"))]
fn main() {
    println!("jail tests Linux, so it does nothing here.");
}
