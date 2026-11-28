//! What a recipe says it will do, in words, is `guesthouse_recipes_core`'s. What stays here is reading
//! a machine's recipe and how its steps went for those words.

use super::record::{read_chosen, read_outcome};
use crate::context::Context;
use crate::machine::Machine;
#[cfg(test)]
use guesthouse_recipes_core::{Recipe, Source};
pub use guesthouse_recipes_core::{origin_text, what_it_will_run};

/// Where a machine's recipe is from and how its steps went, for a row that has the recipe's name as its
/// title already. Nothing for a machine made with none.
pub fn origin_and_outcome(context: &Context, machine: &Machine) -> Option<String> {
    let chosen = read_chosen(context, machine)?;
    let outcome = read_outcome(context, machine);
    Some(guesthouse_recipes_core::origin_and_outcome(&chosen, &outcome, machine.throwaway))
}

/// A machine's recipe by name, and how its steps went, in a sentence or two, or nothing for a machine made
/// with none.
pub fn summary(context: &Context, machine: &Machine) -> Option<String> {
    let chosen = read_chosen(context, machine)?;
    let outcome = read_outcome(context, machine);
    Some(guesthouse_recipes_core::summary(&chosen, &outcome, machine.throwaway))
}

#[cfg(test)]
#[path = "preview_tests.rs"]
mod tests;
