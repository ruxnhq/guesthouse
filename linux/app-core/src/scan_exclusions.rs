//! Folders that Microsoft Defender's real-time scanning leaves out, in a Windows guest. A recipe can
//! name them, such as a games library, but only the person making the machine turns them on, and it is
//! off unless they do. Defender stays on for everything else, and Windows Security stays as it was.
//!
//! It is the one choice here that makes a guest less safe, so it is kept beside the machine for as long
//! as it holds, and the machine's settings and `guesthouse-vm show` say so. Taking it back is a button,
//! and `Remove-MpPreference` in the guest. It is Defender's own documented setting. Microsoft's pages
//! say Tamper Protection guards exclusions only on a device an organization manages, which a guest is
//! not, so it should take them with Tamper Protection on. No guest has shown that yet.

use crate::context::Context;
use crate::failure::Failure;
use crate::failure::Result;
use crate::machine::Machine;
use crate::settings::write_json;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Beside `machine.json`. A machine without exclusions has none.
pub const EXCLUSIONS_FILE: &str = "scan-exclusions.json";
/// Beside it when the app can't know what Defender leaves out in the guest, as after an import or going
/// back to a snapshot: the next start asks the guest, and the record says what it found.
pub const LOOK_MARK: &str = "scan-exclusions-look";
/// The rules for which folders may be left out, the words for what it means, and the folders as the
/// guest's script is given them and as it says them back, are in the crate every host can share.
pub use guesthouse_guest_names_core::linux_host::scan_exclusions::{
    MOST, WHAT_IT_MEANS, folder_problem, guest_data, list_problem, parse_found, what_it_does,
};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Kept {
    folders: Vec<String>,
    /// Defender took them. Until then, and if it didn't, every folder is scanned.
    #[serde(default)]
    applied: bool,
    /// They are what the guest said Defender leaves out, at a start that asked it.
    #[serde(default)]
    found: bool,
}

fn exclusions_file(context: &Context, machine: &Machine) -> PathBuf {
    machine.folder(context).join(EXCLUSIONS_FILE)
}

fn read_kept(context: &Context, machine: &Machine) -> Kept {
    let text = fs::read_to_string(exclusions_file(context, machine)).unwrap_or_default();
    serde_json::from_str(&text).unwrap_or_default()
}

/// The folders this machine was made to leave out of scanning, whether or not Defender has them yet.
/// The app writes the file and nothing else does, so one that can't be read is taken as none.
pub fn read_exclusions(context: &Context, machine: &Machine) -> Vec<String> {
    read_kept(context, machine).folders
}

/// Notes that Defender took the folders.
pub fn mark_applied(context: &Context, machine: &Machine) -> Result<()> {
    let kept = read_kept(context, machine);
    if kept.folders.is_empty() {
        return Err(Failure::new("The machine has no folders to leave out of scanning."));
    }
    write_json(&exclusions_file(context, machine), &Kept { applied: true, ..kept })
}

/// What is true of Defender's scanning in this machine, as far as Guesthouse knows, for its settings
/// and `show`. Nothing for a machine where Guesthouse left no folder out.
pub fn state_text(context: &Context, machine: &Machine) -> Option<String> {
    let kept = read_kept(context, machine);
    if kept.folders.is_empty() {
        return None;
    }
    let folders = kept.folders.join(", ");
    Some(if kept.found {
        format!("Defender in this guest doesn't scan {folders}, as the guest said at a start.")
    } else if kept.applied {
        format!("Defender doesn't scan {folders}. Everything else is scanned.")
    } else if machine.installing {
        format!("Defender is to leave out {folders} once Windows is installed.")
    } else {
        format!("Defender was to leave out {folders}, but didn't take them, so every folder is scanned.")
    })
}

/// What `show` says: `state_text`, or that no folder is left out, or that it isn't known yet.
pub fn show_text(context: &Context, machine: &Machine) -> String {
    state_text(context, machine).unwrap_or_else(|| {
        if needs_look(context, machine) {
            "Not known yet. The guest is asked at the next start.".to_owned()
        } else {
            "Guesthouse left no folder out of its scanning".to_owned()
        }
    })
}

/// Keeps the folders. None leaves no file.
pub fn write_exclusions(context: &Context, machine: &Machine, folders: &[String]) -> Result<()> {
    let path = exclusions_file(context, machine);
    if folders.is_empty() {
        return match fs::remove_file(path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
            _ => Ok(()),
        };
    }
    write_json(&path, &Kept { folders: folders.to_vec(), ..Kept::default() })
}

/// Asks the next start to find out from the guest what Defender leaves out.
pub fn mark_for_look(folder: &std::path::Path) -> Result<()> {
    Ok(fs::write(folder.join(LOOK_MARK), "")?)
}

pub fn needs_look(context: &Context, machine: &Machine) -> bool {
    machine.folder(context).join(LOOK_MARK).exists()
}

/// Keeps what the guest said, in place of what the app knew, and forgets the mark. A throwaway's
/// clean state is the disk that was asked, so its clean notes say the same.
pub fn record_found(context: &Context, machine: &Machine, found: &[String]) -> Result<()> {
    let mut places = vec![machine.folder(context)];
    if machine.throwaway {
        places.push(crate::throwaway::clean_folder(context, machine));
    }
    for place in places.iter().filter(|place| place.is_dir()) {
        let path = place.join(EXCLUSIONS_FILE);
        if found.is_empty() {
            match fs::remove_file(&path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
                _ => {}
            }
        } else {
            write_json(&path, &Kept { folders: found.to_vec(), applied: true, found: true })?;
        }
    }
    match fs::remove_file(machine.folder(context).join(LOOK_MARK)) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
        _ => Ok(()),
    }
}

#[cfg(test)]
#[path = "scan_exclusions_tests.rs"]
mod tests;
