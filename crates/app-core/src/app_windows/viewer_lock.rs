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
use std::path::PathBuf;

const LOCK_FILE: &str = "app-windows-viewer.lock";
const PRIVATE: u32 = 0o600;

/// The viewer's lock for the machine that runs in this slot.
pub fn lock_file(context: &Context, slot: Slot) -> PathBuf {
    context.run_folder(Some(slot)).join(LOCK_FILE)
}

/// Held by the one viewer of a machine.
pub struct ViewerLock(#[allow(dead_code)] File);

/// Takes the lock, or says `None` if a viewer of this machine already has it.
pub fn take(context: &Context, slot: Slot) -> io::Result<Option<ViewerLock>> {
    context.make_run_folder(Some(slot))?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(PRIVATE)
        .open(lock_file(context, slot))?;
    Ok(flock(&file, FlockOperation::NonBlockingLockExclusive).is_ok().then_some(ViewerLock(file)))
}

/// Whether a viewer of the machine has the lock now.
pub fn held(context: &Context, slot: Slot) -> io::Result<bool> {
    Ok(take(context, slot)?.is_none())
}
