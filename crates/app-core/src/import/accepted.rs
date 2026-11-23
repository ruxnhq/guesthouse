//! Which files of an export become files of the new machine, and what each must be. This is every
//! file the app reads from a machine's folder, with what an import does with it. A name not here is
//! never written.
//!
//! Carried as they are, as only the guest or a program for this guest reads them:
//! - `disk.qcow2`, and the OpenCore image the settings name: qcow2s that stand alone, checked
//!   from their headers and by qemu-img.
//! - `vars.fd`, `OVMF_CODE.fd` beside the OpenCore image, `snapshots/TAG.fd`: firmware, read by the
//!   guest's firmware in QEMU.
//! - `tpm/FILE` and `snapshots/TAG.tpm/FILE`: the TPM's state, read by swtpm. That runs outside the
//!   jail, as it does for every machine.
//! - `ssh/id` and `ssh/id.pub`: the key this PC's ssh logs in to an Ubuntu guest with.
//!
//! Carried once checked, and left out with a note if they fail:
//! - `guest.token`, `guest.token.next`: an agent token, as the app makes them.
//! - `windows-password`, `seed/desk-password`: one line, as an account's password must be.
//!
//! Made again here from what they hold:
//! - `display.json`, `gpu-identity.json`: read as the app's own notes and written out again.
//! - `snapshots/TAG.json`: a snapshot's name, and whether its disk was on NVMe, written out again.
//!   Going back to a disk from before the move to NVMe puts it back on SATA, where it boots.
//! - `snapshots/TAG.state/`: the same notes and marks as above, as the snapshot kept them, so going
//!   back says what that disk holds.
//! - `nvme-trial`, `nvme-refused`, `intel-device-off`: marks, made again empty.
//! - `machine.json`: from the settings, as [`super::machine_from`] takes them.
//! - `throwaway-clean/`: from the notes above, if the settings make it a throwaway.
//!
//! Never carried:
//! - `remote-access.json`: Remote Desktop and SSH are opt-ins that reach this PC, and SSH sends
//!   your own keys into the guest. They are chosen here.
//! - `amd-fallback`, `intel-software-desktop`: what the other PC's card did, learnt again here.
//! - `thumbnail.png`: a picture this PC's image loader would read. The next run makes one.
//! - `gpu-cache/`: shaders compiled by the other PC's driver, which this PC's would load.
//! - `throwaway/` and `delete-when-stopped`: a run's own, and a choice made here.
//! - `guest.token.next.again`, `guest.token.lock` and `guest.token.refused`: the other PC's own
//!   bookkeeping of a token change. The import plans a change of its own.
//! - `recipe.json` and `recipe-result.json`: a recipe holds a script that Run Again would run in the
//!   guest as its administrator, which nobody here has read. What it installed is on the disk.
//! - `auto-snapshot.json`: which snapshot to offer after a failed start, and which changes a start
//!   has met, which this PC's starts find again. A snapshot that was automatic is kept as one of
//!   yours, so nothing deletes it by itself.
//! - Whatever an install, a report or a log left: `answer.iso`, `boot/`, the rest of `seed/`,
//!   `test-report/`, and the like.

use crate::display::DisplayState;
use crate::failure::{Failure, Result};
use crate::gpu_identity::IdentityState;
use crate::machine::Machine;
use crate::settings::write_json;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

/// More than any of the small files above holds.
pub(super) const MOST_SMALL_BYTES: u64 = 64 << 10;
const TOKEN_HEX_DIGITS: usize = 32;
const TOKENS: [&str; 2] = ["guest.token", "guest.token.next"];
const PASSWORDS: [&str; 2] = ["windows-password", "seed/desk-password"];
const MARKS: [&str; 3] = ["nvme-trial", "nvme-refused", "intel-device-off"];
const DISPLAY_NOTE: &str = "display.json";
const IDENTITY_NOTE: &str = "gpu-identity.json";
const SSH_KEYS: [&str; 2] = ["ssh/id", "ssh/id.pub"];
const OPENCORE_FIRMWARE: &str = "OVMF_CODE.fd";
const SNAPSHOT_NAME_LIMIT: usize = crate::snapshots::MAX_SNAPSHOT_NAME;
const SNAPSHOT_NOTES: &str = ".state";

/// What becomes of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Handling {
    /// Written as it is. A large one: a disk, firmware or the TPM.
    AsItIs,
    /// Written, and checked or made again once all are in: see [`finish`].
    Small,
}

fn in_folder(path: &Path, folder: &str) -> Option<String> {
    let rest = path.strip_prefix(folder).ok()?;
    (rest.components().count() == 1).then(|| rest.to_string_lossy().into_owned())
}

