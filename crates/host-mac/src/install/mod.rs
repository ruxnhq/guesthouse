// SPDX-License-Identifier: GPL-2.0-or-later
//! Installing a guest from nothing, with one press: the plan (what is done, in what order, for a
//! Windows or an Ubuntu machine), the state it is in (for the screens and for starting again after
//! the app was closed), the fetching of the installer, the answers the guest's installer is given,
//! and the runner that takes one step after another.
//!
//! The steps that need a guest (installing the system, the graphics driver, the first start) are
//! written against `runner::Steps` and `steps::MachineControl`, so everything but the guest itself
//! is tested with stand-ins. Nothing here starts a machine by itself.
//!
//! The names (`Step`, the failure, its choices) are the ones the install screens use.

pub mod driver;
pub mod fake;
pub mod fetch;
pub mod machine;
pub mod password;
pub mod plan;
pub mod runner;
pub mod state;
pub mod steps;
pub mod stick;
pub mod ubuntu;
pub mod windows;
