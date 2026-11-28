//! Moves an older Windows machine's disk from the emulated SATA controller to NVMe, which is faster
//! and which Windows has its own driver for. Windows installed on SATA keeps its NVMe driver from
//! loading at boot, by a `StartOverride` key it puts back at every boot from a disk that isn't NVMe.
//! So the move happens in one start: once the desktop is up, the guest's script deletes the key,
//! Windows shuts down cleanly, and the same start goes on in a new VM with the disk on NVMe. Nothing
//! boots Windows in between, so the key can't come back.
//!
//! The start on NVMe is a trial. If Windows comes up, the machine stays on NVMe. If it doesn't, the
//! machine goes back to SATA for good and the start goes on there, so a move can't leave a machine
//! that won't start. The script writes the deleted key to the disk at once, so a Windows that has to
//! be turned off instead of shutting down moves too. Back on SATA, Windows puts the key back by itself. Two files in the machine's
//! folder keep this if the app stops halfway: one while the trial is due, and one that says why a
//! machine stays on SATA, so the move isn't tried at every start.

use super::Task;
use crate::agent::guest_script;
use crate::context::Context;
use crate::failure::Result;
use crate::log::write_log;
use crate::machine::{Kind, Machine};
use std::fs;
use std::path::PathBuf;

const SCRIPT_NAME: &str = "nvme-ready.ps1";
const SCRIPT_SECONDS: u64 = 30;
/// What the script says once Windows will load the NVMe driver at its next boot.
const READY_MARK: &str = "ready";
/// The machine is on NVMe and hasn't started there yet.
const TRIAL_FILE: &str = "nvme-trial";
/// The machine stays on SATA, and this says why.
const REFUSED_FILE: &str = "nvme-refused";
pub(super) const MOVING: &str = "Moving the disk to NVMe";
pub(super) const BACK_ON_SATA: &str =
    "Windows did not start from the NVMe disk, so this machine is back on SATA.";

fn trial_file(context: &Context, machine: &Machine) -> PathBuf {
    machine.folder(context).join(TRIAL_FILE)
}

fn refused_file(context: &Context, machine: &Machine) -> PathBuf {
    machine.folder(context).join(REFUSED_FILE)
}

/// The two files that say where the disk stands in a move, for a snapshot to keep.
pub(crate) fn state_files(context: &Context, machine: &Machine) -> [PathBuf; 2] {
    [trial_file(context, machine), refused_file(context, machine)]
}

/// Whether this start is the machine's first on NVMe since it was moved there.
pub(super) fn on_trial(context: &Context, machine: &Machine) -> bool {
    machine.nvme && trial_file(context, machine).is_file()
}

/// Windows has come up on NVMe, so the machine stays there.
pub(super) fn passed(task: &Task) {
    if on_trial(task.context, &task.machine) {
        let _ = fs::remove_file(trial_file(task.context, &task.machine));
        let _ = write_log(task.context, "nvme: Windows started from the NVMe disk");
    }
}

/// The machine goes back to SATA, where it started before.
fn back_to_sata(task: &mut Task) -> Result<()> {
    task.machine.nvme = false;
    task.machine.save_disk_place(task.context)?;
    let _ = fs::remove_file(trial_file(task.context, &task.machine));
    Ok(())
}

/// Windows didn't come up on its first start on NVMe, for `why`. The machine goes back to SATA and
/// stays there.
pub(super) fn give_up(task: &mut Task, why: &str) -> Result<()> {
    back_to_sata(task)?;
    fs::write(refused_file(task.context, &task.machine), format!("{why}\n"))?;
    write_log(task.context, &format!("nvme: back on SATA, as Windows did not start from NVMe: {why}"))?;
    Ok(())
}

/// Whether this machine is one to move: a Windows guest still on SATA, and not one that went back.
fn due(context: &Context, machine: &Machine) -> bool {
    machine.kind == Kind::Windows
        && !machine.nvme
        && !machine.installing
        && !refused_file(context, machine).exists()
}

/// What of the move a start will find to do, if anything: `trial` for a machine already on NVMe that
/// has not started there, `move` for one that is due and has the script to be readied by.
pub(crate) fn pending(context: &Context, machine: &Machine) -> Option<&'static str> {
    if on_trial(context, machine) {
        return Some("trial");
    }
    (due(context, machine) && guest_script(context, SCRIPT_NAME).is_ok()).then_some("move")
}

/// Readies Windows for NVMe if this machine is one to move, and says whether it was. The machine is
/// then on NVMe, and the VM has to be stopped and started again now, before anything boots Windows
/// on SATA. Anything that goes wrong is logged, the machine stays on SATA, and the next start tries
/// again.
pub(super) fn ready_windows(task: &mut Task) -> Result<bool> {
    // What readies Windows would go with the run, and the disk would move without it. A throwaway
    // moves once it's an ordinary machine again.
    if task.machine.throwaway || !due(task.context, &task.machine) {
        return Ok(false);
    }
    let script = match guest_script(task.context, SCRIPT_NAME) {
        Ok(script) => script,
        Err(failure) => {
            write_log(task.context, &format!("nvme: {}", failure.0))?;
            return Ok(false);
        }
    };
    let said = task.ask_guest(&script, SCRIPT_SECONDS);
    if said.trim() != READY_MARK {
        write_log(task.context, &format!("nvme: Windows was not readied: {}", said.trim()))?;
        return Ok(false);
    }
    fs::write(trial_file(task.context, &task.machine), "")?;
    task.machine.nvme = true;
    task.machine.save_disk_place(task.context)?;
    write_log(task.context, "nvme: Windows is ready, so the VM starts again on NVMe")?;
    Ok(true)
}

#[cfg(test)]
#[path = "nvme_move_tests.rs"]
mod tests;
