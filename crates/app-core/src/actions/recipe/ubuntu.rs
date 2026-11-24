//! A recipe's steps in an Ubuntu guest, over SSH as root. Every command is a fixed text from here, and
//! the steps are data on stdin: `linux/recipe/run.sh` goes in as a file and is run with a word, and the
//! job it reads is a file of lines, so a package name or a script of a recipe is never read by a shell as
//! anything but a name or a file to run.

use super::{Stopped, encoded, numbered_steps, show};
use crate::actions::Task;
use crate::guest_shell::ShellAnswer;
use crate::linux::guest_sources;
use crate::recipe::Chosen;
use std::fs;
use std::time::Duration;

const RUNNER_FILE: &str = "recipe/run.sh";
const FOLDER: &str = "/var/lib/guesthouse-recipe";
pub(super) const PUT_RUNNER: &str =
    "umask 077 && mkdir -p /var/lib/guesthouse-recipe && cat > /var/lib/guesthouse-recipe/run.sh";
pub(super) const PUT_JOB: &str =
    "umask 077 && mkdir -p /var/lib/guesthouse-recipe && cat > /var/lib/guesthouse-recipe/job";
pub(super) const READY: &str = "sh /var/lib/guesthouse-recipe/run.sh ready";
pub(super) const PREPARE: &str = "sh /var/lib/guesthouse-recipe/run.sh prepare";
const QUICK_SECONDS: u64 = 60;
/// The first boot builds a desktop and a driver and then restarts, and the guest is not up for steps
/// until it has.
const UP_MINUTES: u64 = 90;
const UP_POLL: Duration = Duration::from_secs(10);
/// A step that has not ended in this long is taken as stuck. Its guest still has it, and the next run
/// finds it done or not.
const STEP_SECONDS: u64 = 3600;
const WAITING: &str = "Waiting for Ubuntu to finish its first boot";
const COPYING: &str = "Copying the recipe to the guest";
const UPDATING: &str = "Updating the package lists";

fn answer_text(answer: &ShellAnswer) -> String {
    match answer {
        ShellAnswer::Unreachable(said) => format!("The guest could not be reached. {said}"),
        ShellAnswer::TimedOut => "It did not finish in an hour.".to_owned(),
        ShellAnswer::Ran { status, output } if output.trim().is_empty() => {
            format!("It stopped with status {status}.")
        }
        ShellAnswer::Ran { output, .. } => output.clone(),
    }
}

fn succeeded(answer: &ShellAnswer) -> bool {
    matches!(answer, ShellAnswer::Ran { status: 0, .. })
}

/// Asks until the guest answers with success, or its time is up. SSH not being there yet, and a first
/// boot not being over, are both the same to the host: not yet.
fn until(task: &Task, what: &str, command: &str, input: &[u8], end: Duration) -> Result<(), Stopped> {
    loop {
        if task.pid().is_none() {
            return Err(Stopped::new(what, "The machine is not running."));
        }
        let answer = task.context.ask_shell(&task.machine, command, input, QUICK_SECONDS);
        if succeeded(&answer) {
            return Ok(());
        }
        if task.now() >= end {
            return Err(Stopped::new(
                what,
                format!("The guest was not ready in {UP_MINUTES} minutes. {}", answer_text(&answer)),
            ));
        }
        task.sleep(UP_POLL);
    }
}

/// The job: a line for each step, with a package's name as it is and a script in Base64.
fn job(chosen: &Chosen) -> String {
    let mut text = String::new();
    for (number, kind, value) in numbered_steps(chosen) {
        let line = match kind {
            "package" => format!("step {number} apt {value}\n"),
            _ => format!("step {number} script {}\n", encoded(value)),
        };
        text.push_str(&line);
    }
    text
}

fn ask_step(task: &Task, what: &str, command: &str, seconds: u64) -> Result<(), Stopped> {
    let answer = task.context.ask_shell(&task.machine, command, b"", seconds);
    if succeeded(&answer) { Ok(()) } else { Err(Stopped::new(what, answer_text(&answer))) }
}

pub(super) fn run(task: &mut Task, chosen: &Chosen) -> Result<(), Stopped> {
    let names = chosen.recipe.step_names();
    if !task.machine.has_ssh_key(task.context) {
        return Err(Stopped::new(
            COPYING,
            "This machine has no SSH key of its own, so nothing can be run in it.",
        ));
    }
    let runner = fs::read(guest_sources(task.context).join(RUNNER_FILE))
        .map_err(|error| Stopped::new(COPYING, format!("The recipe's runner is missing: {error}")))?;
    let end = task.now() + Duration::from_secs(UP_MINUTES * 60);
    task.step(WAITING);
    until(task, WAITING, PUT_RUNNER, &runner, end)?;
    until(task, WAITING, READY, b"", end)?;
    task.step(COPYING);
    let put = task.context.ask_shell(&task.machine, PUT_JOB, job(chosen).as_bytes(), QUICK_SECONDS);
    if !succeeded(&put) {
        return Err(Stopped::new(COPYING, answer_text(&put)));
    }
    if !chosen.recipe.steps.packages.is_empty() {
        task.step(UPDATING);
        ask_step(task, UPDATING, PREPARE, STEP_SECONDS)?;
    }
    for (number, _, _) in numbered_steps(chosen) {
        let name = &names[number - 1];
        show(task, chosen, number, name);
        ask_step(task, name, &format!("sh {FOLDER}/run.sh run {number}"), STEP_SECONDS)?;
    }
    Ok(())
}
