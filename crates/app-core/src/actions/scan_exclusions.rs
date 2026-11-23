//! Folders left out of Defender's real-time scanning, put in place near the end of the install and
//! taken back when the user asks. Both go through the agent as SYSTEM, with the folders as data.

use super::Task;
use crate::agent::guest_script;
use crate::failure::{Failure, Result};
use crate::scan_exclusions::{guest_data, mark_applied, read_exclusions, write_exclusions};
use crate::text::base64_encode;

const SCRIPT: &str = "scan-exclusions.ps1";
/// Defender's own setting takes a moment for each folder.
const SECONDS: u64 = 60;
const DONE: &str = "done ";
const ADD: &str = "add";
const REMOVE: &str = "remove";
pub const STEP: &str = "Leaving folders out of virus scanning";

fn change(task: &Task, folders: &[String], what: &str) -> std::result::Result<(), String> {
    let script = guest_script(task.context, SCRIPT).map_err(|failure| failure.0)?;
    let data = base64_encode(guest_data(folders).as_bytes());
    let said = task.ask_guest(&format!("$data = '{data}'\n$change = '{what}'\n{script}"), SECONDS);
    if said.trim() == format!("{DONE}{}", folders.len()) {
        return Ok(());
    }
    let said = said.trim();
    Err(if said.is_empty() { "the agent went quiet".to_owned() } else { super::recipe::one_line(said) })
}

/// Part of an install, once its desktop has been checked. A machine made without exclusions isn't
/// touched, and one where Defender didn't take them is installed all the same, with a note: it is then
/// scanned everywhere, which is the safe way round.
pub(super) fn after_install(task: &mut Task) {
    let folders = read_exclusions(task.context, &task.machine);
    if folders.is_empty() {
        return;
    }
    task.step(STEP);
    let note = match change(task, &folders, ADD) {
        Ok(()) => match mark_applied(task.context, &task.machine) {
            Ok(()) => return,
            Err(failure) => format!(
                "Defender leaves out {}, and that could not be noted: {}",
                folders.join(", "),
                failure.0
            ),
        },
        Err(why) => format!("Defender still scans every folder: {why}."),
    };
    task.error = [task.error.as_str(), note.as_str()].join(" ").trim().to_owned();
}

/// Has Defender scan the folders again, and forgets them only once it does.
pub(super) fn scan_all(task: &mut Task) -> Result<()> {
    let folders = read_exclusions(task.context, &task.machine);
    if folders.is_empty() {
        return Err(Failure::new("Defender already scans every folder in this machine."));
    }
    if task.pid().is_none() {
        return Err(Failure::new("Start the machine first. Defender's setting is in the guest."));
    }
    task.step("Having Defender scan every folder again");
    change(task, &folders, REMOVE)
        .map_err(|why| Failure(format!("Defender's setting is as it was: {why}.")))?;
    write_exclusions(task.context, &task.machine, &[])
}

#[cfg(test)]
#[path = "scan_exclusions_tests.rs"]
mod tests;
