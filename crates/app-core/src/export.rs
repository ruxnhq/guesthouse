//! Exporting a machine that is off into one file, which [`crate::import`] makes a machine of on
//! another PC. The file holds the machine's settings, its disk with the snapshots inside it, and the
//! files of its folder the guest relies on: the firmware settings, the TPM, the snapshots' own files,
//! the agent's token and the account's password. What belongs to this PC stays out of it: paths, the
//! shared folder, host drives, the bridge and the port forwards. The card's address stays, as it's how
//! the import tells which maker's card Windows was installed for, and it chooses a card of its own.

use crate::actions::Task;
use crate::carry::{MAGIC, Writer, safe_path};
use crate::context::Context;
use crate::disk_image::disk_info;
use crate::duplicate::{NOT_COPIED, disk_is_free};
use crate::failure::{Failure, Result};
use crate::host::{GIB, free_gib};
use crate::library::{DISK_FILE, FIRMWARE_FILE};
use crate::log::write_log;
use crate::machine::{MACHINE_FILE, Machine};
use crate::network::DEFAULT_BRIDGE;
use crate::settings::with_suffix;
use crate::tasks::{State, status};
use crate::throwaway::RUN_ONLY;
use rustix::fs::{FlockOperation, flock};
use std::fs::{self, File, OpenOptions};
use std::io::BufWriter;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

pub const EXPORT_TASK: &str = "export";
/// What an export's file name ends in.
pub const EXTENSION: &str = "guesthouse";
/// The export is written beside where it goes, and only takes its name once it's whole.
const PART_SUFFIX: &str = ".part";
/// Left free beside the export, so the drive isn't filled to its last byte.
const SPARE_GIB: f64 = 1.0;
const SHUT_DOWN_FIRST: &str = "Shut the VM down first.";
/// What an import never takes, so an export leaves it here: the recipe names a file of this PC's,
/// and the automatic snapshot's record is about this PC's starts. See `import/accepted.rs`.
/// The mark for a second token change stays too: the import plans its own change.
const STAYS_HERE: [&str; 4] = [
    crate::recipe::CHOSEN_FILE,
    crate::recipe::OUTCOME_FILE,
    crate::auto_snapshot::record::RECORD_FILE,
    crate::token_change::AGAIN_FILE,
];

/// A file that goes in the export: its name there, where it is now, and its size.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Carried {
    name: String,
    from: PathBuf,
    size: u64,
}

/// What an export of this machine would be.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportPlan {
    files: Vec<Carried>,
    pub gib: f64,
}

impl ExportPlan {
    pub fn cost_text(&self) -> String {
        format!("The file will be {:.1} GiB.", self.gib)
    }
}

/// Says why this machine can't be exported now.
fn refusal(context: &Context, machine: &Machine) -> Result<()> {
    if status(context, machine).state != State::Off {
        return Err(Failure::new(SHUT_DOWN_FIRST));
    }
    if machine.installing {
        return Err(Failure::new("It has not finished installing. Export it once it has."));
    }
    if !Path::new(&machine.disk).is_file() {
        return Err(Failure(format!("Its disk, {}, is missing.", machine.disk)));
    }
    if !disk_is_free(&machine.disk) {
        return Err(Failure::new("Its disk is in use by a VM this app didn't start. Stop that VM first."));
    }
    let info = disk_info(&machine.disk)?;
    if let Some(backing) = info["full-backing-filename"].as_str() {
        return Err(Failure(format!(
            "Its disk is built on {backing}, and an export only carries the one disk. Join them first with \
             qemu-img commit or qemu-img convert."
        )));
    }
    Ok(())
}

fn carried(name: &str, from: &Path) -> Result<Carried> {
    Ok(Carried { name: name.to_owned(), from: from.to_owned(), size: fs::metadata(from)?.len() })
}

/// The files of the folder that go, by their names in it. A link, and a name the import couldn't
/// make, are left out, and the log says so.
fn folder_files(
    context: &Context,
    folder: &Path,
    inside: &str,
    skipped: &[&Path],
    found: &mut Vec<Carried>,
) -> Result<()> {
    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        let file_name = entry.file_name().to_string_lossy().into_owned();
        let name = if inside.is_empty() { file_name.clone() } else { format!("{inside}/{file_name}") };
        let path = entry.path();
        let at_top = inside.is_empty();
        let left_out = at_top
            && (NOT_COPIED.contains(&file_name.as_str())
                || RUN_ONLY.contains(&file_name.as_str())
                || [DISK_FILE, FIRMWARE_FILE].contains(&file_name.as_str()))
            // A snapshot keeps the recipe's result with its other notes.
            || STAYS_HERE.contains(&file_name.as_str());
        if left_out || skipped.contains(&path.as_path()) {
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_symlink() || safe_path(&name).is_none() {
            let _ = write_log(context, &format!("An export left out {}", path.display()));
        } else if kind.is_dir() {
            folder_files(context, &path, &name, skipped, found)?;
        } else if kind.is_file() {
            found.push(carried(&name, &path)?);
        }
    }
    Ok(())
}

/// What the export carries, with the disk last, as it's by far the largest.
fn files_of(context: &Context, machine: &Machine) -> Result<Vec<Carried>> {
    let folder = machine.folder(context);
    let skipped = [Path::new(&machine.disk), Path::new(&machine.vars), Path::new(&machine.iso)];
    let mut found = Vec::new();
    if !machine.vars.is_empty() {
        found.push(carried(FIRMWARE_FILE, Path::new(&machine.vars))?);
    }
    if folder.is_dir() {
        folder_files(context, &folder, "", &skipped, &mut found)?;
    }
    if machine.throwaway {
        with_clean_notes(context, machine, &mut found)?;
    }
    found.push(carried(DISK_FILE, Path::new(&machine.disk))?);
    Ok(found)
}

