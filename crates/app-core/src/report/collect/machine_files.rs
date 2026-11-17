//! The machine a report is about: which one that is, and the files the app keeps in its folder.
//!
//! Only the files the app writes there go in, never the disk, its thumbnail, the agent's token or
//! the guest's password. `machine.json` loses the fields that are the user's own before it goes.

use super::Collection;
use crate::context::Context;
use crate::machine::{Machine, load_all};
use crate::slots;
use crate::tasks::read_task;
use crate::vm_host::MACHINE_NOTE;
use crate::{display, gpu_identity, linux};
use guesthouse_daemon::slot::Slot;
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::time::SystemTime;

const MACHINE_FILE: &str = "machine.json";
/// Fields that are the user's own and say nothing about a bug: names they chose, the guest account,
/// the network card's address, the host's drives by serial number, and a folder of theirs.
const PRIVATE_FIELDS: [&str; 5] = ["name", "user", "mac", "drives", "share"];
const LEFT_OUT: &str = "(left out)";

/// The machine with this name or id.
pub fn chosen_machine(context: &Context, wanted: &str) -> Option<Machine> {
    let (machines, _) = load_all(context);
    machines.into_iter().find(|machine| machine.id == wanted || machine.name == wanted)
}

/// The machine that ran last, going by the newest run folder that names one.
pub fn latest_machine(context: &Context) -> Option<Machine> {
    let (machines, _) = load_all(context);
    let newest = slots::all()
        .filter_map(|slot| {
            let folder = context.run_folder(Some(slot));
            let id = machine_of_folder(context, &folder, slot)?;
            Some((last_change(&folder)?, id))
        })
        .max()
        .map(|(_, id)| id)?;
    machines.into_iter().find(|machine| machine.id == newest)
}

/// Which machine a run folder's logs are from: its note, or else, for a folder from before there
/// was a note, the machine its last task was for.
pub fn machine_of_folder(context: &Context, folder: &Path, slot: Slot) -> Option<String> {
    let noted = fs::read_to_string(folder.join(MACHINE_NOTE)).ok().map(|text| text.trim().to_owned());
    noted.filter(|id| !id.is_empty()).or_else(|| {
        let task = read_task(context, Some(slot));
        (!task.machine.is_empty()).then_some(task.machine)
    })
}

/// When anything in a folder last changed.
fn last_change(folder: &Path) -> Option<SystemTime> {
    let entries = fs::read_dir(folder).ok()?;
    entries.flatten().filter_map(|entry| entry.metadata().ok()?.modified().ok()).max()
}

pub fn collect(context: &Context, machine: &Machine, found: &mut Collection) {
    let folder = machine.folder(context);
    match fs::read_to_string(folder.join(MACHINE_FILE)) {
        Ok(text) => found.add("machine/machine.json", without_private_fields(&text)),
        Err(error) => found.miss("machine/machine.json", &format!("it could not be read: {error}"), None),
    }
    found.add_file_if_there("machine/display.json", &display::state_file(context, machine));
    found.add_file_if_there("machine/gpu-identity.json", &gpu_identity::state_file(context, machine));
    found.add_file_if_there("machine/amd-fallback", &linux::amd::fallback_file(context, machine));
}

/// The settings with each private field marked as left out, so it's still clear whether it was set.
fn without_private_fields(text: &str) -> String {
    let Ok(Value::Object(mut fields)) = serde_json::from_str::<Value>(text) else {
        return "(machine.json could not be read as JSON, so it was left out)\n".to_owned();
    };
    for name in PRIVATE_FIELDS {
        if let Some(value) = fields.get_mut(name) {
            *value = match value {
                Value::Array(list) => Value::String(format!("{LEFT_OUT}: {} of them", list.len())),
                Value::String(text) if text.is_empty() => continue,
                _ => Value::String(LEFT_OUT.to_owned()),
            };
        }
    }
    serde_json::to_string_pretty(&Value::Object(fields)).unwrap_or_default() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_users_own_fields_are_left_out() {
        let text = r#"{"name": "Quillon's Games", "disk": "/d", "user": "quill", "mac": "52:54:00:aa:bb:cc",
                       "drives": [{"id": "ata-DISK_SERIAL1"}, {"id": "ata-DISK_SERIAL2"}], "share": "",
                       "cores": 8}"#;
        let cleaned: Value = serde_json::from_str(&without_private_fields(text)).unwrap();
        assert_eq!(cleaned["name"], LEFT_OUT);
        assert_eq!(cleaned["user"], LEFT_OUT);
        assert_eq!(cleaned["mac"], LEFT_OUT);
        assert_eq!(cleaned["drives"], "(left out): 2 of them");
        assert_eq!(cleaned["share"], "", "an empty field still says it isn't set");
        assert_eq!(cleaned["cores"], 8);
        assert_eq!(cleaned["disk"], "/d");
    }
}
