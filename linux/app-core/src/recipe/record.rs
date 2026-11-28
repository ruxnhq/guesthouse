//! What a machine was made with and how its steps went, in two small files in its folder, as
//! `guesthouse_recipes_core` keeps them. What stays here is where the machine's folder is.

use crate::context::Context;
use crate::failure::Result;
use crate::machine::Machine;
pub use guesthouse_recipes_core::{CHOSEN_FILE, Chosen, OUTCOME_FILE, Outcome};
#[cfg(test)]
use guesthouse_recipes_core::{Recipe, Source};

/// The recipe the machine was made for, if it was made for one. A file that can't be read, or whose
/// recipe fails its checks, is no recipe: the machine is a machine without one.
pub fn read_chosen(context: &Context, machine: &Machine) -> Option<Chosen> {
    guesthouse_recipes_core::read_chosen(&machine.folder(context))
}

pub fn write_chosen(context: &Context, machine: &Machine, chosen: &Chosen) -> Result<()> {
    Ok(guesthouse_recipes_core::write_chosen(&machine.folder(context), chosen)?)
}

pub fn read_outcome(context: &Context, machine: &Machine) -> Outcome {
    guesthouse_recipes_core::read_outcome(&machine.folder(context))
}

pub fn write_outcome(context: &Context, machine: &Machine, outcome: &Outcome) -> Result<()> {
    Ok(guesthouse_recipes_core::write_outcome(&machine.folder(context), outcome)?)
}

#[cfg(test)]
#[path = "record_tests.rs"]
mod tests;
