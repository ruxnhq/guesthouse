//! App windows: one Windows program shown as a window of its own on the Linux desktop.
//!
//! The pieces, from the wire up.
mod message;
#[cfg(test)]
mod message_tests;
mod model;
#[cfg(test)]
mod model_tests;

pub use message::*;
pub use model::*;
