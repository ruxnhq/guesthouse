//! A copy of a machine that is off: a folder, disk and identity of its own, built where the library
//! can't see it, and put in place in one step once it's whole.
//!
//! What the guest relies on stays the same: the disk with its snapshots, the firmware settings, and
//! the TPM, which Windows may have sealed its disk's key in. What would clash with the original
//! while both run is new: the id, the MAC, and the agent's token, which both are moved to at their
//! next start. Host drives stay with the original, as two guests can't share a whole drive, and so
//! does the remote view's switch with its port on the network: the copy starts with it off, and
//! gets a port of its own when it is turned on.

use crate::actions::Task;
use crate::context::Context;
use crate::disk_image::backing::checked_chain;
use crate::failure::{Failure, Result};
use crate::host::{GIB, free_gib};
use crate::library::{DISK_FILE, FIRMWARE_FILE, new_id, unused_mac};
use crate::log::write_log;
use crate::machine::{MACHINE_FILE, Machine, sorted_names};
use crate::random::random_hex;
use crate::settings::{max_vram_gib_for, write_json};
use crate::tasks::{State, status};
use crate::token_change::{next_token_file_at, plan_token_change_at};
use rustix::fs::{FlockOperation, flock};
use std::fs::{self, File};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const DUPLICATE_TASK: &str = "duplicate";
/// The task's word after the name for a copy that is a throwaway.
pub const THROWAWAY_WORD: &str = "throwaway";
/// The word after that for a throwaway copy that's deleted once it has run and stopped.
pub const DELETE_WORD: &str = "delete-when-stopped";
/// Where copies are built, in the machines folder, hidden from the library.
const STAGING_ROOT: &str = ".duplicating";
/// In each copy's own place in there: the copy itself, a lock its task holds, and where its disk is
/// being built on another drive.
const STAGED_FOLDER: &str = "machine";
const LOCK_FILE: &str = "lock";
const OTHER_DRIVE_NOTE: &str = "other-drive";
/// A disk being built on another drive, beside where it goes.
const OTHER_DRIVE_PREFIX: &str = ".duplicate-";
/// What stays with the original: what an install or a report left, caches the guest makes again, a
/// token change that's the original's own, and the remote view's switch, whose port two machines
/// can't both listen on.
pub(crate) const NOT_COPIED: [&str; 13] = [
    crate::token_change::LOCK_FILE,
    crate::token_change::REFUSED_FILE,
    crate::throwaway::DELETE_MARK,
    crate::throwaway::RUN_ONLY[0],
    crate::throwaway::RUN_ONLY[1],
    MACHINE_FILE,
    "answer.iso",
    "answer-disc.new",
    "test-report",
    "gpu-cache",
    "intel-hardware-desktop-daemon.out",
    "thumbnail.png.new",
    guesthouse_remote_view_core::switch::SWITCH_FILE,
];
/// Larger files go through `cp`, which can share their blocks with the original's.
const BIG_FILE_BYTES: u64 = 1 << 20;
const PROBE_PREFIX: &str = ".guesthouse-probe-";
const PROBE_BYTES: usize = 64 << 10;
const PROBE_RANDOM_BYTES: usize = 4;
/// Left free beside a full copy, so the drive isn't filled to its last byte.
const SPARE_GIB: f64 = 2.0;
const SECTOR_BYTES: u64 = 512;
const SHUT_DOWN_FIRST: &str = "Shut the VM down first.";
pub(crate) const DISK_IN_USE: &str = "Its disk is in use by a VM this app didn't start. Stop that VM first.";

/// What a copy is to be, before anything is made.
#[derive(Debug, Clone, PartialEq)]
pub struct DuplicatePlan {
    pub copy: Machine,
    /// The drive can share the disk's blocks with the original's until either one writes, as btrfs
    /// and XFS can, so the copy takes moments and next to no room.
    pub shares_blocks: bool,
    /// What the disk takes on its drive now, which a full copy takes again.
    pub disk_gib: f64,
    /// Host drives the original has, which the copy doesn't.
    pub drives_left_out: usize,
    /// A throwaway copy that's deleted once it has run and stopped.
    pub delete_when_stopped: bool,
}

