//! One viewer for a machine. The viewer holds an exclusive lock on a file in the machine's run folder
//! for as long as it lives, and a second viewer for the same machine, however it came to start, finds
//! the lock taken and ends at once. The lock is the operating system's, so it goes with the process,
//! however the process ends, and never outlives it.

use crate::context::Context;
use guesthouse_daemon::slot::Slot;
use rustix::fs::{FlockOperation, flock};
use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

const LOCK_FILE: &str = "app-windows-viewer.lock";
const PRIVATE: u32 = 0o600;

/// The viewer's lock for the machine that runs in this slot.
pub fn lock_file(context: &Context, slot: Slot) -> PathBuf {
    context.run_folder(Some(slot)).join(LOCK_FILE)
}

/// Held by the one viewer of a machine.
pub struct ViewerLock(#[allow(dead_code)] File);

impl ViewerLock {
    /// Writes this process's id into the lock file, for a command that has to end the viewer to find.
    /// Only the holder writes it, so a file that names a process is one that held the lock.
    pub fn record_pid(&self) -> io::Result<()> {
        use std::io::Write;
        self.0.set_len(0)?;
        (&self.0).write_all(std::process::id().to_string().as_bytes())
    }
}

/// The process id the holder of this run folder's lock wrote, if it wrote one. Nothing says the
/// process is still the holder: `modes` checks that before it signals anything.
pub fn recorded_pid(run_folder: &Path) -> Option<u32> {
    std::fs::read_to_string(run_folder.join(LOCK_FILE)).ok()?.trim().parse().ok()
}

/// Takes the lock, or says `None` if a viewer of this machine already has it.
pub fn take(context: &Context, slot: Slot) -> io::Result<Option<ViewerLock>> {
    context.make_run_folder(Some(slot))?;
    take_in(&context.run_folder(Some(slot)))
}

/// The same for a run folder that is already made.
pub fn take_in(run_folder: &Path) -> io::Result<Option<ViewerLock>> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(PRIVATE)
        .open(run_folder.join(LOCK_FILE))?;
    Ok(flock(&file, FlockOperation::NonBlockingLockExclusive).is_ok().then_some(ViewerLock(file)))
}

/// Whether a viewer has the lock in this run folder now.
pub fn held_in(run_folder: &Path) -> io::Result<bool> {
    Ok(take_in(run_folder)?.is_none())
}

/// Whether a viewer of the machine has the lock now.
pub fn held(context: &Context, slot: Slot) -> io::Result<bool> {
    Ok(take(context, slot)?.is_none())
}
