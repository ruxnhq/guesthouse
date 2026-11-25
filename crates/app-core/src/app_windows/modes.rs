//! The two ways to look at a machine, as exclusive modes. A machine's frames and input go to one viewer
//! at a time, so the whole screen (in the app's window) and the app windows (the viewer process of
//! `guesthouse --app-windows`) cannot both be open: each takes the connection from the other, over and
//! over. While the guest's helper runs, the Windows screen is only an overlay of packed cells too.
//!
//! The screen's side holds a lock (`ScreenMark`) for as long as its page is open, as the app windows
//! viewer holds its own (`viewer_lock`). Whoever wants the other mode asks for it in this order, and
//! never connects before it is done: the screen takes its mark, which keeps a new app windows viewer
//! out, ends the viewer that runs, ends the helper in the guest so that its changes are taken back as
//! far as it can (see [`end_helper`]), and only then connects.
//!
//! The viewer is asked to end with SIGTERM, which it takes as it does a terminal's: it lets go of held
//! keys and buttons and closes its windows. Its process id is in its lock file, and it is signalled only
//! if the system shows that process is a `--app-windows` viewer of this machine, as a number in a file
//! that a crashed viewer left behind may belong to anything by now.

use super::HELPER;
use super::viewer_lock;
use crate::agent::stop_helper;
use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::machine::Machine;
use guesthouse_daemon::slot::Slot;
use rustix::fs::{FlockOperation, flock};
use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

/// The argument that starts the viewer, as `guesthouse --app-windows NAME`.
const VIEWER_ARGUMENT: &str = "--app-windows";
const SCREEN_FILE: &str = "screen-viewer.lock";
const PRIVATE: u32 = 0o600;
/// How long a viewer that was asked to end has to let go of the machine. It closes its windows and
/// releases what it holds first.
pub const END_WAIT: Duration = Duration::from_secs(10);
const LOOK: Duration = Duration::from_millis(100);

/// Held while the app's screen page of a machine is open.
pub struct ScreenMark(#[allow(dead_code)] File);

fn take_screen_in(run_folder: &Path) -> io::Result<Option<ScreenMark>> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(PRIVATE)
        .open(run_folder.join(SCREEN_FILE))?;
    Ok(flock(&file, FlockOperation::NonBlockingLockExclusive).is_ok().then_some(ScreenMark(file)))
}

/// Marks the machine's screen as open, or says `None` if it already is. The run folder is made.
pub fn mark_screen(context: &Context, slot: Slot) -> io::Result<Option<ScreenMark>> {
    context.make_run_folder(Some(slot))?;
    // A look at the lock holds it shared for an instant, which is not a screen that is open.
    for time in 0..viewer_lock::TRIES {
        if time > 0 {
            std::thread::sleep(viewer_lock::TRY_AGAIN);
        }
        if let Some(mark) = take_screen_in(&context.run_folder(Some(slot)))? {
            return Ok(Some(mark));
        }
    }
    Ok(None)
}

/// Whether the machine's whole screen is open in the app now.
pub fn screen_open(context: &Context, slot: Slot) -> io::Result<bool> {
    context.make_run_folder(Some(slot))?;
    viewer_lock::held_file(&context.run_folder(Some(slot)).join(SCREEN_FILE))
}

/// The same, asked again a moment later when it says yes. A child this process forks holds a copy of
/// the mark until it becomes its own program, so a yes that is to end something is not taken at the
/// first look.
pub fn screen_open_settled(context: &Context, slot: Slot) -> bool {
    (0..3).all(|time| {
        if time > 0 {
            std::thread::sleep(Duration::from_millis(150));
        }
        screen_open(context, slot).unwrap_or(false)
    })
}

/// What a command says when the screen is in the way of app windows.
pub fn screen_open_text(machine: &Machine) -> String {
    format!(
        "The screen of {} is open in Guesthouse, and a machine shows one or the other. Close the screen \
         and open the app again.",
        machine.name
    )
}

/// Whether `pid` is a viewer of app windows for this machine, by what `proc` (the system's /proc) says.
pub fn is_viewer_of(proc: &Path, pid: u32, machine: &Machine) -> bool {
    let folder = proc.join(pid.to_string());
    let comm = std::fs::read_to_string(folder.join("comm")).unwrap_or_default();
    let Ok(cmdline) = std::fs::read(folder.join("cmdline")) else { return false };
    let words: Vec<&[u8]> = cmdline.split(|byte| *byte == 0).collect();
    let asks_for_it = matches!(words.get(1), Some(word) if *word == VIEWER_ARGUMENT.as_bytes());
    let this_machine = matches!(words.get(2), Some(word) if *word == machine.id.as_bytes() || *word == machine.name.as_bytes());
    comm.trim().starts_with("guesthouse") && asks_for_it && this_machine
}

/// What the end needs from the system, so that a test needs no process of its own.
pub struct Ending<'a> {
    /// The system's /proc.
    pub proc: &'a Path,
    /// Asks a process to end (SIGTERM).
    pub terminate: &'a dyn Fn(u32) -> io::Result<()>,
    /// How long the viewer has to let go once it was asked.
    pub wait: Duration,
}

/// SIGTERM to a process, as the real ending.
pub fn terminate(pid: u32) -> io::Result<()> {
    use rustix::process::{Pid, Signal, kill_process};
    let pid = i32::try_from(pid).ok().and_then(Pid::from_raw).ok_or(io::ErrorKind::InvalidInput)?;
    kill_process(pid, Signal::TERM).map_err(io::Error::from)
}

/// Ends the helper in the guest if there is one, as `guesthouse-vm windows` does for one it started.
/// As it quits, the helper puts each window it moved back where it was, if the window is still there
/// and is neither minimized nor maximized, and puts Windows' scale back to what it was before the host
/// first asked for another. A helper that is killed, not asked, puts nothing back.
pub fn end_helper(context: &Context, machine: &Machine, slot: Slot) {
    stop_helper(context, machine, Some(slot), HELPER);
}

/// Gets the machine ready for its whole screen: the app windows viewer, if one runs, is asked to end and
/// waited for, and the helper in the guest is ended so that the guest's windows are as Windows had
/// them. Nothing is done for a machine whose app windows are not open.
pub fn free_for_screen(context: &Context, machine: &Machine, slot: Slot, ending: &Ending) -> Result<()> {
    let name = &machine.name;
    let held = || viewer_lock::held(context, slot).map_err(Failure::from);
    if !held()? {
        return Ok(());
    }
    let run_folder = context.run_folder(Some(slot));
    let pid = viewer_lock::recorded_pid(&run_folder)
        .filter(|pid| is_viewer_of(ending.proc, *pid, machine))
        .ok_or_else(|| {
        Failure(format!(
            "The app windows of {name} are open in a viewer that Guesthouse could not tell to end. \
                 Close its windows and its program (guesthouse --app-windows {}), then show the screen.",
            machine.id
        ))
    })?;
    (ending.terminate)(pid)
        .map_err(|error| Failure(format!("The app windows of {name} could not be asked to end: {error}.")))?;
    let until = Instant::now() + ending.wait;
    while held()? {
        if Instant::now() >= until {
            return Err(Failure(format!(
                "The app windows of {name} did not end in {} seconds, so the screen was not opened.",
                ending.wait.as_secs()
            )));
        }
        context.clock.sleep(LOOK);
    }
    end_helper(context, machine, slot);
    Ok(())
}
