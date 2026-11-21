//! The parts of `guesthouse-card-check` that take a pretend card, so the tests can run them with no
//! card at all. `main.rs` is the program around them.

// Linux only: it reads the NVIDIA driver's files and opens the card. Other systems get an empty crate.
#![cfg(target_os = "linux")]

pub mod channels;
pub mod host;
pub mod report;
pub mod survey;
