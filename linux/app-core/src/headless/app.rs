//! `app`: starts a program in a running Windows guest, by its path there or by the id of its Start menu
//! shortcut. Opening it as a window of its own on this desktop is the viewer's job (`guesthouse --app`
//! does both in one step). This only launches it in the guest.

use super::windows::running_slot;
use crate::app_windows::{Launch, MachineLink, Request, connect_side};
use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::machine::Machine;
use std::io::Write;
use std::sync::Arc;

pub(super) fn run(
    context: &Context,
    machine: &Machine,
    program: &str,
    start_menu: bool,
    out: &mut dyn Write,
) -> Result<()> {
    let slot = running_slot(context, machine, "app windows")?;
    let mut link = connect_side(&Arc::new(context.clone()), machine, Some(slot))?;
    link.wait_ready()?;
    // The helper stays running: what it starts is shown by it.
    start_in(context, machine, &mut link, &launch_of(program, start_menu), out)
}

/// What a program word asks the helper to start.
pub(super) fn launch_of(program: &str, start_menu: bool) -> Launch {
    if start_menu {
        Launch::App(program.to_owned())
    } else {
        Launch::Path { path: program.to_owned(), args: String::new() }
    }
}

/// Asks a link that is ready to start the program, and says it was started.
pub(super) fn start_in(
    context: &Context,
    machine: &Machine,
    link: &mut MachineLink,
    launch: &Launch,
    out: &mut dyn Write,
) -> Result<()> {
    let name = &machine.name;
    let program = match launch {
        Launch::App(id) => id,
        Launch::Path { path, .. } => path,
    };
    let patience = context.app_windows.timing.request;
    let answer = link
        .client
        .ask(Request::Launch(launch.clone()), patience)
        .map_err(|failure| Failure(format!("{name} did not take {program}: {failure}.")))?;
    if !answer.ok {
        let why = if answer.error.is_empty() { "it gave no reason" } else { &answer.error };
        return Err(Failure(format!("{name} could not start {program}: {why}.")));
    }
    Ok(writeln!(out, "{program} was started in {name}.")?)
}
