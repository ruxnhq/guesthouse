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
use serde_json::json;
use std::fs;
use std::path::PathBuf;

/// Beside `machine.json`. A machine without exclusions has none.
pub const EXCLUSIONS_FILE: &str = "scan-exclusions.json";
/// A recipe names a few folders, not a drive's worth.
pub const MOST: usize = 8;
const LONGEST: usize = 200;
/// Folders that hold Windows itself, every program or the user's whole profile, or where files from
/// the internet land and run from. Leaving one of these out is close to turning Defender off.
const NEVER_TOP: [&str; 5] = ["windows", "users", "program files", "program files (x86)", "programdata"];
const NEVER_ANYWHERE: [&str; 7] = ["downloads", "desktop", "documents", "appdata", "temp", "tmp", "startup"];

/// What it means, said where it is chosen.
pub const WHAT_IT_MEANS: &str = "Defender, Windows' own virus protection, doesn't check files in these folders as \
                                 they are written or run. Defender stays on for everything else. It suits a \
                                 machine for games or builds you trust, where it makes loading and installing \
                                 faster, and not one for files or programs you don't know.";

/// Why a folder can't be left out of scanning, if it can't. A folder is a full path on a drive, of
/// plain characters, and none of the places that would leave most of Windows unchecked.
pub fn folder_problem(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    let drive = bytes.len() > 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\';
    let plain = path.len() <= LONGEST
        && path
            .chars()
            .skip(2)
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '\\' | ' ' | '.' | '_' | '-' | '(' | ')'));
    let parts: Vec<String> = path.get(3..).unwrap_or_default().split('\\').map(str::to_lowercase).collect();
    // Windows drops a dot or a space at the end of a name, so C:\Windows. would be C:\Windows.
    let tidy = parts.iter().all(|part| {
        !part.is_empty() && part.trim() == part && !part.ends_with('.') && !part.starts_with('.')
    });
    if !drive || !plain || !tidy {
        return Some(format!(
            "\"{path}\" is not a folder to leave out of scanning. It is a full path such as D:\\Games, \
             of letters, digits, spaces and . _ - ( ) only."
        ));
    }
    let whole = parts.len() == 1 && NEVER_TOP.contains(&parts[0].as_str());
    let in_windows = parts[0] == "windows";
    let profile = parts[0] == "users" && parts.len() <= 2;
    let risky = parts.iter().any(|part| NEVER_ANYWHERE.contains(&part.as_str()));
    (whole || in_windows || profile || risky).then(|| {
        format!(
            "{path} holds too much of Windows, or what comes from the internet, to leave out of scanning."
        )
    })
}

/// Why a list of folders can't be used, if it can't.
pub fn list_problem(folders: &[String]) -> Option<String> {
    if folders.len() > MOST {
        return Some(format!("At most {MOST} folders can be left out of scanning."));
    }
    folders.iter().find_map(|folder| folder_problem(folder))
}

/// What it means, and the folders, as a block of text.
pub fn what_it_does(folders: &[String]) -> String {
    let list: Vec<String> = folders.iter().map(|folder| format!("- {folder}")).collect();
    format!("{WHAT_IT_MEANS}\n{}\nThe machine's settings can have Defender scan them again.", list.join("\n"))
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Kept {
    folders: Vec<String>,
    /// Defender took them. Until then, and if it didn't, every folder is scanned.
    #[serde(default)]
    applied: bool,
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
    Some(if kept.applied {
        format!("Defender doesn't scan {folders}. Everything else is scanned.")
    } else if machine.installing {
        format!("Defender is to leave out {folders} once Windows is installed.")
    } else {
        format!("Defender was to leave out {folders}, but didn't take them, so every folder is scanned.")
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
    write_json(&path, &Kept { folders: folders.to_vec(), applied: false })
}

/// The folders as the guest's script is given them, as data.
pub fn guest_data(folders: &[String]) -> String {
    json!({ "folders": folders }).to_string()
}

#[cfg(test)]
#[path = "scan_exclusions_tests.rs"]
mod tests;
