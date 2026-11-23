//! What lean Windows did in a machine: the apps it took out at the install, which are only those this
//! Windows had, and any that Windows put back by itself later and that a start took out again. It is
//! kept beside `lean.json`, so the machine's settings and `guesthouse-vm show` can say it.

use super::list::APPS;
use crate::context::Context;
use crate::failure::Result;
use crate::machine::Machine;
use crate::settings::write_json;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

pub const RESULT_FILE: &str = "lean-result.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    /// Package names, from the list, that the install took out.
    #[serde(default)]
    pub taken_out: Vec<String>,
    /// Package names Windows put back after the install, as the last start that found any saw them.
    #[serde(default)]
    pub put_back: Vec<String>,
    /// Why what Windows put back could not be taken out again, if it couldn't.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub still_back: Option<String>,
}

fn result_file(context: &Context, machine: &Machine) -> PathBuf {
    machine.folder(context).join(RESULT_FILE)
}

pub fn read_outcome(context: &Context, machine: &Machine) -> Outcome {
    let text = fs::read_to_string(result_file(context, machine)).unwrap_or_default();
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn write_outcome(context: &Context, machine: &Machine, outcome: &Outcome) -> Result<()> {
    write_json(&result_file(context, machine), outcome)
}

/// The names on the list that a line of the guest's script says it took out, each once and in the
/// list's order. A name the list doesn't have is not believed.
pub fn names_taken_out(said: &str) -> Vec<String> {
    let done: Vec<&str> = said
        .lines()
        .filter_map(|line| {
            line.trim().strip_prefix("deprovisioned ").or_else(|| line.trim().strip_prefix("removed "))
        })
        .collect();
    APPS.iter().filter(|app| done.contains(&app.name)).map(|app| app.name.to_owned()).collect()
}

/// The names a check of the guest found there again, in the list's order.
pub fn names_back(said: &str) -> Vec<String> {
    let back: Vec<&str> = said.lines().filter_map(|line| line.trim().strip_prefix("back ")).collect();
    APPS.iter().filter(|app| back.contains(&app.name)).map(|app| app.name.to_owned()).collect()
}

/// Package names as the titles a person knows.
pub fn titles(names: &[String]) -> String {
    let titles: Vec<&str> = names
        .iter()
        .map(|name| APPS.iter().find(|app| app.name == name).map_or(name.as_str(), |app| app.title))
        .collect();
    titles.join(", ")
}

/// What lean Windows did in this machine, in a sentence or two, once it has done anything.
pub fn state_text(context: &Context, machine: &Machine) -> Option<String> {
    let outcome = read_outcome(context, machine);
    let mut parts = Vec::new();
    if !outcome.taken_out.is_empty() {
        parts.push(format!("Took out {}, which this Windows had.", titles(&outcome.taken_out)));
    }
    if !outcome.put_back.is_empty() {
        let back = titles(&outcome.put_back);
        let it = if outcome.put_back.len() == 1 { "it" } else { "them" };
        parts.push(match &outcome.still_back {
            None => format!("Windows had put back {back}, and a start took {it} out again."),
            Some(why) => format!("Windows had put back {back}, and {it} could not be taken out again: {why}"),
        });
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

#[cfg(test)]
#[path = "record_tests.rs"]
mod tests;
