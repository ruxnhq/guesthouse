//! What a snapshot keeps beside the disk: the machine's own notes about its guest. Which vendor ID the
//! VM shows, what the display driver is doing and where the disk sits all describe what is inside
//! Windows (and whether an Intel guest's render device was switched off in it), so after going back to a disk from before a change they must say what that disk holds. A
//! machine left showing a vendor ID whose driver package the old disk never had starts with no GPU.
//! A throwaway's disk is its clean state, so its clean notes are the ones kept and put back.

use crate::actions::nvme_state_files;
use crate::context::Context;
use crate::display;
use crate::failure::Result;
use crate::gpu_identity;
use crate::intel_desktop;
use crate::machine::Machine;
use crate::recipe;
use crate::throwaway;
use serde_json::Value;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

/// The notes as they describe the disk. A throwaway's are the clean ones, as its disk is the clean
/// state and the notes beside its settings are what its last run left.
fn files(context: &Context, machine: &Machine) -> Vec<PathBuf> {
    let [trial, refused] = nvme_state_files(context, machine);
    let notes = [
        gpu_identity::state_file(context, machine),
        display::state_file(context, machine),
        trial,
        refused,
        intel_desktop::device_off_file(context, machine),
        machine.folder(context).join(recipe::OUTCOME_FILE),
        machine.folder(context).join(crate::lean::RESULT_FILE),
        machine.folder(context).join(crate::scan_exclusions::EXCLUSIONS_FILE),
    ];
    let clean = throwaway::clean_folder(context, machine);
    if !(machine.throwaway && clean.is_dir()) {
        return notes.into();
    }
    notes.iter().map(|note| clean.join(name_of(note))).collect()
}

fn name_of(path: &Path) -> &OsStr {
    path.file_name().unwrap_or_default()
}

/// Copies the notes that exist into `folder`. A note that is not there is left out, and going back
/// then removes it.
pub fn keep(context: &Context, machine: &Machine, folder: &Path) -> Result<()> {
    fs::create_dir_all(folder)?;
    for file in files(context, machine).iter().filter(|file| file.is_file()) {
        fs::copy(file, folder.join(name_of(file)))?;
    }
    Ok(())
}

/// Puts the notes back as `folder` has them. A snapshot from before they were kept has no folder, and
/// the notes stay as they are.
pub fn restore(context: &Context, machine: &Machine, folder: &Path) -> Result<()> {
    if !folder.is_dir() {
        return Ok(());
    }
    for file in files(context, machine) {
        let kept = folder.join(name_of(&file));
        if kept.is_file() {
            fs::copy(kept, &file)?;
        } else if file.is_file() {
            fs::remove_file(&file)?;
        }
    }
    Ok(())
}

/// Puts the disk back on the controller it was on, as a disk from before the move to NVMe has
/// Windows' key that keeps its driver from loading, and one on NVMe would not boot.
pub fn restore_disk_place(context: &Context, machine: &mut Machine, saved: PathBuf) -> Result<()> {
    let text = fs::read_to_string(saved).unwrap_or_default();
    let was = serde_json::from_str::<Value>(&text).ok().and_then(|saved| saved["nvme"].as_bool());
    match was {
        Some(nvme) if nvme != machine.nvme => {
            machine.nvme = nvme;
            machine.save_disk_place(context)
        }
        _ => Ok(()),
    }
}
