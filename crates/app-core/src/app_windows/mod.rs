//! App windows: one Windows program shown as a window of its own on the Linux desktop.
//!
//! The pieces, from the wire up.
mod geometry;
#[cfg(test)]
mod geometry_tests;
mod message;
#[cfg(test)]
mod message_tests;
mod model;
#[cfg(test)]
mod model_tests;

pub use geometry::*;
pub use message::*;
pub use model::*;
