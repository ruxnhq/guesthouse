//! Work on the card through the kernel's xe driver.
//!
//! [`abi`] is the calls as bytes, [`batch`] the few instructions written here, [`plan`] the
//! decisions that need no card, and [`device`] the thin layer that calls the kernel, behind a trait
//! the tests have a stand-in for. [`selftest`] runs them all on a card with no guest, which is what
//! `intel-probe --run` does. The backend itself doesn't use them yet.

pub mod abi;
pub mod batch;
pub mod device;
#[cfg(any(test, feature = "lab-fake-card"))]
pub(crate) mod fake;
pub mod guest_registers;
pub mod plan;
pub mod selftest;
