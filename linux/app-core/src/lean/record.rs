//! What lean Windows did in a machine, kept beside `lean.json` as `guesthouse_recipes_core::lean` keeps
//! it. What stays here is where the machine's folder is.

use crate::context::Context;
use crate::failure::Result;
use crate::machine::Machine;
pub use guesthouse_recipes_core::lean::{Outcome, RESULT_FILE, names_back, names_taken_out, titles};
#[cfg(test)]
use std::fs;

pub fn read_outcome(context: &Context, machine: &Machine) -> Outcome {
    guesthouse_recipes_core::lean::read_outcome(&machine.folder(context))
}

pub fn write_outcome(context: &Context, machine: &Machine, outcome: &Outcome) -> Result<()> {
    Ok(guesthouse_recipes_core::lean::write_outcome(&machine.folder(context), outcome)?)
}

/// What lean Windows did in this machine, in a sentence or two, once it has done anything.
pub fn state_text(context: &Context, machine: &Machine) -> Option<String> {
    guesthouse_recipes_core::lean::state_text(&read_outcome(context, machine))
}

#[cfg(test)]
#[path = "record_tests.rs"]
mod tests;
