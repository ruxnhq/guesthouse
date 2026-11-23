//! A machine's snapshots from a terminal: what is there, taking one, going back to one, and deleting
//! one, each refused where the app's Snapshots page refuses it. Going back with no snapshot named is to
//! the one taken before a start that failed. What a start does by itself is in
//! [`crate::auto_snapshot`], and it holds here as it does for the app's Start, as both run the same task.

use super::run_task_with;
use crate::auto_snapshot::RestorePoint;
use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::machine::Machine;
use crate::snapshots::{Snapshot, list_snapshots};
use crate::tasks::{State, status};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use std::io::Write;

/// What a failed start tells, to say how to go back.
pub(super) fn offer(id: &str, point: &RestorePoint) -> String {
    let name = &point.name;
    if point.automatic {
        format!("Guesthouse took a snapshot before it, {name}. guesthouse-vm go-back {id} goes back to it.")
    } else {
        format!(
            "It started from the snapshot {name}, which it had gone back to. guesthouse-vm go-back {id} goes \
             back to it again."
        )
    }
}

fn when(taken: i64) -> String {
    Timestamp::from_second(taken).map_or_else(
        |_| String::new(),
        |time| time.to_zoned_datetime(TimeZone::system()).strftime("%-d %B %Y, %H:%M").to_string(),
    )
}

pub(super) fn list(context: &Context, machine: &Machine, out: &mut dyn Write) -> Result<()> {
    let found = list_snapshots(context, machine);
    if found.is_empty() {
        return Ok(writeln!(out, "{} has no snapshots.", machine.name)?);
    }
    for snapshot in found {
        let by = if snapshot.automatic { "  (taken by Guesthouse before a start)" } else { "" };
        writeln!(out, "{}: {} [{}]{by}", when(snapshot.taken), snapshot.name, snapshot.tag)?;
    }
    Ok(())
}

pub(super) fn go_back(context: &Context, machine: &Machine, yes: bool, out: &mut dyn Write) -> Result<()> {
    let found = status(context, machine);
    let Some(point) = found.restore_point else {
        return Err(Failure(format!(
            "{} has no start that failed after a snapshot to go back to.",
            machine.name
        )));
    };
    if found.state != State::Off || found.busy {
        return Err(Failure::new("The VM must be off to go back."));
    }
    if !yes {
        return Err(Failure(format!(
            "Everything done on {} since {} is lost: files, programs and changes to the guest. Other \
             snapshots stay. This cannot be undone. Run it again with --yes to go on.",
            machine.name, point.name
        )));
    }
    let ended = run_task_with(context, machine, "go_back", &[point.tag], true, out)?.unwrap_or_default();
    if ended.error.is_empty() {
        Ok(writeln!(out, "{} is back at {}.", machine.name, point.name)?)
    } else {
        Err(Failure(ended.error))
    }
}

const SHUT_DOWN_FIRST: &str = "Shut the VM down first.";
const NOT_INSTALLED: &str = "Its install has not finished.";
const ASK_AGAIN: &str = "Run it again with --yes to go on.";

/// Snapshots change only while the machine is off, with nothing at work on it, and installed, as on
/// the app's page. A throwaway's run is the machine running.
fn can_change(context: &Context, machine: &Machine) -> Result<()> {
    if machine.installing {
        return Err(Failure::new(NOT_INSTALLED));
    }
    let found = status(context, machine);
    if found.state != State::Off || found.busy {
        return Err(Failure::new(SHUT_DOWN_FIRST));
    }
    Ok(())
}

/// The snapshot by its tag, or by its name when only one has it.
fn named(context: &Context, machine: &Machine, said: &str) -> Result<Snapshot> {
    let all = list_snapshots(context, machine);
    if let Some(found) = all.iter().find(|snapshot| snapshot.tag == said) {
        return Ok(found.clone());
    }
    let mut called: Vec<Snapshot> = all.into_iter().filter(|snapshot| snapshot.name == said).collect();
    match called.len() {
        1 => Ok(called.remove(0)),
        0 => Err(Failure(format!("{} has no snapshot called {said}. snapshots lists them.", machine.name))),
        _ => Err(Failure(format!(
            "{} snapshots are called {said}. Give its tag, in brackets in snapshots.",
            called.len()
        ))),
    }
}

/// Runs a snapshot task to its end, and says what it did or why not.
fn run(
    context: &Context,
    machine: &Machine,
    action: &str,
    argument: &str,
    out: &mut dyn Write,
) -> Result<()> {
    let ended = run_task_with(context, machine, action, &[argument.to_owned()], true, out)?;
    match ended.map(|found| found.error).unwrap_or_default() {
        error if error.is_empty() => Ok(()),
        error => Err(Failure(error)),
    }
}

pub(super) fn take(context: &Context, machine: &Machine, name: &str, out: &mut dyn Write) -> Result<()> {
    can_change(context, machine)?;
    run(context, machine, "take_snapshot", name, out)?;
    Ok(writeln!(out, "{} has a snapshot called {}.", machine.name, name.trim())?)
}

pub(super) fn go_back_to(
    context: &Context,
    machine: &Machine,
    said: &str,
    yes: bool,
    out: &mut dyn Write,
) -> Result<()> {
    can_change(context, machine)?;
    let snapshot = named(context, machine, said)?;
    if !yes {
        return Err(Failure(format!(
            "Everything done on {} since {} is lost: files, programs and changes to the guest. Other \
             snapshots stay. This cannot be undone. {ASK_AGAIN}",
            machine.name,
            when(snapshot.taken)
        )));
    }
    run(context, machine, "go_back", &snapshot.tag, out)?;
    Ok(writeln!(out, "{} is back at {}.", machine.name, snapshot.name)?)
}

pub(super) fn delete(
    context: &Context,
    machine: &Machine,
    said: &str,
    yes: bool,
    out: &mut dyn Write,
) -> Result<()> {
    can_change(context, machine)?;
    let snapshot = named(context, machine, said)?;
    if !yes {
        return Err(Failure(format!(
            "You can no longer go back to {} once it is deleted. {} stays as it is now. {ASK_AGAIN}",
            snapshot.name, machine.name
        )));
    }
    run(context, machine, "delete_snapshot", &snapshot.tag, out)?;
    Ok(writeln!(out, "{} is deleted.", snapshot.name)?)
}
