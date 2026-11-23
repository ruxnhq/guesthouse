//! Lean Windows in the installed guest. Setup already took the apps out before the account's first
//! logon; this runs the same script again through the agent, as SYSTEM, for every account, so that
//! anything that reached one anyway goes too. It needs neither the account nor winget, which is why it
//! is not a recipe step: it runs with or without a recipe, before the install's own shutdown.
//!
//! It never fails an install. What it couldn't do is a note, and the machine's settings and
//! `guesthouse-vm lean` run it again.

use super::Task;
use crate::agent::{QUICK_SECONDS, guest_script};
use crate::failure::{Failure, Result};
use crate::lean::{APPS, SCRIPT, guest_data, is_lean, names_taken_out, read_outcome, write_outcome};
use crate::text::base64_encode;
use std::time::Duration;

const START_SCRIPT: &str = "lean-start.ps1";
const START_SECONDS: u64 = 60;
const START_MARK: &str = "started";
const STATUS_ASK: &str =
    "Get-Content -LiteralPath C:\\gpupv\\lean\\status.txt -Raw -ErrorAction SilentlyContinue";
/// What both runs said: Setup's, before the first logon, and the one through the agent.
const LOGS_ASK: &str = "Get-Content -LiteralPath C:\\gpupv\\lean-setup.log, C:\\gpupv\\lean\\status.txt \
                        -ErrorAction SilentlyContinue";
/// Nobody has timed it in a guest yet. Each package takes seconds, so this is generous, and the task in
/// the guest stops itself at 30.
const LIMIT_MINUTES: u64 = 20;
const POLL: Duration = Duration::from_secs(5);
pub const STEP: &str = "Making Windows lean";
const AGAIN: &str = "Run it again from the machine's settings.";

/// How the status file ended: how many things the script couldn't do, and the first of them.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Ended {
    pub not_done: usize,
    pub first: Option<String>,
}

/// The end of the status file, once it has one. Lines the script didn't write are skipped.
pub(super) fn read_end(text: &str) -> Option<Ended> {
    let mut first = None;
    for line in text.lines().map(str::trim) {
        if let Some(what) = line.strip_prefix("kept ") {
            first.get_or_insert_with(|| super::recipe::one_line(what));
        }
        if let Some(count) = line.strip_prefix("end ") {
            return Some(Ended { not_done: count.trim().parse().unwrap_or(1), first });
        }
    }
    None
}

fn start_script(task: &Task) -> Result<String> {
    let runner = guest_script(task.context, SCRIPT)?;
    let start = guest_script(task.context, START_SCRIPT)?;
    let data = base64_encode(guest_data().as_bytes());
    Ok(format!("$data = '{data}'\n$runner = '{}'\n{start}", base64_encode(runner.as_bytes())))
}

/// Runs the script in the guest and waits for it. What it says is why it didn't all happen.
fn run(task: &Task) -> std::result::Result<(), String> {
    task.step(STEP);
    let script = start_script(task).map_err(|failure| failure.0)?;
    let started = task.ask_guest(&script, START_SECONDS);
    if !started.contains(START_MARK) {
        let said = started.trim();
        return Err(if said.is_empty() {
            "the agent went quiet".to_owned()
        } else {
            super::recipe::one_line(said)
        });
    }
    let end = task.now() + Duration::from_secs(LIMIT_MINUTES * 60);
    loop {
        task.sleep(POLL);
        if task.pid().is_none() {
            return Err("the machine stopped first".to_owned());
        }
        if let Some(ended) = read_end(&task.ask_guest(STATUS_ASK, QUICK_SECONDS)) {
            return match ended {
                Ended { not_done: 0, .. } => Ok(()),
                Ended { not_done, first } => {
                    let what = first.map_or(String::new(), |first| format!(", the first: {first}"));
                    Err(format!("{not_done} things were left as they were{what}"))
                }
            };
        }
        if task.now() >= end {
            return Err(format!("it did not finish in {LIMIT_MINUTES} minutes"));
        }
    }
}

/// Keeps which apps the runs took out, as only those this Windows had are. A note that can't be kept
/// costs only the settings page's words.
fn note_taken_out(task: &Task) {
    let found = names_taken_out(&task.ask_guest(LOGS_ASK, QUICK_SECONDS));
    let mut outcome = read_outcome(task.context, &task.machine);
    let before = outcome.taken_out.clone();
    outcome.taken_out = APPS
        .iter()
        .map(|app| app.name.to_owned())
        .filter(|name| before.contains(name) || found.contains(name))
        .collect();
    if outcome.taken_out != before {
        let _ = write_outcome(task.context, &task.machine, &outcome);
    }
}

/// Part of an install, once its desktop has been checked. A machine made without the choice is not
/// touched, and one where it didn't all happen is installed all the same, with a note.
pub(super) fn after_install(task: &mut Task) {
    if !is_lean(task.context, &task.machine) {
        return;
    }
    let ran = run(task);
    note_taken_out(task);
    if let Err(why) = ran {
        let note = format!("Lean Windows did not all happen: {why}. {AGAIN}");
        task.error = [task.error.as_str(), note.as_str()].join(" ").trim().to_owned();
    }
}

/// Lean Windows again, as the machine's settings or `guesthouse-vm lean` ask. This is the user's own
/// request, so what didn't happen is this task's failure.
pub(super) fn run_again(task: &mut Task) -> Result<()> {
    if !is_lean(task.context, &task.machine) {
        return Err(Failure::new("This machine was not made lean."));
    }
    if task.machine.installing {
        return Err(Failure::new("The install has not finished, and this runs after it."));
    }
    if task.pid().is_none() {
        return Err(Failure::new("Start the machine first. This runs in the guest."));
    }
    let ran = run(task);
    note_taken_out(task);
    ran.map_err(|why| Failure(format!("Lean Windows did not all happen: {why}.")))
}

#[cfg(test)]
#[path = "lean_tests.rs"]
mod tests;
