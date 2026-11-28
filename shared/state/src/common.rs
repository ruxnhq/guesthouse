//! Where a machine stands, on any host, and the word each state is shown with. These are the Linux
//! host's five states and words, and two more that a host may need and the Linux host never has:
//! a shutdown that has stalled, and a machine that failed. The Windows host's states are these too,
//! and go over its pipe by the names below.
//!
//! A failed machine's reason isn't part of its state: each host carries it beside the state, as it
//! does any other problem, so a state is a plain value every host can copy and compare.

use serde::{Deserialize, Serialize};

/// Where a machine stands. Each has a name on the wire, lower case and never changed: a new name
/// reaches an app of another version as one it can't read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum State {
    /// Not running, and nothing being done to it.
    #[default]
    #[serde(rename = "off")]
    Off,
    #[serde(rename = "starting")]
    Starting,
    /// Its system is being put on its disk, before it can be started as usual.
    #[serde(rename = "installing")]
    Installing,
    #[serde(rename = "running")]
    Running,
    /// Asked to shut down, or being turned off.
    #[serde(rename = "stopping")]
    Stopping,
    /// Asked to shut down, and still not off after the time it should take. Turning it off at
    /// once is what's left.
    #[serde(rename = "stopping_stalled")]
    StoppingStalled,
    /// It couldn't start, or it stopped without being asked to. Like `Off`, it can be started.
    #[serde(rename = "failed")]
    Failed,
}

impl State {
    /// The word the state is shown with. A sentence that says it mid-way puts it in lower case.
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Starting => "Starting",
            Self::Installing => "Installing",
            Self::Running => "Running",
            Self::Stopping => "Stopping",
            Self::StoppingStalled => "Still stopping",
            Self::Failed => "Failed",
        }
    }
}

#[cfg(test)]
mod tests;
