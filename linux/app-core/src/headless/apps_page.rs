//! What the Apps page of a machine asks of its helper: the programs of the Start menu, with their icons,
//! and the windows that are open, in one look. It is the same side connection that `apps` and `windows`
//! use, so a viewer that is open keeps its own connection. A helper that was not running is started for
//! the look and ended after.

use super::windows::running_slot;
use crate::app_windows::{Found, Look, Request, connect_side, is_on, off_text};
use crate::context::Context;
use crate::machine::Machine;
use std::sync::Arc;

/// Asks a running machine for its programs and its open windows. It waits for the helper as long as
/// the context says, so it belongs on a thread of its own. A machine that is not running, or has the
/// switch off, is not asked, and that is a failure with the reason in words.
pub fn look(context: &Context, machine: &Machine) -> Look {
    let name = &machine.name;
    if !is_on(context, machine) {
        return Look::Failed(off_text(machine));
    }
    let slot = match running_slot(context, machine, "app windows") {
        Ok(slot) => slot,
        Err(failure) => return Look::Failed(format!("{failure}")),
    };
    let mut link = match connect_side(&Arc::new(context.clone()), machine, Some(slot)) {
        Ok(link) => link,
        Err(failure) => return Look::Failed(format!("{failure}")),
    };
    let result = match link.wait_ready() {
        // A guest that has only just started has no session or no agent for a while, and that is
        // waited out by asking again.
        Err(failure) if link.start_may_pass() => Look::NotYet(format!("{failure}")),
        Err(failure) => Look::Failed(format!("{failure}")),
        Ok(()) => {
            let asked =
                link.client.ask(Request::Apps { icons: true }, context.app_windows.timing.apps_request);
            match asked {
                Err(failure) => Look::Failed(format!("{name} did not answer: {failure}.")),
                Ok(answer) if !answer.ok => {
                    let why = if answer.error.is_empty() { "it gave no reason" } else { &answer.error };
                    Look::Failed(format!("{name} said: {why}."))
                }
                Ok(answer) => {
                    let windows = link.client.model().windows().cloned().collect();
                    Look::Listed(Found { apps: answer.apps.unwrap_or_default(), windows })
                }
            }
        }
    };
    link.end_if_started();
    result
}
