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
use std::time::Duration;

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

/// The same for a run folder that is already made. A look by [`held_in`] holds the lock shared for an
/// instant, which is not a viewer, so a lock found taken is tried again a few times before it counts.
pub fn take_in(run_folder: &Path) -> io::Result<Option<ViewerLock>> {
    let file = open(run_folder)?;
    for time in 0..TRIES {
        if time > 0 {
            std::thread::sleep(TRY_AGAIN);
        }
        if flock(&file, FlockOperation::NonBlockingLockExclusive).is_ok() {
            return Ok(Some(ViewerLock(file)));
        }
    }
    Ok(None)
}

/// How often a lock found taken is tried again, and how long apart: longer than any look holds it.
pub const TRIES: u32 = 5;
pub const TRY_AGAIN: Duration = Duration::from_millis(60);

fn open(run_folder: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).truncate(false).write(true).mode(PRIVATE).open(run_folder.join(LOCK_FILE))
}

/// Whether some process holds this lock now. The look takes it shared, and only for an instant: two
/// looks at once both find it free, and a viewer that starts meanwhile tries again (see [`take_in`]).
pub fn held_file(file: &Path) -> io::Result<bool> {
    let file = OpenOptions::new().create(true).truncate(false).write(true).mode(PRIVATE).open(file)?;
    Ok(flock(&file, FlockOperation::NonBlockingLockShared).is_err())
}

/// Whether a viewer has the lock in this run folder now.
pub fn held_in(run_folder: &Path) -> io::Result<bool> {
    held_file(&run_folder.join(LOCK_FILE))
}

/// Whether a viewer of the machine has the lock now.
pub fn held(context: &Context, slot: Slot) -> io::Result<bool> {
    context.make_run_folder(Some(slot))?;
    held_in(&context.run_folder(Some(slot)))
}