/// What becomes of the file at `path` in an export whose settings are `carried`, or `None` for one
/// that's never written.
pub(super) fn handling(path: &Path, carried: &Machine) -> Option<Handling> {
    let name = path.to_string_lossy();
    let opencore = Path::new(&carried.opencore);
    let opencore_firmware = opencore.parent().map(|folder| folder.join(OPENCORE_FIRMWARE));
    if [crate::library::DISK_FILE, crate::library::FIRMWARE_FILE].contains(&name.as_ref())
        || (!carried.opencore.is_empty() && (path == opencore || Some(path) == opencore_firmware.as_deref()))
        || in_folder(path, "tpm").is_some()
    {
        return Some(Handling::AsItIs);
    }
    if let Some(file) = in_folder(path, "snapshots") {
        let (tag, kind) = file.rsplit_once('.')?;
        return match kind {
            "fd" if super::is_snapshot_tag(tag) => Some(Handling::AsItIs),
            "json" if super::is_snapshot_tag(tag) => Some(Handling::Small),
            _ => None,
        };
    }
    if let Some(parent) = path.parent().and_then(|parent| parent.to_str()) {
        let in_snapshot_tpm = parent.strip_prefix("snapshots/").and_then(|tpm| tpm.strip_suffix(".tpm"));
        if in_snapshot_tpm.is_some_and(super::is_snapshot_tag) {
            return Some(Handling::AsItIs);
        }
        let in_snapshot_notes =
            parent.strip_prefix("snapshots/").and_then(|notes| notes.strip_suffix(SNAPSHOT_NOTES));
        let note = path.file_name().and_then(|note| note.to_str()).unwrap_or_default();
        if in_snapshot_notes.is_some_and(super::is_snapshot_tag) && is_guest_note(note) {
            return Some(Handling::Small);
        }
    }
    let small = TOKENS.iter().chain(&PASSWORDS).chain(&MARKS).chain(&SSH_KEYS).any(|known| *known == name)
        || [DISPLAY_NOTE, IDENTITY_NOTE].contains(&name.as_ref());
    small.then_some(Handling::Small)
}

/// The notes about the guest that a snapshot keeps, and the import makes again.
fn is_guest_note(name: &str) -> bool {
    MARKS.contains(&name) || [DISPLAY_NOTE, IDENTITY_NOTE].contains(&name)
}

/// The notes and marks in `folder` made again, or left out.
fn remake_notes(folder: &Path) -> Result<()> {
    for mark in MARKS {
        let path = folder.join(mark);
        if path.exists() {
            fs::write(path, "")?;
        }
    }
    remade::<DisplayState>(folder, DISPLAY_NOTE)?;
    remade::<IdentityState>(folder, IDENTITY_NOTE)
}

fn is_token(text: &str) -> bool {
    let token = text.trim();
    token.len() == TOKEN_HEX_DIGITS && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Leaves out `file` if it's there and `fails`, saying so once in `notes`.
fn keep_if(
    folder: &Path,
    file: &str,
    fails: impl Fn(&str) -> bool,
    note: &str,
    notes: &mut Vec<String>,
) -> Result<()> {
    let path = folder.join(file);
    let Ok(text) = fs::read_to_string(&path) else {
        if path.exists() {
            fs::remove_file(&path)?;
            notes.push(note.to_owned());
        }
        return Ok(());
    };
    if fails(&text) {
        fs::remove_file(&path)?;
        if !notes.iter().any(|said| said == note) {
            notes.push(note.to_owned());
        }
    }
    Ok(())
}

/// A note the app keeps, read as its own type and written out again, or left out.
fn remade<T: serde::de::DeserializeOwned + serde::Serialize>(folder: &Path, file: &str) -> Result<()> {
    let path = folder.join(file);
    let Ok(text) = fs::read_to_string(&path) else { return Ok(()) };
    match serde_json::from_str::<T>(&text) {
        Ok(note) => write_json(&path, &note),
        Err(_) => Ok(fs::remove_file(&path)?),
    }
}

/// Checks or makes again each small file, once all are written.
pub(super) fn finish(folder: &Path, notes: &mut Vec<String>) -> Result<()> {
    const BAD_TOKEN: &str =
        "The agent's token in the export wasn't one, so the app can't reach the guest's agent.";
    const BAD_PASSWORD: &str = "The account's password in the export wasn't one, so it was left out.";
    for file in TOKENS {
        keep_if(folder, file, |text| !is_token(text), BAD_TOKEN, notes)?;
    }
    for file in PASSWORDS {
        let wrong =
            |text: &str| crate::account::password_problem(text.strip_suffix('\n').unwrap_or(text)).is_some();
        keep_if(folder, file, wrong, BAD_PASSWORD, notes)?;
    }
    remake_notes(folder)?;
    let snapshots = folder.join("snapshots");
    for entry in fs::read_dir(&snapshots).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() && path.to_string_lossy().ends_with(SNAPSHOT_NOTES) {
            remake_notes(&path)?;
        }
        if path.extension().is_some_and(|kind| kind == "json") {
            let said: Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap_or_default()).unwrap_or_default();
            let name: String =
                said["name"].as_str().unwrap_or_default().chars().take(SNAPSHOT_NAME_LIMIT).collect();
            let name = if name.trim().is_empty() { "Snapshot".to_owned() } else { name };
            let kept = match said["nvme"].as_bool() {
                Some(nvme) => json!({ "name": name, "nvme": nvme }),
                None => json!({ "name": name }),
            };
            fs::write(&path, kept.to_string())?;
        }
    }
    Ok(())
}

/// Refuses a small file that's too big to be what it says.
pub(super) fn small_enough(path: &Path, size: u64) -> Result<()> {
    if size > MOST_SMALL_BYTES {
        return Err(Failure(format!("{} in the export is far bigger than it can be.", path.display())));
    }
    Ok(())
}
