//! What a host that keeps pairing's time itself says about pairing, as four events: pairing opened
//! until a time, a device waits, a device paired, pairing closed. Such a host feeds them to its
//! pairing with [`crate::pairing::Pairing::host_says`] or [`crate::session::Driver::host_says`],
//! in the order it said them, and the pairing follows: it reads no status for it and runs no clock
//! of its own (see [`crate::pairing`]).
//!
//! Times are whole seconds, as the host says them. The pairing turns them into its own milliseconds
//! with the `now` of the step:
//! - `until_unix_seconds` is when the host closes pairing, in whole seconds since 1970. The end the
//!   pairing shows is that times 1000. 0 is not fed: it would read as an end already passed.
//! - `seconds` is how long the person still has to answer the device, as the host said it. The end
//!   the pairing shows is `now` plus that.
//!
//! The JSON is an object with [`TAG`] naming the event and its fields beside it, for a host that
//! passes the events on. A device that waits inside a host's status stays a plain
//! [`crate::share::Pending`]; [`PairingEvent::from`] makes the event of one.
//!
//! A device's name and its code never go to a log. [`PairingEvent`]'s `Debug` leaves both out, so a
//! `{:?}` in a log line cannot print them. The JSON has them, as it is for the screen that shows them.

use crate::share::{CloseReason, Pending};
use serde::{Deserialize, Serialize};
use std::fmt;

/// The field that names which event a JSON object is.
pub const TAG: &str = "pairing_event";

/// One thing a host said about pairing.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "pairing_event", rename_all = "snake_case")]
pub enum PairingEvent {
    /// Pairing is open until this time, in whole seconds since 1970.
    PairingOpen { until_unix_seconds: u64 },
    /// A device waits for an answer: its name, its fingerprint, the code it shows ("123 456", six
    /// digits with a space in the middle, as remoteman says it), and the seconds left to answer.
    DeviceWaits { name: String, fingerprint: String, code: String, seconds: u32 },
    /// A device was let in, by its fingerprint, under the name it gave.
    Paired { fingerprint: String, name: String },
    /// Pairing closed, and why.
    PairingClosed { reason: CloseReason },
}

/// A device that waits, as a host's status has it, as the event. Seconds past `u32::MAX` are
/// `u32::MAX`.
impl From<Pending> for PairingEvent {
    fn from(pending: Pending) -> PairingEvent {
        PairingEvent::DeviceWaits {
            name: pending.name,
            fingerprint: pending.fingerprint,
            code: pending.code,
            seconds: u32::try_from(pending.seconds).unwrap_or(u32::MAX),
        }
    }
}

/// Never the name or the code: what is left out is shown as `..`.
impl fmt::Debug for PairingEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PairingEvent::PairingOpen { until_unix_seconds } => {
                f.debug_struct("PairingOpen").field("until_unix_seconds", until_unix_seconds).finish()
            }
            PairingEvent::DeviceWaits { fingerprint, seconds, .. } => f
                .debug_struct("DeviceWaits")
                .field("fingerprint", fingerprint)
                .field("seconds", seconds)
                .finish_non_exhaustive(),
            PairingEvent::Paired { fingerprint, .. } => {
                f.debug_struct("Paired").field("fingerprint", fingerprint).finish_non_exhaustive()
            }
            PairingEvent::PairingClosed { reason } => {
                f.debug_struct("PairingClosed").field("reason", reason).finish()
            }
        }
    }
}

#[cfg(test)]
mod tests;
