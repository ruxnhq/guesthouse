//! App windows: one Windows program shown as a window of its own on the Linux desktop.
//!
//! The pieces, from the wire up.
mod client;
#[cfg(test)]
mod client_tests;
mod geometry;
#[cfg(test)]
mod geometry_tests;
mod link;
mod message;
#[cfg(test)]
mod message_tests;
mod model;
#[cfg(test)]
mod model_tests;
pub mod stand_in;

pub use client::*;
pub use geometry::*;
pub use link::{HelperStarter, LinkConfig, RequestFailure, Status, Timing};
pub use message::*;
pub use model::*;
