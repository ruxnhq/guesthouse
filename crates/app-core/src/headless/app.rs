//! `app`: starts a program in a running Windows guest by its path there. Opening it as a window of
//! its own on this desktop is the viewer's job. This only launches it in the guest.

use super::windows::running_slot;
use crate::app_windows::{Launch, Request, connect_side};
use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::machine::Machine;
use std::io::Write;
use std::sync::Arc;

pub(super) fn run(context: &Context, machine: &Machine, program: &str, out: &mut dyn Write) -> Result<()> {
    let slot = running_slot(context, machine, "app windows")?;
    let name = &machine.name;
    let mut link = connect_side(&Arc::new(context.clone()), machine, Some(slot))?;
    link.wait_ready()?;
    // The helper stays running: what it starts is shown by it.
    let request = Request::Launch(Launch::Path { path: program.to_owned(), args: String::new() });
    let patience = context.app_windows.timing.request;
    let answer = link
        .client
        .ask(request, patience)
        .map_err(|failure| Failure(format!("{name} did not take {program}: {failure}.")))?;
    if !answer.ok {
        let why = if answer.error.is_empty() { "it gave no reason" } else { &answer.error };
        return Err(Failure(format!("{name} could not start {program}: {why}.")));
    }
    Ok(writeln!(out, "{program} was started in {name}.")?)
}