impl DuplicatePlan {
    /// What the copy costs, for the question before it's made.
    pub fn cost_text(&self) -> String {
        let gib = self.disk_gib;
        let cost = if self.shares_blocks {
            format!("Its disk of {gib:.1} GiB shares its blocks with the original's, so this is quick.")
        } else {
            format!("This copies {gib:.1} GiB, which can take some minutes.")
        };
        match self.drives_left_out {
            0 => cost,
            _ => format!("{cost} Host drives stay with the original."),
        }
    }
}

/// What a copy is called unless the user says otherwise.
pub fn copy_name(source: &Machine) -> String {
    format!("{} Copy", source.name)
}

pub fn staging_root(context: &Context) -> PathBuf {
    context.paths.machines.join(STAGING_ROOT)
}

/// Says why this machine can't be copied now.
fn refusal(context: &Context, source: &Machine) -> Option<Failure> {
    if status(context, source).state != State::Off {
        return Some(Failure::new(SHUT_DOWN_FIRST));
    }
    if source.installing {
        return Some(Failure::new("It has not finished installing. Copy it once it has."));
    }
    if !Path::new(&source.disk).is_file() {
        return Some(Failure(format!("Its disk, {}, is missing.", source.disk)));
    }
    (!disk_is_free(&source.disk)).then(|| Failure::new(DISK_IN_USE))
}

/// Whether no QEMU has the disk open to write. qemu-img won't open a disk that one has, unless
/// told to look anyway, which only the app's own reads of a running disk are.
pub(crate) fn disk_is_free(disk: &str) -> bool {
    let looked = Command::new("qemu-img").args(["info", "-f", "qcow2", "--output=json", disk]).output();
    looked.is_ok_and(|done| done.status.success())
}

/// Whether a QEMU or another program has the disk open to write, which qemu-img says by refusing it.
/// A disk that isn't there, or that qemu-img can't read for another reason, is not in use.
pub(crate) fn disk_in_use(disk: &str) -> bool {
    let Ok(looked) = Command::new("qemu-img").args(["info", "-f", "qcow2", "--output=json", disk]).output()
    else {
        return false;
    };
    !looked.status.success() && String::from_utf8_lossy(&looked.stderr).contains("lock")
}

/// `path` in the copy's folder, if it was in the original's.
fn rerooted(path: &str, from: &Path, to: &Path) -> String {
    match Path::new(path).strip_prefix(from) {
        Ok(inside) if !path.is_empty() => to.join(inside).to_string_lossy().into_owned(),
        _ => path.to_owned(),
    }
}

/// The copy's settings: the original's, with its own id, folder, disk and MAC.
fn copy_of(context: &Context, source: &Machine, name: &str, throwaway: bool) -> Machine {
    let id = new_id(context, name);
    let (from, to) = (source.folder(context), context.paths.machines.join(&id));
    let disk_name = Path::new(&source.disk).file_name().map_or(DISK_FILE.into(), |name| name.to_owned());
    let (disk, disk_folder) = match Path::new(&source.disk_folder).parent() {
        Some(place) if !source.disk_folder.is_empty() => {
            let folder = place.join(&id);
            (folder.join(disk_name).to_string_lossy().into_owned(), folder.to_string_lossy().into_owned())
        }
        _ if Path::new(&source.disk).starts_with(&from) => {
            (rerooted(&source.disk, &from, &to), String::new())
        }
        // An imported machine's disk is outside its folder, and the copy's is in its own.
        _ => (to.join(DISK_FILE).to_string_lossy().into_owned(), String::new()),
    };
    let vars = match source.vars.as_str() {
        "" => String::new(),
        inside if Path::new(inside).starts_with(&from) => rerooted(inside, &from, &to),
        _ => to.join(FIRMWARE_FILE).to_string_lossy().into_owned(),
    };
    Machine {
        id,
        name: name.trim().to_owned(),
        disk,
        disk_folder,
        vars,
        iso: rerooted(&source.iso, &from, &to),
        opencore: rerooted(&source.opencore, &from, &to),
        android_data: rerooted(&source.android_data, &from, &to),
        mac: unused_mac(context),
        drives: Vec::new(),
        start_with_pc: false,
        throwaway: source.throwaway || throwaway,
        ..source.clone()
    }
}

/// What the disk takes on its drive.
pub(crate) fn taken_gib(path: &str) -> f64 {
    fs::metadata(path).map_or(0.0, |found| (found.blocks() * SECTOR_BYTES) as f64 / GIB as f64)
}

