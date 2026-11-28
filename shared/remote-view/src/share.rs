//! Where a machine's screen sharing stands, in the words every host shows it in: the same field
//! names on the wire, the same states, the same reasons a pairing closed.
//!
//! These are plain data. Reading them from remoteman's `status` is `status.rs`, and working out
//! the next one is `pairing.rs` and `view.rs`.

use serde::{Deserialize, Serialize};

/// The first port tried for a machine's screen. 7791 is remoteman's own, so this starts after it
/// and each machine gets a port of its own that stays the same.
pub const FIRST_PORT: u16 = 7792;
pub const LAST_PORT: u16 = 7899;

/// How long a pairing stays open unless it is asked for another time.
pub const PAIRING_SECONDS: u64 = 120;
/// The times a pairing may be opened for, in seconds.
pub const MIN_PAIRING_SECONDS: u64 = 1;
pub const MAX_PAIRING_SECONDS: u64 = 3600;

/// What a device can be allowed or refused, as `device ... allow` takes it, with `input` standing
/// for control: its opposite is `view-only`.
pub const PERMISSIONS: [&str; 6] = ["input", "clipboard", "files", "sound", "mic", "camera"];

/// The seconds a pairing is opened for: the default when none is asked for, or why the number is
/// not allowed.
pub fn pairing_seconds(asked: Option<u64>) -> Result<u64, String> {
    let seconds = asked.unwrap_or(PAIRING_SECONDS);
    if (MIN_PAIRING_SECONDS..=MAX_PAIRING_SECONDS).contains(&seconds) {
        return Ok(seconds);
    }
    Err(format!("Pairing can stay open for {MIN_PAIRING_SECONDS} to {MAX_PAIRING_SECONDS} seconds."))
}

/// How long a host holds a new device before it turns it away, when nobody answers.
pub const CONFIRM_SECONDS: u64 = 60;

fn confirm_seconds() -> u64 {
    CONFIRM_SECONDS
}

/// A new device that waits for an answer, as remoteman's `status.pending` has it. Its `Debug` never
/// prints the name or the code, which never go to a log.
#[derive(Deserialize, Serialize, Clone, PartialEq, Eq)]
pub struct Pending {
    pub name: String,
    pub fingerprint: String,
    /// Six digits, "123 456", which the device shows too, for comparing.
    pub code: String,
    /// How long there is still to answer, when this was said.
    #[serde(default = "confirm_seconds")]
    pub seconds: u64,
}

/// The fingerprint and the seconds; what is left out is shown as `..`.
impl std::fmt::Debug for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pending")
            .field("fingerprint", &self.fingerprint)
            .field("seconds", &self.seconds)
            .finish_non_exhaustive()
    }
}

/// What a trusted device may do, as remoteman's `status.devices[].permissions`. A host that does
/// not say allows everything, so a missing field is `true`.
#[derive(Deserialize, Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(default)]
pub struct Permissions {
    pub input: bool,
    pub clipboard: bool,
    pub files: bool,
    pub sound: bool,
    pub mic: bool,
    pub camera: bool,
}

impl Default for Permissions {
    fn default() -> Self {
        Permissions { input: true, clipboard: true, files: true, sound: true, mic: true, camera: true }
    }
}

impl Permissions {
    /// One permission by its name in [`PERMISSIONS`].
    pub fn get(&self, what: &str) -> Option<bool> {
        Some(match what {
            "input" => self.input,
            "clipboard" => self.clipboard,
            "files" => self.files,
            "sound" => self.sound,
            "mic" => self.mic,
            "camera" => self.camera,
            _ => return None,
        })
    }
}

/// A device the host trusts, as remoteman's `status.devices[]`. What the host adds beyond these
/// fields is not kept.
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub fingerprint: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub connected: bool,
    #[serde(default)]
    pub permissions: Permissions,
}

/// Where sharing stands.
#[derive(Deserialize, Serialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ShareState {
    #[default]
    Off,
    Starting,
    Listening,
    Failed,
}

/// Why a pairing closed.
#[derive(Deserialize, Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CloseReason {
    /// The device that came was let in.
    Paired,
    /// The device that came was turned away.
    Declined,
    /// Nobody answered in time, or nobody came in time.
    Timeout,
    /// It was closed on request.
    Asked,
    /// Anything else. A reason this version does not know reads as this.
    #[serde(other)]
    Other,
}

/// Where a machine's screen sharing stands, as in a listing and as an event.
#[derive(Deserialize, Serialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct ShareInfo {
    pub machine: String,
    pub state: ShareState,
    pub port: Option<u16>,
    /// The host key's fingerprint, for the person to compare on the viewer.
    pub fingerprint: Option<String>,
    pub pairing: bool,
    /// When an open pairing closes, in milliseconds since 1970.
    pub pairing_until_ms: Option<u64>,
    /// The name of the viewer connected now.
    pub viewer: Option<String>,
    /// The name of the device that paired since pairing was last opened.
    pub paired: Option<String>,
    /// Why pairing last closed. Readers that do not know the field ignore it.
    #[serde(default)]
    pub closed_reason: Option<CloseReason>,
    /// A device is waiting to be accepted or declined. Then `pairing` is false, but pairing is
    /// not closed: the host turns it off when the device comes, and the device is its use.
    #[serde(default)]
    pub pending: Option<Pending>,
    /// The devices the host trusts, in the order they were paired.
    #[serde(default)]
    pub devices: Vec<DeviceInfo>,
    pub reason: Option<String>,
    /// Where the connected device put the screen, in remoteman's word for it. With `only` the
    /// machine's own monitor is off its desktop while the device stays, and a local window of it has
    /// no picture. Readers that do not know the field ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewer_placement: Option<String>,
}

/// remoteman's placement that takes the machine's own monitor away while the device stays.
pub const PLACEMENT_ONLY: &str = "only";

impl ShareInfo {
    /// The name of the device that has the machine's screen to itself, when one has: the local
    /// window has no picture until it leaves.
    pub fn viewing_only(&self) -> Option<&str> {
        (self.viewer_placement.as_deref() == Some(PLACEMENT_ONLY)).then_some(self.viewer.as_deref()?)
    }

    pub fn off(machine: &str) -> ShareInfo {
        ShareInfo { machine: machine.to_string(), ..ShareInfo::default() }
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_debug;
