//! `fake_frames RUN_FOLDER [FRAMES_A_SECOND]`: serves `frames.sock` with a pretend guest on the pretend
//! RM, for trying a viewer or a test client with no card and no VM. The pictures are the pretend
//! engine's stripes, which move from frame to frame.

#[cfg(target_os = "linux")]
#[path = "fake_frames/linux.rs"]
mod linux;

#[cfg(target_os = "linux")]
use linux::main;

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("fake_frames runs on Linux only.");
    std::process::exit(1);
}
