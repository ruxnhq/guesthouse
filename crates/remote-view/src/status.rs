//! What remoteman's host says to `ctl status`: one line of JSON, read into a struct.
//!
//! Only what the controls use is kept. A field the host adds later is ignored, and a field an
//! older host lacks reads as the host would have meant it: nothing pending, nobody connected,
//! every permission allowed. Fields that give the host-wide clipboard and files are optional, as
//! "not said" must stay apart from "off".

use crate::ctl::STOPPING;
use crate::share::{DeviceInfo, Pending};
use serde::Deserialize;

/// The viewer connected now.
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Client {
    #[serde(default)]
    pub name: String,
    /// Where the device put this machine's screen: `extend`, `main`, `mirror`, or `only`, which takes
    /// the guest's own monitor off its desktop for as long as the device stays.
    #[serde(default)]
    pub placement: Option<String>,
}

/// The host's answer to `status`.
#[derive(Deserialize, Debug, Clone, Default, PartialEq, Eq)]
#[serde(default)]
pub struct Status {
    /// The host's own name, which viewers are shown.
    pub name: Option<String>,
    /// The host's fingerprint, which a viewer compares.
    pub fingerprint: Option<String>,
    /// Pairing is open: the next unknown device is let come and wait.
    pub pairing: bool,
    /// The device that came and waits for an answer, with its code.
    pub pending: Option<Pending>,
    /// The viewer connected now.
    pub client: Option<Client>,
    /// The devices the host trusts.
    pub devices: Vec<DeviceInfo>,
    /// Sharing is switched off: every device is turned away.
    pub paused: bool,
    /// What the host serves every device, before each device's own choice. `None`: not said.
    pub clipboard: Option<bool>,
    pub files: Option<bool>,
    /// How the host looks for a newer remoteman by itself, from 0.1.9. `None`: an older host, or one
    /// the service didn't start, which says `null`.
    pub update: Option<Update>,
}

/// What the host says of looking for updates by itself.
#[derive(Deserialize, Debug, Clone, Default, PartialEq, Eq)]
#[serde(default)]
pub struct Update {
    /// `off`, `check` or `install`.
    pub mode: Option<String>,
}

/// What came back from asking for the status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Status(Box<Status>),
    /// The host is shutting down and has no answer.
    Stopping,
    /// Anything else, in the host's words or the parser's own.
    Error(String),
}

/// Reads what the program printed. The first line that opens a JSON object is the status, so a
/// warning printed before it by the shell does not matter; with none, the text is the host's reason.
pub fn parse_reply(output: &str) -> Reply {
    let text = output.trim();
    if text.eq_ignore_ascii_case(STOPPING) {
        return Reply::Stopping;
    }
    let Some(line) = text.lines().map(str::trim).find(|line| line.starts_with('{')) else {
        return Reply::Error(if text.is_empty() {
            "The host said nothing.".to_string()
        } else {
            text.to_string()
        });
    };
    match serde_json::from_str::<Status>(line) {
        Ok(status) => Reply::Status(Box::new(status)),
        Err(error) => Reply::Error(format!("The host's status could not be read: {error}")),
    }
}

impl Status {
    /// The device with this fingerprint.
    pub fn device(&self, fingerprint: &str) -> Option<&DeviceInfo> {
        self.devices.iter().find(|device| device.fingerprint.eq_ignore_ascii_case(fingerprint))
    }

    /// The host said it never looks for updates by itself.
    pub fn updates_off(&self) -> bool {
        self.update.as_ref().and_then(|update| update.mode.as_deref()) == Some("off")
    }
}

#[cfg(test)]
mod tests;
