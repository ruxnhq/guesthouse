//! Refreshing a throwaway's clean state: one start with its changes kept, so the guest's copy of
//! NVIDIA's driver, and the system folders' copies of its files, are brought up to this PC's driver
//! once, and later runs start from that. Without it, each run starts from a disk with the older driver
//! and copies the new one in again, with a restart of Windows.
//!
//! The machine stays a throwaway throughout. The disk, the firmware settings, the TPM and the notes
//! about the guest are its clean state, as [`crate::throwaway`] keeps them, and a snapshot of all of
//! them is taken first. The start then runs on the disk itself, as an install does, and Windows is
//! shut down. If anything fails, or the app goes before the end, the snapshot is gone back to, so the
//! old clean state stays.

use super::{STOP_WAIT_SECONDS, Task, force, start};
use crate::failure::{Failure, Result};
use crate::guest_driver_store::due;
use crate::log::write_log;
use crate::snapshots::{delete_tag, go_back, make_snapshot};
use crate::throwaway::{keep_notes, restore_notes};
use crate::vm_host::Stopped;
use std::fs;
use std::path::PathBuf;

/// What the app and `guesthouse-vm` ask the task program for.
pub const REFRESH_CLEAN_TASK: &str = "refresh_clean";
/// In the machine's folder while a refresh is under way: the tag of the snapshot taken before it.
const MARK_FILE: &str = "clean-refresh";
const SNAPSHOT_NAME: &str = "Before its clean state was refreshed";
const NOT_A_THROWAWAY: &str = "Only a throwaway has a clean state to refresh.";
const KEPT: &str = "The clean state stays as it was.";

fn mark_file(task: &Task) -> PathBuf {
    task.machine.folder(task.context).join(MARK_FILE)
}

/// Starts the machine once on its own disk, lets the start bring the driver up to date, and shuts
/// Windows down. That disk, with the notes the start left, is then the clean state.
pub(super) fn refresh_clean_state(task: &mut Task) -> Result<()> {
    if !task.machine.throwaway {
        return Err(Failure::new(NOT_A_THROWAWAY));
    }
    if task.pid().is_some() {
        return Err(Failure::new("Shut the VM down first."));
    }
    put_back_cut_short(task)?;
    let tag = make_snapshot(task, SNAPSHOT_NAME, true)?;
    fs::write(mark_file(task), &tag)?;
    let kept = task.machine.clone();
    let result = run_on_its_own_disk(task);
    task.machine = kept;
    match result {
        Ok(()) => {
            keep_notes(&task.machine.folder(task.context))?;
            write_log(task.context, &format!("{}: its clean state was refreshed", task.machine.id))?;
            finish(task, &tag)
        }
        Err(failure) => {
            let _ = write_log(
                task.context,
                &format!("{}: refreshing its clean state: {}", task.machine.id, failure.0),
            );
            put_back(task, &tag)?;
            Err(Failure(format!("{} {KEPT}", failure.0)))
        }
    }
}

/// The start, as one of an ordinary machine: on the disk itself, and without a snapshot of its own,
/// as the one taken before stands for it.
fn run_on_its_own_disk(task: &mut Task) -> Result<()> {
    // The notes beside the settings are the last run's, and the start goes by what the disk has.
    restore_notes(task.context, &task.machine)?;
    task.machine.throwaway = false;
    task.machine.auto_snapshot = Some(false);
    start(task)?;
    if task.pid().is_none() {
        return Err(Failure::new("The VM went before its start was done."));
    }
    if due(task.context, &task.machine).is_some() {
        let said =
            if task.error.is_empty() { "the driver did not reach Windows" } else { task.error.as_str() };
        return Err(Failure(format!("NVIDIA's driver in Windows was not brought up to date: {said}")));
    }
    task.step("Shutting down Windows");
    if task.host.stop(&task.machine, STOP_WAIT_SECONDS)? == Stopped::TurnedOff {
        return Err(Failure::new("Windows did not shut down in time, so the VM was turned off."));
    }
    Ok(())
}

fn finish(task: &mut Task, tag: &str) -> Result<()> {
    delete_tag(task.context, &task.machine, tag)?;
    let _ = fs::remove_file(mark_file(task));
    Ok(())
}

/// Back to the snapshot taken before, with the VM off first.
fn put_back(task: &mut Task, tag: &str) -> Result<()> {
    if task.pid().is_some() {
        force(task, None)?;
    }
    task.step("Putting the clean state back");
    go_back(task, tag)?;
    finish(task, tag)
}

/// A refresh that the app or the PC cut short left the disk half changed and its mark behind: the
/// snapshot it took goes back first. A throwaway's start does this too, so no run starts from it.
pub(super) fn put_back_cut_short(task: &mut Task) -> Result<()> {
    let Ok(tag) = fs::read_to_string(mark_file(task)) else { return Ok(()) };
    let tag = tag.trim().to_owned();
    write_log(
        task.context,
        &format!("{}: a refresh of its clean state was cut short, so it goes back", task.machine.id),
    )?;
    put_back(task, &tag)
}

#[cfg(test)]
#[path = "clean_state_tests.rs"]
mod tests;
