//! Lean Windows: a Windows guest without the apps Windows adds for every new account, and with a few
//! documented policies that keep programs from running in the background. It is a choice of its own,
//! off unless the user, or the recipe they pick, turns it on. An install with it off is exactly as it
//! was without it: no file, no line in the answer file, nothing on the disc.
//!
//! `list` is what it takes out and switches off. Before the account's first logon it runs from the
//! answer disc, in Setup's specialize pass, and once the install is up it runs again through the
//! guest's agent, to catch what reached the account anyway. The line in the answer file that runs it
//! is in `guesthouse-answers-core` (`linux_host::lean`), and runs [`SCRIPT`] with [`DATA_ON_DISC`];
//! running it through the agent is the host's.

mod list;
mod record;

pub use list::{APPS, App, LEFT_ALONE, NOT_ADDED, POLICIES, Policy, guest_data, what_it_does};
pub use record::{
    Outcome, RESULT_FILE, names_back, names_taken_out, read_outcome, state_text, titles, write_outcome,
};

use crate::failure::Result;
use guesthouse_basics::json_file::write_json;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Beside `machine.json`, as `remote-access.json` is. A machine made without the choice has none.
pub const LEAN_FILE: &str = "lean.json";
/// The guest's script, and the list it reads, by these names on the answer disc.
pub const SCRIPT: &str = "lean.ps1";
pub const DATA_ON_DISC: &str = "lean.json";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Chosen {
    lean: bool,
}

fn lean_file(folder: &Path) -> PathBuf {
    folder.join(LEAN_FILE)
}

/// Whether the machine in `folder` was made lean. A file that can't be read is no, which leaves
/// Windows as it is.
pub fn is_lean(folder: &Path) -> bool {
    let text = fs::read_to_string(lean_file(folder)).unwrap_or_default();
    serde_json::from_str::<Chosen>(&text).is_ok_and(|chosen| chosen.lean)
}

/// Keeps the choice. Off leaves no file, so a machine made without it looks as one always did.
pub fn write_lean(folder: &Path, lean: bool) -> Result<()> {
    if !lean {
        return match fs::remove_file(lean_file(folder)) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
            _ => Ok(()),
        };
    }
    Ok(write_json(&lean_file(folder), &Chosen { lean })?)
}

/// An import's copies of the choice and of what lean Windows did, read as the app's own and written
/// out again, or left out. What it did keeps only names on the list, and none of what the other PC's
/// guest said: the first start here looks again.
pub fn remake_imported(folder: &Path) -> Result<()> {
    let choice = folder.join(LEAN_FILE);
    if let Ok(text) = fs::read_to_string(&choice) {
        match serde_json::from_str::<Chosen>(&text) {
            Ok(Chosen { lean: true }) => write_json(&choice, &Chosen { lean: true })?,
            _ => fs::remove_file(&choice)?,
        }
    }
    let result = folder.join(RESULT_FILE);
    if let Ok(text) = fs::read_to_string(&result) {
        let said: Outcome = serde_json::from_str(&text).unwrap_or_default();
        let on_list = |names: &[String]| -> Vec<String> {
            APPS.iter().map(|app| app.name.to_owned()).filter(|name| names.contains(name)).collect()
        };
        let kept = Outcome {
            taken_out: on_list(&said.taken_out),
            put_back: on_list(&said.put_back),
            still_back: None,
        };
        if kept == Outcome::default() {
            fs::remove_file(&result)?;
        } else {
            write_json(&result, &kept)?;
        }
    }
    Ok(())
}