/// Whether a file next to `disk` can be copied into `place` by sharing its blocks. Tried on a small
/// file of its own, which goes again either way.
fn shares_blocks(disk: &Path, place: &Path) -> bool {
    let Some(folder) = disk.parent() else { return false };
    let name = format!("{PROBE_PREFIX}{}", random_hex::<PROBE_RANDOM_BYTES>());
    let (probe, copy) = (folder.join(&name), place.join(&name));
    if fs::write(&probe, vec![0; PROBE_BYTES]).is_err() {
        return false;
    }
    let shared = Command::new("cp").arg("--reflink=always").arg("--").arg(&probe).arg(&copy).output();
    let _ = fs::remove_file(&probe);
    let _ = fs::remove_file(&copy);
    shared.is_ok_and(|done| done.status.success())
}

/// The folder the copy's disk goes in.
fn disk_place(copy: &Machine) -> PathBuf {
    Path::new(&copy.disk).parent().map(Path::to_owned).unwrap_or_default()
}

/// What a copy of `source` called `name` would be, or why there can't be one. Nothing is made.
/// A throwaway copy starts clean every time, from the disk as it is now.
pub fn plan_duplicate(
    context: &Context,
    source: &Machine,
    name: &str,
    throwaway: bool,
) -> Result<DuplicatePlan> {
    if name.trim().is_empty() {
        return Err(Failure::new("The copy needs a name."));
    }
    if let Some(problem) = refusal(context, source) {
        return Err(problem);
    }
    let copy = copy_of(context, source, name, throwaway);
    if let Some(problem) = copy.problems(max_vram_gib_for(context, &copy)).into_iter().next() {
        return Err(Failure(problem));
    }
    // The disk's own folder on another drive is made beside the original's.
    let place = if copy.disk_folder.is_empty() {
        context.paths.machines.clone()
    } else {
        let place = disk_place(&copy).parent().map(Path::to_owned).unwrap_or_default();
        if !place.is_dir() {
            return Err(Failure(format!("{} is not there. Is its drive plugged in?", place.display())));
        }
        if Path::new(&copy.disk_folder).exists() {
            return Err(Failure(format!("{} is already there. Choose another name.", copy.disk_folder)));
        }
        place
    };
    fs::create_dir_all(&place)?;
    let disk_gib = taken_gib(&source.disk);
    let shares = shares_blocks(Path::new(&source.disk), &place);
    let free = free_gib(&place)?;
    if !shares && free < disk_gib + SPARE_GIB {
        return Err(Failure(format!(
            "The copy needs {:.0} GiB free in {}, and there is {free:.0} GiB.",
            (disk_gib + SPARE_GIB).ceil(),
            place.display()
        )));
    }
    Ok(DuplicatePlan {
        copy,
        shares_blocks: shares,
        disk_gib,
        drives_left_out: source.drives.len(),
        delete_when_stopped: false,
    })
}

/// A copy being built, and the lock that says its task is still at it.
pub(crate) struct Staging {
    pub(crate) root: PathBuf,
    _lock: File,
}

impl Staging {
    pub(crate) fn folder(&self) -> PathBuf {
        self.root.join(STAGED_FOLDER)
    }

    pub(crate) fn new(context: &Context, copy: &Machine) -> Result<Self> {
        let root = staging_root(context).join(&copy.id);
        fs::create_dir_all(&root)?;
        let lock = File::create(root.join(LOCK_FILE))?;
        flock(&lock, FlockOperation::NonBlockingLockExclusive)
            .map_err(|_| Failure::new("This copy is being made already."))?;
        let staging = Self { root, _lock: lock };
        let _ = fs::remove_dir_all(staging.folder());
        fs::create_dir(staging.folder())?;
        if let Some(other) = other_drive_staging(copy) {
            fs::write(staging.root.join(OTHER_DRIVE_NOTE), other.to_string_lossy().as_bytes())?;
            let _ = fs::remove_dir_all(&other);
            fs::create_dir(&other)?;
        }
        Ok(staging)
    }

    /// Where the disk is built.
    fn disk(&self, copy: &Machine, context: &Context) -> PathBuf {
        let name = Path::new(&copy.disk).file_name().unwrap_or_default();
        match other_drive_staging(copy) {
            Some(other) => other.join(name),
            None => self
                .folder()
                .join(Path::new(&copy.disk).strip_prefix(copy.folder(context)).unwrap_or(name.as_ref())),
        }
    }
}

