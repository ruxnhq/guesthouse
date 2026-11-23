//! Throwaway machines: every start begins from the same clean state, and what a run writes is gone
//! once it ends, however it ends and whether or not the app is open.
//!
//! A run writes to a qcow2 overlay on the machine's disk. The task that starts the VM makes the
//! overlay, opens it, and deletes it before QEMU starts, handing QEMU the open file. Nothing has its
//! name after that, so the kernel frees it when the last of the task and QEMU lets go, which needs
//! nothing of the app. The disk itself is only read. A task that starts the VM again, as a start does
//! for a new GPU identity, keeps the overlay it made, so the guest's own setup isn't lost halfway.
//!
//! The firmware settings and the TPM change while a guest runs too, so each run gets fresh copies of
//! them, and the notes the app keeps about what's in the guest go back to what they were when the
//! machine became a throwaway. What the guest writes to a shared folder or a host drive stays: those
//! are this PC's, not the machine's.

use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::machine::Machine;
use crate::random::random_hex;
use crate::snapshots::copy_tree;
use crate::tasks::{State, status};
use crate::vm_args::{Boot, MacosBoot, THROWAWAY_FDS, VmPlan, throwaway_disk};
use crate::writable_copy::copy_writable;
use guesthouse_daemon::handover::{KEPT_ABOVE, leave_open_at};
use rustix::io::fcntl_dupfd_cloexec;
use std::fs::{self, OpenOptions};
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// What a run is given fresh, in the machine's folder: the firmware settings, the TPM, and the
/// agent's token for this run.
const RUN_FOLDER: &str = "throwaway";
/// The notes about the guest as they were when it became a throwaway.
const CLEAN_FOLDER: &str = "throwaway-clean";
/// The notes the app keeps about what's in the guest, which a run can change.
const GUEST_NOTES: [&str; 8] = [
    crate::recipe::OUTCOME_FILE,
    "display.json",
    "gpu-identity.json",
    "nvme-trial",
    "nvme-refused",
    "intel-software-desktop",
    "intel-device-off",
    "amd-fallback",
];
const VARS_FILE: &str = "vars.fd";
const TPM_FOLDER: &str = "tpm";
pub const RUN_TOKEN_FILE: &str = "guest.token";
/// An overlay is made beside the disk, so it's on the same drive, and only has this name for a moment.
const OVERLAY_PREFIX: &str = ".throwaway-";
const OVERLAY_SUFFIX: &str = ".qcow2";
const OVERLAY_RANDOM_BYTES: usize = 4;
const PRIVATE: u32 = 0o600;
/// A macOS guest's own files a run gets copies of, as named in its folder.
const OPENCORE_FILE: &str = "opencore.qcow2";
const MACOS_HOME: &str = "gpu-cache";
const SHUT_DOWN_FIRST: &str = "Shut the VM down first.";

/// In a throwaway copy's folder that Duplicate was asked to delete once it has run: `waiting` until
/// a start has its VM up, then `started`. Only Duplicate makes it, and nothing carries it on.
pub const DELETE_MARK: &str = "delete-when-stopped";
const WAITING: &str = "waiting";
const STARTED: &str = "started";

pub fn run_folder(context: &Context, machine: &Machine) -> PathBuf {
    machine.folder(context).join(RUN_FOLDER)
}

/// Where a throwaway's notes about its guest are kept as its disk has them. The ones beside its
/// settings are the last run's, until the next start puts these back.
pub(crate) fn clean_folder(context: &Context, machine: &Machine) -> PathBuf {
    machine.folder(context).join(CLEAN_FOLDER)
}

/// The token a throwaway run's guest has, once the start has moved it there.
pub fn run_token_file(context: &Context, machine: &Machine) -> PathBuf {
    run_folder(context, machine).join(RUN_TOKEN_FILE)
}

/// Folders of the machine's own that a copy or an export leaves out: a run's, which goes anyway.
pub const RUN_ONLY: [&str; 1] = [RUN_FOLDER];

