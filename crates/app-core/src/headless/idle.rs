//! `idle`: reads how busy a running machine is and says whether it would count as idle. It only reads.

use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::idle::{Limits, describe, limits_text, take};
use crate::machine::{Kind, Machine};
use crate::tasks::{State, status};
use std::io::Write;

/// How the machine is called in the lines for what can't be read.
fn guest_name(machine: &Machine) -> &'static str {
    match machine.kind {
        Kind::Windows => "a Windows guest",
        Kind::Linux => "an Ubuntu guest",
        Kind::Plain if machine.macos() => "a macOS guest",
        Kind::Plain => "a plain VM",
    }
}

pub(super) fn run(
    context: &Context,
    machine: &Machine,
    minutes: Option<&str>,
    out: &mut dyn Write,
) -> Result<()> {
    let limits = match minutes {
        None => Limits::default(),
        Some(text) => match text.parse::<u64>() {
            Ok(minutes) if minutes > 0 => Limits::new(minutes),
            _ => return Err(Failure(format!("--minutes is a number of minutes, not {text}."))),
        },
    };
    let name = &machine.name;
    let found = status(context, machine);
    let (Some(slot), Some(qemu)) = (found.slot, found.qemu) else {
        return Err(Failure(format!("{name} is not running, so there is nothing to read.")));
    };
    if found.state != State::Running {
        let text = format!("{name} is {}, so wait for it to be running.", found.state.name().to_lowercase());
        return Err(Failure(text));
    }
    writeln!(out, "Reading {name}, which takes a few seconds.")?;
    out.flush()?;
    let reading = take(context, machine, slot, qemu.pid);
    for line in describe(&reading, &limits, guest_name(machine)) {
        writeln!(out, "{line}")?;
    }
    writeln!(out, "\n{}", limits_text(&limits))?;
    Ok(())
}
