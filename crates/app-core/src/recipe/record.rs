//! What a machine was made with and how its steps went, in two small files in its folder. They sit
//! beside `machine.json` and not in it, as `remote-access.json` does, so an older build still reads the
//! machine. The recipe is kept as it was when the user said yes, and a run again runs that copy, so a
//! file edited since is never run by the machine unseen.

use super::format::Recipe;
use super::shelf::Source;
use crate::context::Context;
use crate::failure::Result;
use crate::machine::Machine;
use crate::settings::write_json;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

pub const CHOSEN_FILE: &str = "recipe.json";
/// How the steps went describes what is on the disk, so a snapshot keeps it and a throwaway's run
/// starts from its clean one, as with the other notes about the guest.
pub const OUTCOME_FILE: &str = "recipe-result.json";

/// The recipe a machine was made for, and where it came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chosen {
    pub source: Source,
    /// The file it was read from, for the words that say where it came from.
    pub file: String,
    pub recipe: Recipe,
}

/// How the steps went the last time they ran.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum Outcome {
    /// Not run yet, or the install ended with the machine off.
    #[default]
    Pending,
    Done,
    /// `step` is the step's words, and `why` what the guest said of it.
    Failed {
        step: String,
        why: String,
    },
}

fn chosen_file(context: &Context, machine: &Machine) -> PathBuf {
    machine.folder(context).join(CHOSEN_FILE)
}

fn outcome_file(context: &Context, machine: &Machine) -> PathBuf {
    machine.folder(context).join(OUTCOME_FILE)
}

/// The recipe the machine was made for, if it was made for one. A file that can't be read, or whose
/// recipe fails its checks, is no recipe: the machine is a machine without one.
pub fn read_chosen(context: &Context, machine: &Machine) -> Option<Chosen> {
    let text = fs::read_to_string(chosen_file(context, machine)).ok()?;
    let chosen: Chosen = serde_json::from_str(&text).ok()?;
    chosen.recipe.check().ok()?;
    Some(chosen)
}

pub fn write_chosen(context: &Context, machine: &Machine, chosen: &Chosen) -> Result<()> {
    write_json(&chosen_file(context, machine), chosen)?;
    write_outcome(context, machine, &Outcome::Pending)
}

pub fn read_outcome(context: &Context, machine: &Machine) -> Outcome {
    let text = fs::read_to_string(outcome_file(context, machine)).unwrap_or_default();
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn write_outcome(context: &Context, machine: &Machine, outcome: &Outcome) -> Result<()> {
    write_json(&outcome_file(context, machine), outcome)
}

#[cfg(test)]
#[path = "record_tests.rs"]
mod tests;
