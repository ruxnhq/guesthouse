//! The Linux host's machine states: the common ones in [`crate::common`], whose first five and
//! their words are the Linux host's own. It never has the other two. The app shows them by their
//! words and never sends them anywhere.

pub use crate::common::State;

#[cfg(test)]
mod tests;
