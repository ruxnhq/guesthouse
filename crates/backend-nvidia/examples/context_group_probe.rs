//! `context_group_probe`: runs the channel layout of a guest context's queues on the host's real
//! card with no guest, and times a wait of one channel for another.
//!
//! `context_group_probe grouped` makes a drawing channel and two compute channels in one group, as
//! [`Channel::new_in_context`] does for a guest context. `context_group_probe alone` makes three
//! channels with a group each. Either way every channel runs a semaphore release, to show it was
//! started, and then a compute channel waits many times for a semaphore the drawing channel
//! releases, which is what a Direct3D 12 queue's channels do to each other every frame.

#[cfg(target_os = "linux")]
#[path = "context_group_probe/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
use linux::main;

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("context_group_probe runs on Linux only.");
    std::process::exit(1);
}