/// Makes the machine a throwaway or an ordinary machine again. What's on its disk now is the
/// clean state.
pub fn set_throwaway(context: &Context, machine: &mut Machine, on: bool) -> Result<()> {
    if machine.throwaway == on {
        return Ok(());
    }
    let found = status(context, machine);
    if found.state != State::Off || found.busy {
        return Err(Failure::new(SHUT_DOWN_FIRST));
    }
    if machine.installing {
        return Err(Failure::new("It has not finished installing. Make it a throwaway once it has."));
    }
    let clean = clean_folder(context, machine);
    if on {
        keep_notes(&machine.folder(context))?;
    } else if clean.is_dir() {
        // The disk is the clean state, and the notes beside the settings are what the last run left.
        restore_notes(context, machine)?;
    }
    machine.throwaway = on;
    if let Err(failure) = machine.save(context) {
        machine.throwaway = !on;
        if on {
            let _ = fs::remove_dir_all(&clean);
        }
        return Err(failure);
    }
    if !on {
        let _ = fs::remove_dir_all(&clean);
        let _ = fs::remove_dir_all(run_folder(context, machine));
        // Turned back into an ordinary machine, it's one to keep.
        let _ = fs::remove_file(machine.folder(context).join(DELETE_MARK));
    }
    Ok(())
}

/// Marks a copy being made in `folder` to be deleted once it has run.
pub fn mark_for_deletion(folder: &Path) -> Result<()> {
    Ok(fs::write(folder.join(DELETE_MARK), WAITING)?)
}

/// Notes that a copy marked to be deleted has had its VM up.
pub fn note_started(context: &Context, machine: &Machine) {
    let mark = machine.folder(context).join(DELETE_MARK);
    if machine.throwaway && mark.exists() {
        let _ = fs::write(mark, STARTED);
    }
}

/// Deletes each throwaway copy that was marked to go once it has run, has run, and is off with
/// nothing at work on it. Whichever of the app, `guesthouse-vm` or a start task looks next does
/// this, so nothing has to stay running for it. Returns the ids deleted.
pub fn delete_finished(context: &Context) -> Vec<String> {
    let mut deleted = Vec::new();
    for machine in crate::machine::load_all(context).0 {
        let mark = machine.folder(context).join(DELETE_MARK);
        let has_run = fs::read_to_string(&mark).is_ok_and(|said| said.trim() == STARTED);
        // Looked at every second by the app, so only a marked one is looked at further.
        if !(machine.throwaway && has_run) {
            continue;
        }
        let found = status(context, &machine);
        if found.state != State::Off || found.busy {
            continue;
        }
        match crate::library::delete(context, &machine) {
            Ok(()) => {
                let _ = crate::log::write_log(
                    context,
                    &format!("{} had run and was deleted, as asked", machine.id),
                );
                deleted.push(machine.id);
            }
            Err(failure) => {
                let _ = crate::log::write_log(
                    context,
                    &format!("{} could not be deleted: {failure}", machine.id),
                );
            }
        }
    }
    deleted
}

/// An install, as Start Over runs on a throwaway, writes a new clean state to the disk itself, as a
/// start does while a machine is installing. Once it's done, the notes it left are the clean ones.
pub fn installed(context: &Context, machine: &Machine) -> Result<()> {
    if machine.throwaway { keep_notes(&machine.folder(context)) } else { Ok(()) }
}

/// Keeps the notes about the guest in a machine's `folder` as they are now, for each run to start
/// from.
pub fn keep_notes(folder: &Path) -> Result<()> {
    let clean = folder.join(CLEAN_FOLDER);
    let _ = fs::remove_dir_all(&clean);
    fs::create_dir_all(&clean)?;
    for name in GUEST_NOTES {
        let note = folder.join(name);
        if note.is_file() {
            fs::copy(note, clean.join(name))?;
        }
    }
    Ok(())
}

/// The notes as they were when it became a throwaway: each kept one back, and any other gone.
fn restore_notes(context: &Context, machine: &Machine) -> Result<()> {
    let clean = clean_folder(context, machine);
    if !clean.is_dir() {
        return Err(Failure::new("Its clean state is missing. Turn Throwaway off and on again."));
    }
    for name in GUEST_NOTES {
        let (kept, note) = (clean.join(name), machine.folder(context).join(name));
        if kept.is_file() {
            fs::copy(kept, note)?;
        } else if note.exists() {
            fs::remove_file(note)?;
        }
    }
    Ok(())
}

/// One run of a throwaway machine, which lasts as long as the task that started it and its QEMU.
pub struct ThrowawayRun {
    pub machine: String,
    /// The overlay, open to write and to read, with no name left.
    overlay: [OwnedFd; 2],
    vars: Option<PathBuf>,
    tpm: PathBuf,
    /// A macOS guest's OpenCore image and its QEMU's home, which it writes to as well.
    macos: Option<(PathBuf, PathBuf)>,
}

