//! A snapshot before a start that may change Windows in a way it cannot come back from. A snapshot
//! needs the machine off, so it is taken as a start begins, once the start is known to go ahead, and
//! only when [`risks::pending`] finds a change due that no successful start has met yet. If that start
//! then fails, the machine offers going back to it ([`restore_point`]), and going back stays the
//! user's choice.
//!
//! There is one of these per machine, and a new one replaces it. Snapshots the user takes are never
//! touched: this only ever deletes ones its own record marks as automatic.
//!
//! None of this may stop a start that would have worked. A snapshot that cannot be taken is said and
//! the start goes on.

pub mod record;
pub mod risks;

use crate::actions::Task;
use crate::context::Context;
use crate::disk_image::disk_info;
use crate::failure::Result;
use crate::log::write_log;
use crate::machine::{Kind, Machine};
use crate::snapshots::{delete_tag, list_snapshots, make_snapshot};
use risks::{Pending, Risk};

/// Whether a Windows machine takes these snapshots until its user says otherwise. The one place to
/// change it: a machine's own setting, once made, overrides it.
pub const AUTO_SNAPSHOT_DEFAULT_ON: bool = false;

const NAME_START: &str = "Automatic: before ";
const SEVERAL_CHANGES: &str = "changes";
/// A disk the snapshot code refuses is told once in the settings, and a start does not nag about it.
const DISK_CANNOT: &str = "This disk can't hold snapshots, as only a qcow2 disk can.";
pub const STEP: &str = "Saving a snapshot first";

/// What a start remembers of this, to finish it once it has succeeded.
#[derive(Debug, Clone, Default)]
pub struct AutoStart {
    /// What a successful start has seen through. Empty when no snapshot stands behind this start.
    pub covers: Vec<String>,
    /// What the user is told if a snapshot was wanted and could not be taken.
    pub note: String,
}

/// The name that says what the snapshot is before. When it was taken is the snapshot's own date, which
/// the lists show, so the name does not say it too.
fn name_for(risks: &[&Pending]) -> String {
    let mut kinds: Vec<Risk> = risks.iter().map(|pending| pending.risk).collect();
    kinds.dedup();
    let what = match kinds[..] {
        [one] => one.words(),
        _ => SEVERAL_CHANGES,
    };
    format!("{NAME_START}{what}").chars().take(crate::snapshots::MAX_SNAPSHOT_NAME).collect()
}

/// Whether this machine's disk can hold the snapshot, as qcow2 does and nothing else does.
pub fn disk_can_hold(machine: &Machine) -> bool {
    disk_info(&machine.disk).is_ok_and(|info| info["format"] == "qcow2")
}

/// Whether the machine takes automatic snapshots at all: the kinds with a start that changes Windows.
pub fn applies_to(machine: &Machine) -> bool {
    machine.kind == Kind::Windows && !machine.installing
}

/// The snapshot, if one is due. Called as a start begins, with nothing running.
pub fn before_start(task: &mut Task) {
    let machine = task.machine.clone();
    if !applies_to(&machine) || !machine.auto_snapshot_on() {
        return;
    }
    let context = task.context;
    let mut record = record::read(context, &machine);
    let all = risks::pending(context, &machine);
    let due: Vec<&Pending> =
        all.iter().filter(|pending| !record.covered.contains(&pending.signature)).collect();
    if due.is_empty() {
        return;
    }
    let signatures: Vec<String> = all.iter().map(|pending| pending.signature.clone()).collect();
    if record.pending {
        // The start before this one has not finished. The snapshot behind it is the last disk that
        // worked, and one taken now would be of what that start left.
        task.auto.covers = signatures;
        return;
    }
    if !disk_can_hold(&machine) {
        let _ = write_log(context, &format!("auto snapshot: {DISK_CANNOT}"));
        task.auto.covers = signatures;
        return;
    }
    task.step(STEP);
    let name = name_for(&due);
    match make_snapshot(task, &name, true) {
        Ok(tag) => {
            let _ = delete_all_but(task.context, &task.machine, &tag);
            record = record::Record { tag, name, pending: true, covered: record.covered };
            if let Err(failure) = record::write(context, &machine, &record) {
                let _ =
                    write_log(context, &format!("auto snapshot: could not write its record: {}", failure.0));
                return;
            }
            task.auto.covers = signatures;
        }
        Err(failure) => {
            let _ = write_log(context, &format!("auto snapshot: not taken: {}", failure.0));
            task.auto.note = format!("No snapshot was taken before this start: {}", failure.0);
        }
    }
}

/// Deletes the automatic snapshots but `keep`, and never any other. The first failure stops it.
fn delete_all_but(context: &Context, machine: &Machine, keep: &str) -> Result<()> {
    let older: Vec<String> = list_snapshots(context, machine)
        .into_iter()
        .filter(|snapshot| snapshot.automatic && snapshot.tag != keep)
        .map(|snapshot| snapshot.tag)
        .collect();
    for tag in older {
        delete_tag(context, machine, &tag).inspect_err(|failure| {
            // Two stay until the next replacement clears the older one.
            let _ = write_log(context, &format!("auto snapshot: the old one stays: {}", failure.0));
        })?;
    }
    Ok(())
}

/// Every automatic snapshot of the machine goes, as when its entry is removed.
pub fn delete_all(context: &Context, machine: &Machine) -> Result<()> {
    delete_all_but(context, machine, "")
}

/// The start finished, so the disk has been lived on: there is nothing to go back to for it, and the
/// changes it met are met. That holds whatever the start had due, as the change that failed a start
/// can be gone by the next one, and a user can have turned the switch off between.
pub fn start_succeeded(task: &Task) {
    let mut record = record::read(task.context, &task.machine);
    if !record.pending && task.auto.covers.is_empty() {
        return;
    }
    record.pending = false;
    for signature in &task.auto.covers {
        if !record.covered.contains(signature) {
            record.covered.push(signature.clone());
        }
    }
    if let Err(failure) = record::write(task.context, &task.machine, &record) {
        let _ = write_log(task.context, &format!("auto snapshot: could not write its record: {}", failure.0));
    }
}

/// What a failed start can offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestorePoint {
    pub tag: String,
    pub name: String,
}

/// The snapshot to offer a machine that is off after a start that did not succeed.
pub fn restore_point(context: &Context, machine: &Machine) -> Option<RestorePoint> {
    if !applies_to(machine) {
        return None;
    }
    let record = record::read(context, machine);
    (record.pending && !record.tag.is_empty()).then_some(RestorePoint { tag: record.tag, name: record.name })
}

#[cfg(test)]
mod tests;
