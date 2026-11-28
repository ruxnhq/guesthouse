//! Which files in a run folder are a run's own logs, and whose run it was.
//!
//! Slots are shared between machines, and a start doesn't clear every file an earlier run left, so a
//! run's files are the ones on this list that changed at or after its note was written. The note,
//! `MACHINE_NOTE`, names the machine.

use crate::daemon;
use crate::vm_args::{self, INSTALL_LOG, LOGCAT_FILE};
use crate::vm_host::MACHINE_NOTE;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// What the host's side of a run leaves: the daemon's output and QEMU's, QEMU's own log, the TPM's,
/// the Linux GPU device's calls, and the small notes the app and the daemon leave for the machine's
/// row (written in jailing.rs, daemon/src/display_note.rs, daemon/src/gpu_note.rs, actions/ubuntu.rs
/// and actions/desktop.rs). The jail's note is in the notes folder beside the run folder.
pub const HOST_LOGS: [&str; 10] = [
    daemon::OUTPUT_FILE,
    vm_args::OUTPUT_FILE,
    "qemu.log",
    "swtpm.log",
    "calls.log",
    NOTES_FOLDER_LOG,
    "display-ring",
    "gpu-stopped",
    "gpu-off-reason",
    "guesthouse-boots",
];
/// The one of those that is kept in the notes folder, out of the jails' reach.
const NOTES_FOLDER_LOG: &str = crate::jailing::NOTE_FILE;
/// The logs a start keeps from the run before, in vm_host/local.rs.
pub const KEPT_HOST_LOGS: [&str; 2] = [daemon::OUTPUT_FILE, "qemu.log"];
/// What the guest wrote to its serial port.
pub const GUEST_SERIAL: &str = "serial.log";
/// What an Ubuntu install wrote to its serial port.
pub const INSTALL_SERIAL: &str = INSTALL_LOG;
/// What Android's logcat wrote to its second console: errors and crashes, from every app.
pub const GUEST_LOGCAT: &str = LOGCAT_FILE;
/// The suffix of the logs kept from the run before.
pub const KEPT_SUFFIX: &str = ".1";

/// The machine a note names and when it was written, which is when that run started.
pub fn noted_run(folder: &Path, note: &str) -> Option<(String, SystemTime)> {
    let path = folder.join(note);
    let id = fs::read_to_string(&path).ok()?.trim().to_owned();
    (!id.is_empty()).then_some(id).zip(modified(&path))
}

/// When this machine's current run in the folder started, if the folder's run is this machine's.
pub fn run_of(folder: &Path, id: &str) -> Option<SystemTime> {
    noted_run(folder, MACHINE_NOTE).filter(|(noted, _)| noted == id).map(|(_, since)| since)
}

/// When this machine's run before the current one started, if the kept logs are this machine's.
pub fn kept_run_of(folder: &Path, id: &str) -> Option<SystemTime> {
    let note = format!("{MACHINE_NOTE}{KEPT_SUFFIX}");
    noted_run(folder, &note).filter(|(noted, _)| noted == id).map(|(_, since)| since)
}

/// Each of these files, with `suffix` added, that is there and changed at or after `since`.
pub fn files_since(folder: &Path, names: &[&str], suffix: &str, since: SystemTime) -> Vec<(String, PathBuf)> {
    found_since(names, suffix, since, |name| folder.join(name))
}

/// The same for a run folder, whose jail note is in the notes folder beside it.
pub fn run_files_since(
    run_folder: &Path,
    names: &[&str],
    suffix: &str,
    since: SystemTime,
) -> Vec<(String, PathBuf)> {
    let notes = crate::paths::notes_beside(run_folder);
    found_since(names, suffix, since, |name| {
        if name == NOTES_FOLDER_LOG { notes.join(name) } else { run_folder.join(name) }
    })
}

fn found_since(
    names: &[&str],
    suffix: &str,
    since: SystemTime,
    place: impl Fn(&str) -> PathBuf,
) -> Vec<(String, PathBuf)> {
    names
        .iter()
        .map(|name| format!("{name}{suffix}"))
        .map(|name| {
            let path = place(&name);
            (name, path)
        })
        .filter(|(_, path)| modified(path).is_some_and(|changed| changed >= since))
        .collect()
}

/// When the file was last changed, if it's a plain file and not a link.
pub fn modified(path: &Path) -> Option<SystemTime> {
    let metadata = fs::symlink_metadata(path).ok()?;
    metadata.is_file().then(|| metadata.modified().ok()).flatten()
}

/// Opens a file of a run folder to read, following no link and only if it's a plain file. A jailed
/// process writes in a run folder, and what it leaves there is only ever its own file.
pub fn open_run_file(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = fs::File::options()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a plain file"));
    }
    Ok(file)
}

/// A moment as a reader would want it.
pub fn local_time(moment: SystemTime) -> String {
    // A file's time that jiff can't hold is no real time, so it reads as the start of 1970.
    let moment = Timestamp::try_from(moment).unwrap_or(Timestamp::UNIX_EPOCH);
    moment.to_zoned_datetime(TimeZone::system()).strftime("%Y-%m-%d %H:%M").to_string()
}
