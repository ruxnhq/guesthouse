//! `fake_adapter DRIVER_FOLDER OUTPUT_FOLDER [ada|ampere|turing]`: writes what the backend works out for a
//! pretend card, to compare with another implementation of the same rules. It never touches a real
//! card. The driver's INF has to list the pretend card's device.

#[cfg(target_os = "linux")]
#[path = "fake_adapter/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
use linux::main;

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("fake_adapter runs on Linux only.");
    std::process::exit(1);
}