/// The folder a disk on another drive is built in, beside the one it becomes.
fn other_drive_staging(copy: &Machine) -> Option<PathBuf> {
    let folder = Path::new(&copy.disk_folder);
    let name = folder.file_name()?.to_string_lossy();
    (!copy.disk_folder.is_empty()).then(|| folder.with_file_name(format!("{OTHER_DRIVE_PREFIX}{name}")))
}

/// Takes away what a copy cut short left: one whose lock nobody holds. A disk built on another drive
/// goes with it, and so does one already moved into place there whose machine never came, which
/// holds nothing but that disk.
pub fn sweep_abandoned(context: &Context) {
    let root = staging_root(context);
    for id in sorted_names(&root) {
        let place = root.join(&id);
        let Ok(lock) = File::open(place.join(LOCK_FILE)) else {
            let _ = fs::remove_dir_all(&place);
            continue;
        };
        if flock(&lock, FlockOperation::NonBlockingLockExclusive).is_err() {
            continue;
        }
        if let Ok(other) = fs::read_to_string(place.join(OTHER_DRIVE_NOTE)) {
            let other = PathBuf::from(other);
            let named_by_us =
                other.file_name().is_some_and(|name| name.to_string_lossy().starts_with(OTHER_DRIVE_PREFIX));
            if named_by_us {
                let _ = fs::remove_dir_all(&other);
                let placed = other.with_file_name(&id);
                let only_a_disk = sorted_names(&placed).len() == 1;
                if only_a_disk && !context.paths.machines.join(&id).exists() {
                    let _ = fs::remove_dir_all(placed);
                }
            }
        }
        let _ = fs::remove_dir_all(&place);
        let _ = write_log(context, &format!("Took away a copy of a machine that was cut short: {id}"));
    }
    let _ = fs::remove_dir(&root);
}

/// Copies one file, sharing its blocks where the drive can, keeping what it's allowed to be.
fn copy_file(from: &Path, to: &Path) -> Result<()> {
    if fs::metadata(from)?.len() < BIG_FILE_BYTES {
        fs::copy(from, to)?;
        return Ok(());
    }
    let done = Command::new("cp")
        .args(["--reflink=auto", "--preserve=mode"])
        .arg("--")
        .arg(from)
        .arg(to)
        .output()?;
    if !done.status.success() {
        let said = String::from_utf8_lossy(&done.stderr);
        return Err(Failure(format!("{} could not be copied: {}", from.display(), said.trim())));
    }
    Ok(())
}

/// Copies a folder and what is in it, but for `skipped` at its top and the original's disk. A link
/// isn't followed: what it leads to isn't the machine's own.
fn copy_folder(context: &Context, from: &Path, to: &Path, skipped: &[&str], disk: &Path) -> Result<()> {
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        let (path, target) = (entry.path(), to.join(&name));
        let kind = entry.file_type()?;
        if skipped.iter().any(|skip| name == *skip) || path == disk {
            continue;
        }
        if kind.is_symlink() {
            let _ = write_log(context, &format!("A copy left out the link {}", path.display()));
        } else if kind.is_dir() {
            fs::create_dir(&target)?;
            fs::set_permissions(&target, fs::Permissions::from_mode(entry.metadata()?.mode()))?;
            copy_folder(context, &path, &target, &[], disk)?;
        } else {
            copy_file(&path, &target)?;
        }
    }
    Ok(())
}

/// Refuses to copy a disk whose header names other files than the original's settings do, as the
/// copy's settings name the same ones.
fn check_backing_files(source: &Machine) -> Result<()> {
    checked_chain(Path::new(&source.disk), &source.backing_files)
        .map(|_| ())
        .map_err(|refused| Failure(refused.0.replacen("The VM did not start", "The copy was not made", 1)))
}

/// The copy names the backing file the original's settings name, if there is one, by its full path,
/// so it reads the same file the original does from its own folder.
fn keep_backing_file(source: &Machine, copy: &Path) -> Result<()> {
    let Some(backing) = source.backing_files.first() else { return Ok(()) };
    let done = Command::new("qemu-img")
        .args(["rebase", "-u", "-f", "qcow2", "-F", &backing.format, "-b", &backing.path])
        .arg(copy)
        .output()?;
    if !done.status.success() {
        return Err(Failure(format!(
            "The copy's disk could not name its backing file: {}",
            String::from_utf8_lossy(&done.stderr).trim()
        )));
    }
    Ok(())
}

