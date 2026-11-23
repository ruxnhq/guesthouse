//! Lean Windows: a Windows guest without the apps Windows adds for every new account, and with a few
//! documented policies that keep programs from running in the background. It is a choice of its own,
//! off unless the user, or the recipe they pick, turns it on. An install with it off is exactly as it
//! was without it: no file, no line in the answer file, nothing on the disc.
//!
//! `list` is what it takes out and switches off. Before the account's first logon it runs from the
//! answer disc, in Setup's specialize pass, and once the install is up it runs again through the agent
//! (`actions/lean.rs`), to catch what reached the account anyway.

mod list;
mod record;

pub use list::{APPS, LEFT_ALONE, POLICIES, guest_data, what_it_does};
pub use record::{
    Outcome, RESULT_FILE, names_back, names_taken_out, read_outcome, state_text, titles, write_outcome,
};

use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::machine::Machine;
use crate::settings::write_json;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Beside `machine.json`, as `remote-access.json` is. A machine made without the choice has none.
pub const LEAN_FILE: &str = "lean.json";
/// The guest's script, and the list it reads, by these names on the answer disc.
pub const SCRIPT: &str = "lean.ps1";
pub const DATA_ON_DISC: &str = "lean.json";
/// Setup's specialize pass is the last moment before any account exists, so what it takes out is never
/// installed for the new account. The answer file's own commands come first.
const SPECIALIZE_END: &str = "      </RunSynchronous>";
const SPECIALIZE_COMMAND: &str = "        <RunSynchronousCommand wcm:action=\"add\"><Order>5</Order><Path>cmd /c for %i in (D E F G H) do if exist %i:\\lean.ps1 powershell -NoProfile -ExecutionPolicy Bypass -File %i:\\lean.ps1 -Data %i:\\lean.json</Path></RunSynchronousCommand>\n";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Chosen {
    lean: bool,
}

fn lean_file(context: &Context, machine: &Machine) -> PathBuf {
    machine.folder(context).join(LEAN_FILE)
}

/// Whether the machine was made lean. A file that can't be read is no, which leaves Windows as it is.
pub fn is_lean(context: &Context, machine: &Machine) -> bool {
    let text = fs::read_to_string(lean_file(context, machine)).unwrap_or_default();
    serde_json::from_str::<Chosen>(&text).is_ok_and(|chosen| chosen.lean)
}

/// Keeps the choice. Off leaves no file, so a machine made without it looks as one always did.
pub fn write_lean(context: &Context, machine: &Machine, lean: bool) -> Result<()> {
    if !lean {
        return match fs::remove_file(lean_file(context, machine)) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
            _ => Ok(()),
        };
    }
    write_json(&lean_file(context, machine), &Chosen { lean })
}

/// The filled answer file with one command more in the specialize pass, which runs the script on the
/// disc. The template is never changed, so an answer file without it is the same as before.
pub fn with_specialize_step(answers: &str) -> Result<String> {
    if answers.matches(SPECIALIZE_END).count() != 1 {
        return Err(Failure::new(
            "The answer file has no place for lean Windows' step, so nothing was installed.",
        ));
    }
    Ok(answers.replacen(SPECIALIZE_END, &format!("{SPECIALIZE_COMMAND}{SPECIALIZE_END}"), 1))
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
