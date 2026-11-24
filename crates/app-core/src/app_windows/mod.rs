//! App windows: one Windows program shown as a window of its own on the Linux desktop.
//!
//! The pieces, from the wire up.
mod client;
#[cfg(test)]
mod client_tests;
mod describe;
#[cfg(test)]
mod describe_tests;
mod forward;
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
mod start;
#[cfg(test)]
mod start_tests;
mod switch;
#[cfg(test)]
mod switch_tests;

pub use client::*;
pub use describe::describe;
pub use forward::*;
pub use geometry::*;
pub use link::{HelperStarter, LinkConfig, LinkSettings, RequestFailure, Status, Timing};
pub use message::*;
pub use model::*;
pub use start::*;
pub use switch::*;