/// The copy's files, all in the staging folder.
fn build(
    context: &Context,
    task: &Task,
    source: &Machine,
    plan: &DuplicatePlan,
    staging: &Staging,
    waiting_before: bool,
) -> Result<()> {
    let copy = &plan.copy;
    let staged_disk = staging.disk(copy, context);
    let gib = plan.disk_gib;
    task.step(&if plan.shares_blocks {
        "Copying the disk".to_owned()
    } else {
        format!("Copying the disk, {gib:.1} GiB")
    });
    check_backing_files(source)?;
    copy_file(Path::new(&source.disk), &staged_disk)?;
    keep_backing_file(source, &staged_disk)?;
    task.step("Copying the machine's files");
    let from = source.folder(context);
    if from.is_dir() {
        fs::set_permissions(staging.folder(), fs::metadata(&from)?.permissions())?;
        copy_folder(context, &from, &staging.folder(), &NOT_COPIED, Path::new(&source.disk))?;
    }
    if !source.vars.is_empty() && !Path::new(&source.vars).starts_with(&from) {
        fs::copy(&source.vars, staging.folder().join(FIRMWARE_FILE))?;
    }
    if copy.throwaway && !source.throwaway {
        crate::throwaway::keep_notes(&staging.folder())?;
    }
    if plan.delete_when_stopped {
        crate::throwaway::mark_for_deletion(&staging.folder())?;
    }
    let token_file = staging.folder().join(copy.token_file(context).file_name().unwrap_or_default());
    if !waiting_before {
        // The original's waiting token was made by this duplicate, while it was off, so no guest can
        // have it yet. The copy plans one of its own, and moves to it in one change, not two.
        let _ = fs::remove_file(next_token_file_at(&token_file));
    }
    plan_token_change_at(&token_file)?;
    crate::actions::copy_identity::plan_at(&staging.folder(), copy.kind)?;
    if let Some(problem) = copy.problems(max_vram_gib_for(context, copy)).into_iter().next() {
        return Err(Failure(problem));
    }
    write_json(&staging.folder().join(MACHINE_FILE), copy)
}

/// Puts the copy in place: its disk on another drive first, then the machine, which the library
/// sees from then on.
fn put_in_place(context: &Context, copy: &Machine, staging: &Staging) -> Result<()> {
    if let Some(other) = other_drive_staging(copy) {
        fs::rename(&other, &copy.disk_folder)?;
    }
    fs::rename(staging.folder(), copy.folder(context))?;
    Ok(())
}

/// The duplicate task: makes the copy called `name`. The original is only read, but for a new token
/// its guest is moved to at its next start.
pub fn duplicate(task: &mut Task, name: &str, throwaway: bool, delete_when_stopped: bool) -> Result<()> {
    let (context, source) = (task.context, task.machine.clone());
    sweep_abandoned(context);
    if delete_when_stopped && !throwaway {
        return Err(Failure::new("Only a throwaway copy can be deleted once it has stopped."));
    }
    let plan = DuplicatePlan { delete_when_stopped, ..plan_duplicate(context, &source, name, throwaway)? };
    // A waiting token from before may be the original guest's already, after a change cut short, so
    // the copy keeps it to reach its guest, and both move on from it once more.
    let waiting_before = crate::token_change::next_token_file(context, &source).exists();
    // Before the copy, so a crash once it's in place can't leave the original on the shared token.
    // A copy that then fails costs the original no more than a change of token it didn't need.
    crate::token_change::plan_token_change(context, &source)?;
    let staging = Staging::new(context, &plan.copy)?;
    let made = build(context, task, &source, &plan, &staging, waiting_before)
        .and_then(|()| put_in_place(context, &plan.copy, &staging));
    if let Err(failure) = made {
        if let Some(other) = other_drive_staging(&plan.copy) {
            let _ = fs::remove_dir_all(other);
        }
        let _ = fs::remove_dir_all(&staging.root);
        let _ = fs::remove_dir(staging_root(context));
        return Err(failure);
    }
    let _ = fs::remove_dir_all(&staging.root);
    let _ = fs::remove_dir(staging_root(context));
    let _ = write_log(context, &format!("{} was copied to {}", source.id, plan.copy.id));
    Ok(())
}

#[cfg(test)]
mod tests;
