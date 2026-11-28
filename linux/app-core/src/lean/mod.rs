//! Lean Windows: a Windows guest without the apps Windows adds for every new account, and with a few
//! documented policies that keep programs from running in the background. It is a choice of its own,
//! off unless the user, or the recipe they pick, turns it on. An install with it off is exactly as it
//! was without it: no file, no line in the answer file, nothing on the disc.
//!
//! `list` is what it takes out and switches off. Before the account's first logon it runs from the
//! answer disc, in Setup's specialize pass, and once the install is up it runs again through the agent
//! (`actions/lean.rs`), to catch what reached the account anyway.
//!
//! The tables, the words and the files a machine keeps are `guesthouse_recipes_core::lean`'s, and the
//! answer file's step is `guesthouse_answers_core::linux_host::lean`'s. This module keeps the names they
//! have always had here, and adds where a machine's folder is.

mod list;
mod record;

pub use list::{APPS, LEFT_ALONE, POLICIES, guest_data, what_it_does};
pub use record::{
    Outcome, RESULT_FILE, names_back, names_taken_out, read_outcome, state_text, titles, write_outcome,
};

use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::machine::Machine;
pub use guesthouse_recipes_core::lean::{DATA_ON_DISC, LEAN_FILE, SCRIPT};
#[cfg(test)]
use std::fs;

/// The command that runs the script in Setup's specialize pass, the last moment before any account
/// exists, as the shared crate adds it.
#[cfg(test)]
use guesthouse_answers_core::linux_host::lean::SPECIALIZE_COMMAND;

/// Whether the machine was made lean. A file that can't be read is no, which leaves Windows as it is.
pub fn is_lean(context: &Context, machine: &Machine) -> bool {
    guesthouse_recipes_core::lean::is_lean(&machine.folder(context))
}

/// Keeps the choice. Off leaves no file, so a machine made without it looks as one always did.
pub fn write_lean(context: &Context, machine: &Machine, lean: bool) -> Result<()> {
    Ok(guesthouse_recipes_core::lean::write_lean(&machine.folder(context), lean)?)
}

/// An import's copies of the choice and of what lean Windows did, read as the app's own and written
/// out again, or left out. What it did keeps only names on the list, and none of what the other PC's
/// guest said: the first start here looks again.
pub fn remake_imported(folder: &std::path::Path) -> Result<()> {
    Ok(guesthouse_recipes_core::lean::remake_imported(folder)?)
}

/// The filled answer file with one command more in the specialize pass, which runs the script on the
/// disc. The template is never changed, so an answer file without it is the same as before.
pub fn with_specialize_step(answers: &str) -> Result<String> {
    guesthouse_answers_core::linux_host::lean::with_specialize_step(answers).map_err(Failure::new)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
