//! `guesthouse --app`: one step from a click to a program's window. It starts the machine if it is off,
//! waits for the helper in the guest to answer, starts the program there, and makes sure the machine
//! has one viewer to show its windows. The viewer is another process, which the caller knows how to
//! start and this does not, so it is given as a function.
//!
//! Nothing here sleeps for a set time. Each wait ends on what it waits for: the machine running, the
//! helper answering, the viewer holding its lock. A wait that has a limit says what it waited for.

use super::app::{launch_of, start_in};
use super::pick::find;
use super::windows::running_slot;
use crate::app_windows::viewer_lock;
use crate::app_windows::{Launch, connect_side, is_on, off_text};
use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::machine::{Kind, Machine};
use crate::tasks::{State, status};
use guesthouse_daemon::slot::Slot;
use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often a wait looks again.
const LOOK: Duration = Duration::from_millis(200);
/// How long a machine that is starting, which something else started, is waited for.
const START_WAIT: Duration = Duration::from_secs(15 * 60);

/// A viewer process that was just started.
pub trait ViewerProcess {
    /// Why it has ended, if it has.
    fn ended(&mut self) -> Option<String>;
}

/// What the caller says: the program to open in the machine, and how to start a viewer for it.
pub struct Opening<'a> {
    /// A machine's id or name.
    pub machine: &'a str,
    pub program: &'a str,
    /// The program is the id of a Start menu shortcut and not a path.
    pub start_menu: bool,
}

/// Starts a viewer for the machine, as a process of its own that outlives this one.
pub type StartViewer<'a> = &'a mut dyn FnMut(&Machine) -> Result<Box<dyn ViewerProcess>>;

pub fn open_program(
    context: &Context,
    opening: &Opening,
    start_viewer: StartViewer,
    out: &mut dyn Write,
) -> Result<()> {
    let machine = find(context, opening.machine)?;
    let name = &machine.name;
    // Before anything is started: a machine that cannot do this is not turned on for it.
    if machine.kind != Kind::Windows {
        return Err(Failure(format!(
            "{name} is not a Windows machine, and only Windows has a helper for this."
        )));
    }
    if !is_on(context, &machine) {
        return Err(Failure(off_text(&machine)));
    }
    bring_up(context, &machine, out)?;
    let slot = running_slot(context, &machine, "app windows")?;
    launch_when_ready(context, &machine, &launch_of(opening.program, opening.start_menu), out)?;
    ensure_viewer(context, &machine, slot, start_viewer, out)
}

/// Starts the machine if it is off, and waits until it is running. A machine another start is
/// bringing up is waited for.
fn bring_up(context: &Context, machine: &Machine, out: &mut dyn Write) -> Result<()> {
    let name = &machine.name;
    let until = Instant::now() + START_WAIT;
    loop {
        match status(context, machine).state {
            State::Running => return Ok(()),
            State::Off => {
                // A start that fails because another one is under way is not a failure: that other
                // one is waited for. Anything else is.
                if let Err(failure) = super::start(context, machine, true, out) {
                    if status(context, machine).state == State::Off {
                        return Err(failure);
                    }
                } else if status(context, machine).state == State::Off {
                    return Err(Failure(format!("{name} did not start.")));
                }
            }
            State::Starting => {
                if Instant::now() >= until {
                    return Err(Failure(format!(
                        "{name} was still starting after {} minutes.",
                        START_WAIT.as_secs() / 60
                    )));
                }
                context.clock.sleep(LOOK);
            }
            other => {
                let text =
                    format!("{name} is {}, so wait for it to be running.", other.name().to_lowercase());
                return Err(Failure(text));
            }
        }
    }
}

/// Waits for the helper and starts the program. A guest that has only just started has no session or
/// no agent for a while, and that is waited out; a refusal of any other kind is not.
fn launch_when_ready(
    context: &Context,
    machine: &Machine,
    launch: &Launch,
    out: &mut dyn Write,
) -> Result<()> {
    let slot = running_slot(context, machine, "app windows")?;
    let until = Instant::now() + context.app_windows.patience;
    loop {
        let mut link = connect_side(&Arc::new(context.clone()), machine, Some(slot))?;
        match link.wait_ready() {
            Ok(()) => return start_in(context, machine, &mut link, launch, out),
            Err(failure) if link.start_may_pass() && Instant::now() < until => {
                drop(failure);
                drop(link);
                context.clock.sleep(LOOK);
            }
            Err(failure) => return Err(failure),
        }
    }
}

/// One viewer for the machine: none is started if one holds the lock.
fn ensure_viewer(
    context: &Context,
    machine: &Machine,
    slot: Slot,
    start_viewer: StartViewer,
    out: &mut dyn Write,
) -> Result<()> {
    let name = &machine.name;
    let held = |context: &Context| viewer_lock::held(context, slot).map_err(Failure::from);
    if held(context)? {
        return Ok(writeln!(out, "{name}'s app windows are already shown by the viewer that is open.")?);
    }
    let mut viewer = start_viewer(machine)?;
    let until = Instant::now() + context.app_windows.patience;
    loop {
        // A viewer that lost the race ends at once, and the lock is then the other one's: that is a
        // viewer all the same.
        if held(context)? {
            return Ok(writeln!(out, "Opened the app windows viewer for {name}.")?);
        }
        if let Some(why) = viewer.ended() {
            return Err(Failure(format!(
                "The app windows viewer for {name} ended before it showed anything: {why}. \
                 guesthouse --app-windows {} says why on its own.",
                machine.id
            )));
        }
        if Instant::now() >= until {
            return Err(Failure(format!("The app windows viewer for {name} did not start.")));
        }
        context.clock.sleep(LOOK);
    }
}
