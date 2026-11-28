//! App windows with no display: each program of a Windows guest shown as a window of its own on the
//! host. The guest's half is a helper in the user's session (`guest/windows/app-windows.ps1` and
//! `guest/windows/app-windows.cs`) that parks each program's window in its own cell of the guest's screen and
//! reports the cells; a host shows each cell as a window cropped from the guest's one frame. This crate
//! is the part of the host's half that calls nothing:
//!
//! - `message`: the JSON lines both ways, read strictly, written as the contract has them.
//! - `model`: the cells of one machine as the helper's layouts arrive, and what each layout changed.
//! - `geometry`: where things are, in numbers, for a viewer that places host windows and sends input.
//! - [`describe`]: one cell as one line for a person.
//!
//! The name is the cells' and not the feature's, as `windows/app` is the Windows host's app.
//! Keeping the link to the helper, starting it through the guest's agent, the Linux launcher's entries,
//! the machine's switch and its Apps page stay with the host. Only the Linux host shows app windows
//! today, so the crate has no module per host.

pub mod describe;
pub mod geometry;
pub mod message;
pub mod model;

pub use describe::describe;
pub use geometry::*;
pub use message::*;
pub use model::*;

#[cfg(test)]
mod tests;
