//! A recipe's steps: a stage of their own that runs once the install has fully succeeded and the guest
//! is up. A step that fails never turns a good install into a failed machine. The machine stays, the
//! user is told which step it was, and the machine's settings can run the steps again.
//!
//! Windows runs them through the guest's agent (`windows.rs`) and Ubuntu over SSH (`ubuntu.rs`). A
//! machine made with no recipe has no recipe file, and none of this does anything for it.

mod ubuntu;
mod windows;

use super::Task;
use crate::failure::{Failure, Result};
use crate::recipe::{Chosen, Guest, Outcome, read_chosen, read_outcome, write_outcome};
use crate::text::base64_encode;

/// A throwaway's run is gone at its next start, and so would be what the steps install.
const NOT_ON_THROWAWAY: &str = "Its recipe's steps did not run, as a throwaway loses what they install when it \
                                shuts down. Turn Throwaway off and run them from its settings to keep them.";

/// The most of what a guest said that goes into a line a person reads.
const WHY_LONGEST: usize = 300;

/// Why the steps stopped: the step's words, and what the guest said of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Stopped {
    pub step: String,
    pub why: String,
}

impl Stopped {
    pub(super) fn new(step: &str, why: impl Into<String>) -> Self {
        Self { step: step.to_owned(), why: one_line(&why.into()) }
    }
}

/// What a guest said as one line of plain text, and the end of it, which is where the reason is.
pub(super) fn one_line(text: &str) -> String {
    let plain: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let words = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    match words.char_indices().rev().nth(WHY_LONGEST - 1) {
        Some((start, _)) if start > 0 => format!("...{}", &words[start..]),
        _ => words,
    }
}

/// The step as the user sees it while it runs.
pub(super) fn show(task: &Task, chosen: &Chosen, number: usize, name: &str) {
    let total = chosen.recipe.step_names().len();
    task.step(&format!("Setting up for {}: {name} ({number} of {total})", chosen.recipe.name));
}

/// The steps as the guest is given them: each a number, a kind and one value, which only ever travels
/// as JSON or Base64 and is never put into a script's text.
pub(super) fn numbered_steps(chosen: &Chosen) -> Vec<(usize, &'static str, &str)> {
    let packages = chosen.recipe.steps.packages.iter().map(|name| ("package", name.as_str()));
    let script = chosen.recipe.steps.script.iter().map(|text| ("script", text.as_str()));
    packages.chain(script).enumerate().map(|(index, (kind, value))| (index + 1, kind, value)).collect()
}

pub(super) fn encoded(text: &str) -> String {
    base64_encode(text.as_bytes())
}

/// Runs the steps of the machine's recipe, and keeps how it went.
fn stage(task: &mut Task, chosen: &Chosen) -> std::result::Result<(), Stopped> {
    if chosen.recipe.step_names().is_empty() {
        let _ = write_outcome(task.context, &task.machine, &Outcome::Done);
        return Ok(());
    }
    task.step(&format!("Setting up for {}", chosen.recipe.name));
    let ran = match chosen.recipe.guest {
        Guest::Windows => windows::run(task, chosen),
        Guest::Ubuntu => ubuntu::run(task, chosen),
    };
    let outcome = match &ran {
        Ok(()) => Outcome::Done,
        Err(stopped) => Outcome::Failed { step: stopped.step.clone(), why: stopped.why.clone() },
    };
    // The steps have run, and a note that can't be kept costs only the settings page's words.
    let _ = write_outcome(task.context, &task.machine, &outcome);
    ran
}

fn failure_text(chosen: &Chosen, stopped: &Stopped) -> String {
    let again = "Run the recipe again from the machine's settings.";
    format!("The recipe {} stopped at \"{}\": {} {again}", chosen.recipe.name, stopped.step, stopped.why)
}

/// What an install does last. The machine is installed and, if it fit, started, so the recipe's steps
/// run now. Whatever they do, the install has succeeded: a failure is only told. A machine left off
/// has its steps waiting.
pub(super) fn after_install(task: &mut Task) {
    let Some(chosen) = read_chosen(task.context, &task.machine) else { return };
    if task.machine.throwaway && !chosen.recipe.step_names().is_empty() {
        task.error = [task.error.as_str(), NOT_ON_THROWAWAY].join(" ").trim().to_owned();
        return;
    }
    if task.pid().is_none() {
        let note = "Its recipe's steps wait for the machine to run. Run them from its settings.";
        task.error = [task.error.as_str(), note].join(" ").trim().to_owned();
        return;
    }
    // What the start said, such as that it started without the GPU, is about the start, which came
    // first. Said as a step now, it's read before the steps' own, and it stays in what the task says
    // at its end.
    if !task.error.is_empty() && !chosen.recipe.step_names().is_empty() {
        task.step(&task.error.clone());
    }
    if let Err(stopped) = stage(task, &chosen) {
        let told = failure_text(&chosen, &stopped);
        task.error = [task.error.as_str(), told.as_str()].join(" ").trim().to_owned();
    }
}

/// A recipe's steps again, as the machine's settings ask. This is the user's own request, so a step
/// that fails is this task's failure. On a throwaway they run for this run only, and how they went
/// goes back to the clean state's at its next start.
pub(super) fn run_again(task: &mut Task) -> Result<()> {
    let Some(chosen) = read_chosen(task.context, &task.machine) else {
        return Err(Failure::new("This machine has no recipe."));
    };
    if task.machine.installing {
        return Err(Failure::new("The install has not finished, and the recipe runs after it."));
    }
    if task.pid().is_none() {
        return Err(Failure::new("Start the machine first. The recipe runs in the guest."));
    }
    stage(task, &chosen).map_err(|stopped| Failure(failure_text(&chosen, &stopped)))
}

/// Forgets how the last run went, for an install that starts over.
pub(super) fn reset(task: &Task) {
    if read_chosen(task.context, &task.machine).is_some()
        && read_outcome(task.context, &task.machine) != Outcome::Pending
    {
        let _ = write_outcome(task.context, &task.machine, &Outcome::Pending);
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[cfg(test)]
#[path = "runner_tests.rs"]
mod runner_tests;
