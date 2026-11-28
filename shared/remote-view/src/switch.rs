//! The per-machine switch for viewing from other devices, and the port a machine gets.
//!
//! The switch is off unless someone turns it on, and while it is off nothing about the machine
//! changes: nothing is installed in it and nothing listens. It is kept in a file of its own beside
//! the machine's other settings, so that a build without it reads the machine as it always did and
//! a machine made before it has none.
//!
//! This is the contents of that file and the choices made about it. Reading and writing it is the
//! host's, which knows where machines are kept. A host that keeps the two values somewhere else,
//! such as in its machine's own settings, takes and gives them as they are with [`Switch::new`] and
//! [`Switch::values`], and the choices are the same.

use crate::share::{FIRST_PORT, LAST_PORT};
use serde::{Deserialize, Serialize};

/// The file's name, in the machine's folder.
pub const SWITCH_FILE: &str = "remote-view.json";

/// What the file holds. A port stays after the switch goes off, so the address a device has kept
/// is the same when it is turned on again.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Switch {
    #[serde(default)]
    pub allow_remote_view: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_port: Option<u16>,
}

/// The port a machine is given: the first in the range that no other machine has and that
/// `is_free` accepts. The range holds 108 ports, one for each machine that can be shared.
pub fn choose_port(taken: &[u16], is_free: impl Fn(u16) -> bool) -> Option<u16> {
    (FIRST_PORT..=LAST_PORT).find(|port| !taken.contains(port) && is_free(*port))
}

/// The ports the other machines have, which a machine's switch can't be given: the port `port` reads
/// for each of `machines` that `is_own` doesn't say is the machine itself, in their order. `port` is
/// asked only of the others. The Linux host reads each machine's file, the Windows service the
/// switches it holds.
pub fn others_ports<M>(
    machines: impl IntoIterator<Item = M>,
    is_own: impl Fn(&M) -> bool,
    port: impl Fn(&M) -> Option<u16>,
) -> Vec<u16> {
    machines.into_iter().filter(|machine| !is_own(machine)).filter_map(|machine| port(&machine)).collect()
}

/// What is said when every port is taken.
pub const NO_PORT: &str = "No port is free for sharing the screen (7792 to 7899 are all in use).";

impl Switch {
    /// The switch from its two values, wherever they were kept. A port outside the range is no
    /// port: a new one is chosen when the switch is next turned on.
    pub fn new(allow_remote_view: bool, remote_port: Option<u16>) -> Switch {
        let remote_port = remote_port.filter(|port| (FIRST_PORT..=LAST_PORT).contains(port));
        Switch { allow_remote_view, remote_port }
    }

    /// The two values to keep: whether it is on, and its port.
    pub fn values(&self) -> (bool, Option<u16>) {
        (self.allow_remote_view, self.remote_port)
    }

    /// The switch from the file's text, which is `None` when there is no file. Anything that cannot
    /// be read is off, which leaves the machine as it is. A port outside the range is dropped, as
    /// [`Switch::new`] drops it.
    pub fn read(text: Option<&str>) -> Switch {
        let switch = text.and_then(|text| serde_json::from_str::<Switch>(text).ok()).unwrap_or_default();
        Switch::new(switch.allow_remote_view, switch.remote_port)
    }

    /// The switch turned on or off. Turning it on gives it a port the first time, chosen with
    /// `taken` (the other machines') and `is_free`; one it has is kept, and so is the port when it
    /// is turned off.
    pub fn turned(&self, on: bool, taken: &[u16], is_free: impl Fn(u16) -> bool) -> Result<Switch, String> {
        let remote_port = match (on, self.remote_port) {
            (true, None) => Some(choose_port(taken, is_free).ok_or(NO_PORT)?),
            (_, port) => port,
        };
        Ok(Switch { allow_remote_view: on, remote_port })
    }

    /// What goes in the file, or `None` when it should not be there: off with no port is as a
    /// machine that never had it.
    pub fn text(&self) -> Option<String> {
        if *self == Switch::default() {
            return None;
        }
        Some(serde_json::to_string_pretty(self).expect("a switch is JSON") + "\n")
    }
}

#[cfg(test)]
mod tests;
