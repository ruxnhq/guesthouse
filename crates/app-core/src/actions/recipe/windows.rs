//! A recipe's steps in a Windows guest. The agent runs as SYSTEM and takes one request at a time, and
//! winget is a program of the user's, so a short script through the agent starts the steps in a task
//! of the account's own, and the host reads a status file the task writes, a quick ask at a time. A
//! step that takes twenty minutes never holds the agent.

use super::{Stopped, encoded, numbered_steps, show};
use crate::actions::Task;
use crate::agent::{QUICK_SECONDS, guest_script};
use crate::recipe::Chosen;
use serde_json::json;
use std::time::Duration;

const START_SCRIPT: &str = "recipe-start.ps1";
const RUN_SCRIPT: &str = "recipe-run.ps1";
/// The status file the steps write, one line at a time.
const STATUS_ASK: &str =
    "Get-Content -LiteralPath C:\\gpupv\\recipe\\status.txt -Raw -ErrorAction SilentlyContinue";
const UP_ASK: &str = "'up'";
const UP_MARK: &str = "up";
const START_MARK: &str = "started";
const NOBODY: &str = "nobody is logged in";
const START_SECONDS: u64 = 60;
/// How long the guest's agent gets to answer, as one just booted has not started it yet.
const AGENT_MINUTES: u64 = 5;
const AGENT_POLL: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_secs(5);
/// How long the steps wait for winget to be there, which is up to a new Windows 11 to be quick about.
pub(super) const WINGET_SECONDS: u64 = 600;
/// A step, such as a big installer, that has said nothing for this long is taken as stuck.
const STEP_MINUTES: u64 = 60;
const STARTING: &str = "Starting the steps";
const WAITING: &str = "Waiting for winget";

/// What the status file says so far.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Progress {
    /// The step that has started and not ended, by its number.
    pub running: Option<usize>,
    pub done: usize,
    pub failed: Option<(usize, String)>,
    pub waiting_for_winget: bool,
    pub ended: bool,
}

/// Reads the status lines. A line that isn't one of the steps' own words is skipped, as is one cut
/// short by a read in the middle of a write.
pub(super) fn read_progress(text: &str) -> Progress {
    let mut progress = Progress::default();
    for line in text.lines().map(str::trim) {
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        let (number, why) = rest.split_once(' ').unwrap_or((rest, ""));
        match (word, number.parse::<usize>()) {
            ("waiting", _) => progress.waiting_for_winget = true,
            ("start", Ok(number)) => {
                progress.running = Some(number);
                progress.waiting_for_winget = false;
            }
            ("ok", Ok(_)) => {
                progress.done += 1;
                progress.running = None;
            }
            ("fail", Ok(number)) => progress.failed = Some((number, why.trim().to_owned())),
            ("end", _) => progress.ended = true,
            _ => {}
        }
    }
    progress
}

/// What the guest is sent to start the steps: the recipe's steps as JSON, and the runner. Both are
/// Base64, which has nothing in it a script could read as anything but text to decode.
fn start_script(task: &Task, chosen: &Chosen) -> crate::failure::Result<String> {
    let steps: Vec<_> = numbered_steps(chosen)
        .into_iter()
        .map(|(number, kind, value)| json!({ "number": number, "kind": kind, "value": value }))
        .collect();
    let data = json!({ "winget_seconds": WINGET_SECONDS, "steps": steps }).to_string();
    let runner = guest_script(task.context, RUN_SCRIPT)?;
    let start = guest_script(task.context, START_SCRIPT)?;
    Ok(format!("$data = '{}'\n$runner = '{}'\n{start}", encoded(&data), encoded(&runner)))
}

fn wait_for_agent(task: &Task) -> Result<(), Stopped> {
    let end = task.now() + Duration::from_secs(AGENT_MINUTES * 60);
    while !task.ask_guest(UP_ASK, QUICK_SECONDS).contains(UP_MARK) {
        if task.pid().is_none() {
            return Err(Stopped::new(STARTING, "The machine is not running."));
        }
        if task.now() >= end {
            return Err(Stopped::new(STARTING, "The guest's agent did not answer."));
        }
        task.sleep(AGENT_POLL);
    }
    Ok(())
}

pub(super) fn run(task: &mut Task, chosen: &Chosen) -> Result<(), Stopped> {
    let names = chosen.recipe.step_names();
    wait_for_agent(task)?;
    let script = start_script(task, chosen).map_err(|failure| Stopped::new(STARTING, failure.0))?;
    let started = task.ask_guest(&script, START_SECONDS);
    if started.contains(NOBODY) {
        return Err(Stopped::new(STARTING, "Nobody is logged in to Windows yet."));
    }
    if !started.contains(START_MARK) {
        let said = if started.trim().is_empty() { "the agent went quiet" } else { started.trim() };
        return Err(Stopped::new(STARTING, said));
    }
    let limit = Duration::from_secs(STEP_MINUTES * 60);
    let (mut shown, mut changed, mut said_waiting) = (None, task.now(), false);
    loop {
        task.sleep(POLL);
        if task.pid().is_none() {
            return Err(Stopped::new(
                current(&names, shown),
                "The machine stopped while the steps were running.",
            ));
        }
        let progress = read_progress(&task.ask_guest(STATUS_ASK, QUICK_SECONDS));
        if let Some((number, why)) = &progress.failed {
            let name = names.get(number.saturating_sub(1)).map_or(STARTING, String::as_str);
            return Err(Stopped::new(name, why.as_str()));
        }
        if progress.ended {
            return if progress.done == names.len() {
                Ok(())
            } else {
                Err(Stopped::new(current(&names, shown), "The steps ended before they were all done."))
            };
        }
        if let Some(number) = progress.running.filter(|number| Some(*number) != shown) {
            show(task, chosen, number, &names[number.saturating_sub(1).min(names.len() - 1)]);
            (shown, changed) = (Some(number), task.now());
        } else if progress.waiting_for_winget && !said_waiting {
            task.step(WAITING);
            said_waiting = true;
        }
        if task.now().saturating_sub(changed) > limit {
            return Err(Stopped::new(
                current(&names, shown),
                format!("It did not finish in {STEP_MINUTES} minutes."),
            ));
        }
    }
}

fn current(names: &[String], shown: Option<usize>) -> &str {
    shown.and_then(|number| names.get(number.saturating_sub(1))).map_or(STARTING, String::as_str)
}