/// A throwaway's disk is its clean state, so its notes about the guest go as they were then, not as
/// its last run left them. The import makes its clean state from these.
fn with_clean_notes(context: &Context, machine: &Machine, found: &mut Vec<Carried>) -> Result<()> {
    found.retain(|file| !crate::throwaway::is_guest_note(&file.name));
    let clean = crate::throwaway::clean_folder(context, machine);
    for entry in fs::read_dir(&clean).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if crate::throwaway::is_guest_note(&name) && entry.file_type()?.is_file() {
            found.push(carried(&name, &entry.path())?);
        }
    }
    Ok(())
}

/// The settings as they go in the export: this PC's paths, folders, drives, card and network left
/// out, and the files named as they are in the export.
fn settings_to_carry(context: &Context, machine: &Machine) -> Machine {
    let opencore = Path::new(&machine.opencore).strip_prefix(machine.folder(context)).ok();
    Machine {
        disk: DISK_FILE.to_owned(),
        vars: if machine.vars.is_empty() { String::new() } else { FIRMWARE_FILE.to_owned() },
        iso: String::new(),
        share: String::new(),
        drives: Vec::new(),
        disk_folder: String::new(),
        mac: String::new(),
        bridge: DEFAULT_BRIDGE.to_owned(),
        only_through: String::new(),
        dns: String::new(),
        forwards: Vec::new(),
        start_with_pc: false,
        start_without_jail: false,
        vcache_pin: false,
        opencore: opencore.map(|path| path.to_string_lossy().into_owned()).unwrap_or_default(),
        ..machine.clone()
    }
}

/// What exporting `machine` to `to` would be, or why it can't be. Nothing is written.
pub fn plan_export(context: &Context, machine: &Machine, to: &Path) -> Result<ExportPlan> {
    refusal(context, machine)?;
    if !to.is_absolute() {
        return Err(Failure::new("Give the export's file as a full path, from /."));
    }
    if to.exists() {
        return Err(Failure(format!("{} is already there. Choose another name.", to.display())));
    }
    let folder = to.parent().filter(|folder| folder.is_dir());
    let folder = folder.ok_or_else(|| Failure(format!("{} is not a folder on this PC.", to.display())))?;
    let files = files_of(context, machine)?;
    let gib = files.iter().map(|file| file.size).sum::<u64>() as f64 / GIB as f64;
    let free = free_gib(folder)?;
    if free < gib + SPARE_GIB {
        return Err(Failure(format!(
            "The export needs {:.0} GiB free in {}, and there is {free:.0} GiB.",
            (gib + SPARE_GIB).ceil(),
            folder.display()
        )));
    }
    Ok(ExportPlan { files, gib })
}

fn write_export(context: &Context, machine: &Machine, plan: &ExportPlan, to: &Path) -> Result<()> {
    // It holds the agent's token and the account's password, so it's this user's alone.
    let file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(to)?;
    // Held while it's written, so a later export can tell one a killed task left from one at work.
    flock(&file, FlockOperation::NonBlockingLockExclusive).map_err(std::io::Error::from)?;
    let mut writer = Writer::new(BufWriter::new(file))?;
    let settings = serde_json::to_vec_pretty(&settings_to_carry(context, machine))
        .map_err(|error| Failure(error.to_string()))?;
    writer.file(MACHINE_FILE, settings.len() as u64, &mut &settings[..])?;
    for file in &plan.files {
        writer.file(&file.name, file.size, &mut File::open(&file.from)?)?;
    }
    let written = writer.end()?;
    written.into_inner().map_err(|error| Failure(error.to_string()))?.sync_all()?;
    Ok(())
}

/// Takes away an unfinished export of ours at `part` that nobody is writing: one a killed task left,
/// as big as the disk. Anything else of that name is the user's, and stays.
fn take_away_abandoned(part: &Path) -> Result<()> {
    let Ok(mut file) = File::open(part) else { return Ok(()) };
    let mut start = vec![0; MAGIC.len()];
    let ours = file.read_exact(&mut start).is_ok() && start == MAGIC;
    let unwritten = flock(&file, FlockOperation::NonBlockingLockExclusive).is_ok();
    if !(ours && unwritten) {
        return Err(Failure(format!("{} is in the way. Move it, or choose another name.", part.display())));
    }
    Ok(fs::remove_file(part)?)
}

/// The export task: writes the machine to the file at `to`, under a name of its own until it's
/// whole. The machine is only read.
pub fn export(task: &mut Task, to: &str) -> Result<()> {
    let (context, machine) = (task.context, task.machine.clone());
    let to = PathBuf::from(to);
    // First, as what it left takes room the plan counts.
    let part = with_suffix(&to, PART_SUFFIX);
    take_away_abandoned(&part)?;
    let plan = plan_export(context, &machine, &to)?;
    task.step(&format!("Writing {:.1} GiB to {}", plan.gib, to.display()));
    let written = write_export(context, &machine, &plan, &part).and_then(|()| {
        if to.exists() {
            return Err(Failure(format!("{} appeared while the export was written.", to.display())));
        }
        Ok(fs::rename(&part, &to)?)
    });
    if written.is_err() {
        let _ = fs::remove_file(&part);
    }
    written?;
    let _ = write_log(context, &format!("{} was exported to {}", machine.id, to.display()));
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests;
