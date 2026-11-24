//! `windows`: the app windows a running Windows guest has, from the helper in its user's session.
//! Only a machine that has app windows on is asked. A helper that is not running is started for the
//! look and ended after, so no helper that a look started is left running.

use crate::app_windows::{MachineLink, connect, describe, is_on, off_text};
use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::machine::{Kind, Machine};
use crate::slots;
use crate::tasks::{State, status};
use guesthouse_daemon::slot::Slot;
use std::io::Write;
use std::sync::Arc;

/// What a command that talks to the helper needs first: a running Windows machine with app windows
/// on. Nothing is sent to the guest before these are known.
pub(super) fn running_slot(context: &Context, machine: &Machine, what: &str) -> Result<Slot> {
    let name = &machine.name;
    if machine.kind != Kind::Windows {
        return Err(Failure(format!(
            "{name} is not a Windows machine, and only Windows has a helper for this."
        )));
    }
    let found = status(context, machine);
    let Some(slot) = found.slot else {
        return Err(Failure(format!("{name} is not running, so it has no {what}.")));
    };
    if found.state != State::Running {
        let text = format!("{name} is {}, so wait for it to be running.", found.state.name().to_lowercase());
        return Err(Failure(text));
    }
    if !is_on(context, machine) {
        return Err(Failure(off_text(machine)));
    }
    Ok(slots::number(slot))
}

pub(super) fn run(context: &Context, machine: &Machine, json: bool, out: &mut dyn Write) -> Result<()> {
    let slot = running_slot(context, machine, "windows")?;
    let mut link = connect(&Arc::new(context.clone()), machine, Some(slot))?;
    let result = look(&mut link, machine, json, out);
    link.end_if_started();
    result
}

fn look(link: &mut MachineLink, machine: &Machine, json: bool, out: &mut dyn Write) -> Result<()> {
    link.wait_ready()?;
    if json {
        let line = link.client.last_layout_line().unwrap_or_default();
        return Ok(writeln!(out, "{}", line.trim_end())?);
    }
    let model = link.client.model();
    if model.cells().is_empty() {
        return Ok(writeln!(out, "{} has no app windows open.", machine.name)?);
    }
    for cell in model.cells() {
        writeln!(out, "{}", describe(cell))?;
    }
    Ok(())
}
