//! App windows: one Windows program shown as a window of its own on the Linux desktop. This is the
//! host's half that needs no display. The guest's half is a helper in the user's session
//! (`guest/app-windows.ps1`) that parks each program's window in its own cell of the guest's screen and
//! reports the cells. The viewer shows each cell as a host window cropped from the one frame.
//!
//! How the pieces fit, from the wire up:
//!
//! - `message`: the JSON lines both ways, read strictly, written as the contract has them.
//! - [`Client`] starts a thread that keeps the link to one machine's helper (`link`), and holds the
//!   [`Model`] that the helper's layouts keep up. A GTK viewer builds it with [`connect`], calls
//!   [`Client::poll`] from a `glib::timeout_add_local` as the window-size helper's sizes are read, and
//!   gets [`Update`]s: `Changes` say which cells are new (open a host window; a window comes before its
//!   popups), which changed and in what (move the crop, retitle, minimize), and which are gone (close it).
//!   It sends [`Request`]s with [`Client::request`] and finds each answer as an [`Update::Answered`].
//!   The command line uses [`Client::ask`] and [`Client::wait_for`], which wait as long as they are told.
//! - `geometry` is what the viewer calls to place things: [`guest_point`] for every pointer event
//!   (window or popup alike), [`crop`] for what a host window shows, [`popup_place`] for a popup against
//!   its parent's surface, [`drag_strip`] and [`on_drag_strip`] for moving the host window by its
//!   Windows title bar. Its header says what it assumes about scale.
//! - [`is_on`] and [`set_on`] are the per-machine switch. [`connect`] refuses a machine that has it off,
//!   and the machine only gets its forward (`forward`) at a start with it on.
//! - [`stand_in`] is a pretend helper to test any of this against, with no guest.
//!
//! One helper takes one connection, so a viewer is the only long-lived client of a machine. A command
//! that connects while a viewer is open takes the connection for a moment, and the viewer's client
//! connects again after its wait.

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
pub mod launcher;
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
pub mod viewer_lock;

pub use client::*;
pub use describe::describe;
pub use forward::*;
pub use geometry::*;
pub use link::{HelperStarter, LinkConfig, LinkSettings, RequestFailure, Status, Timing};
pub use message::*;
pub use model::*;
pub use start::*;

pub use switch::*;