impl ThrowawayRun {
    /// Everything a run starts from, made fresh.
    pub fn prepare(context: &Context, machine: &Machine) -> Result<Self> {
        let folder = run_folder(context, machine);
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder)?;
        restore_notes(context, machine)?;
        let vars = if machine.vars.is_empty() {
            None
        } else {
            let copy = folder.join(VARS_FILE);
            copy_writable(&machine.vars, &copy)?;
            Some(copy)
        };
        let tpm = folder.join(TPM_FOLDER);
        if machine.tpm_folder(context).is_dir() {
            copy_tree(&machine.tpm_folder(context), &tpm)?;
        }
        let macos = if machine.opencore.is_empty() {
            None
        } else {
            let (opencore, home) = (folder.join(OPENCORE_FILE), folder.join(MACOS_HOME));
            copy_writable(&machine.opencore, &opencore)?;
            let own_home = machine.folder(context).join(MACOS_HOME);
            if own_home.is_dir() {
                copy_tree(&own_home, &home)?;
            }
            fs::create_dir_all(&home)?;
            Some((opencore, home))
        };
        let overlay = open_overlay(&machine.id, Path::new(&machine.disk))?;
        Ok(Self { machine: machine.id.clone(), overlay, vars, tpm, macos })
    }

    /// The plan with this run's overlay, firmware settings and TPM in place of the machine's own.
    pub fn applied_to(&self, plan: &VmPlan) -> VmPlan {
        VmPlan {
            throwaway: Some(plan.disk.clone()),
            disk: throwaway_disk(),
            vars: self
                .vars
                .as_ref()
                .map_or_else(|| plan.vars.clone(), |vars| vars.to_string_lossy().into_owned()),
            tpm: plan.tpm.as_ref().map(|_| self.tpm.clone()),
            boot: match (&plan.boot, &self.macos) {
                (Boot::Macos(boot), Some((opencore, home))) => {
                    Boot::Macos(MacosBoot { opencore: opencore.clone(), home: home.clone(), ..boot.clone() })
                }
                (boot, _) => boot.clone(),
            },
            ..plan.clone()
        }
    }

    /// Leaves the overlay open in what this command starts, at [`THROWAWAY_FDS`].
    pub fn hand_to(&self, command: &mut Command) {
        let placed: Vec<(&OwnedFd, i32)> = self.overlay.iter().zip(THROWAWAY_FDS).collect();
        leave_open_at(command, &placed);
    }

    #[cfg(test)]
    pub fn overlay(&self) -> &[OwnedFd; 2] {
        &self.overlay
    }
}

/// Makes the overlay beside the disk, opens it twice, and deletes it. One that a task of this machine's
/// cut short left with its name is deleted first. The name has the machine's id in it, so another
/// machine's disk in the same folder never loses its own.
fn open_overlay(id: &str, disk: &Path) -> Result<[OwnedFd; 2]> {
    let disk = std::path::absolute(disk)?;
    let folder = disk.parent().ok_or_else(|| Failure::new("The disk has no folder."))?;
    let own = format!("{OVERLAY_PREFIX}{id}-");
    for entry in fs::read_dir(folder)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let random = name.strip_prefix(&own).and_then(|rest| rest.strip_suffix(OVERLAY_SUFFIX));
        if random.is_some_and(|random| random.len() == OVERLAY_RANDOM_BYTES * 2) {
            let _ = fs::remove_file(entry.path());
        }
    }
    let overlay = folder.join(format!("{own}{}{OVERLAY_SUFFIX}", random_hex::<OVERLAY_RANDOM_BYTES>()));
    // Made first for this user alone, so it never has a name anyone else can open.
    OpenOptions::new().write(true).create_new(true).mode(PRIVATE).open(&overlay)?;
    let made = Command::new("qemu-img")
        .args(["create", "-q", "-f", "qcow2", "-F", "qcow2", "-b"])
        .arg(&disk)
        .arg(&overlay)
        .output()?;
    if !made.status.success() {
        let said = String::from_utf8_lossy(&made.stderr);
        return Err(Failure(format!("The throwaway run's disk could not be made: {}", said.trim())));
    }
    let opened = open_both(&overlay);
    let _ = fs::remove_file(&overlay);
    Ok(opened?)
}

/// Kept above the numbers QEMU sees them at, as the drives are.
fn open_both(overlay: &Path) -> io::Result<[OwnedFd; 2]> {
    let writing = OpenOptions::new().read(true).write(true).open(overlay)?;
    let reading = OpenOptions::new().read(true).open(overlay)?;
    let above = |file: fs::File| fcntl_dupfd_cloexec(file, KEPT_ABOVE).map_err(io::Error::from);
    Ok([above(writing)?, above(reading)?])
}

#[cfg(test)]
mod tests;
