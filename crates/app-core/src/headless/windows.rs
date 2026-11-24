//! `windows`: the windows a running Windows guest has open, from the helper in its user's session.
//! It only reads: the helper is started for the question if it isn't running, and ended after.

use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::guest_windows::{describe, read_guest_windows};
use crate::machine::{Kind, Machine};
use crate::slots;
use crate::tasks::{State, status};
use std::io::Write;

pub(super) fn run(context: &Context, machine: &Machine, json: bool, out: &mut dyn Write) -> Result<()> {
    let name = &machine.name;
    if machine.kind != Kind::Windows {
        return Err(Failure(format!(
            "{name} is not a Windows machine, and only Windows has a helper for this."
        )));
    }
    let found = status(context, machine);
    let Some(slot) = found.slot else {
        return Err(Failure(format!("{name} is not running, so it has no windows.")));
    };
    if found.state != State::Running {
        let text = format!("{name} is {}, so wait for it to be running.", found.state.name().to_lowercase());
        return Err(Failure(text));
    }
    let listing = read_guest_windows(context, machine, Some(slots::number(slot)))?;
    if json {
        return Ok(writeln!(out, "{}", listing.json.trim_end())?);
    }
    if listing.windows.is_empty() {
        return Ok(writeln!(out, "{name} has no windows open.")?);
    }
    for window in &listing.windows {
        writeln!(out, "{}", describe(window))?;
    }
    Ok(())
}
