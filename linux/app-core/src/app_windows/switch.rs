//! The per-machine switch for app windows. It is off unless someone turns it on, and while it is off
//! nothing about the machine changes: no forward is made for the helper, no helper is started, and
//! the commands for it refuse. It is kept in a file of its own beside the machine's settings, as
//! Remote Desktop's and lean Windows' choices are, so that an older build reads the machine as it
//! always did and a machine made before it has none.

use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::machine::{Kind, Machine};
use crate::settings::write_json;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;

pub const SWITCH_FILE: &str = "app-windows.json";
/// What `guesthouse-vm set` calls it.
pub const SETTING: &str = "app-windows";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Chosen {
    on: bool,
}

fn switch_file(context: &Context, machine: &Machine) -> PathBuf {
    machine.folder(context).join(SWITCH_FILE)
}

/// Whether app windows is on for the machine. A file that cannot be read is off, which leaves the
/// machine as it is.
pub fn is_on(context: &Context, machine: &Machine) -> bool {
    if machine.kind != Kind::Windows {
        return false;
    }
    let text = fs::read_to_string(switch_file(context, machine)).unwrap_or_default();
    serde_json::from_str::<Chosen>(&text).is_ok_and(|chosen| chosen.on)
}

/// Keeps the choice. Off leaves no file, so a machine that never had it looks as one always did.
pub fn set_on(context: &Context, machine: &Machine, on: bool) -> Result<()> {
    if machine.kind != Kind::Windows {
        return Err(Failure(format!("{SETTING} is only for a Windows machine with GPU-PV.")));
    }
    if !on {
        return match fs::remove_file(switch_file(context, machine)) {
            Err(error) if error.kind() != ErrorKind::NotFound => Err(error.into()),
            _ => Ok(()),
        };
    }
    write_json(&switch_file(context, machine), &Chosen { on })
}

/// What a command says to a machine that has it off.
pub fn off_text(machine: &Machine) -> String {
    format!(
        "App windows is off for {}. guesthouse-vm set {} {SETTING} on turns it on, and the machine \
         gets what it needs at its next start.",
        machine.name, machine.id
    )
}
