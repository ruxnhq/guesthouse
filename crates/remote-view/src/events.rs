//! What changed in a pairing, as events a host can pass on as they are: pairing opened, a device
//! waits, a device paired, pairing closed. They are worked out from the pairing before and after a
//! step, so every host that runs [`Pairing`] gets the same events from the same changes and none of
//! them works them out again.
//!
//! Times are whole seconds, worked out from the step's `now` ([`Millis`], milliseconds since 1970,
//! from the host's clock as [`Pairing::step`] takes it). Nothing here reads a clock:
//! - `until_unix_seconds` is the pairing's end in milliseconds since 1970, divided by 1000 and rounded
//!   up, so it never says pairing closes before it does. The end is the one [`Pairing::view`] gives
//!   while the time runs. It is never 0 here, as every pairing this crate opens has an end.
//! - `seconds` is the time the device still has to be answered at `now`, rounded up to whole
//!   seconds as [`crate::wording::seconds_until`] rounds, 0 once it is up, and at most `u32::MAX`.
//!   When the device has just come it is what the host said in `status.pending.seconds`.
//!
//! The rules:
//! - Pairing opened is said when an opening starts, and again when it is given a new end while open.
//!   It is not said when its time runs out: pairing closed follows once a reading says so.
//! - A device that waits is said once, when it comes, and again only for another device. The readings
//!   in between do not repeat it.
//! - When a device paired, paired comes first and then pairing closed, in the same step.
//! - Pairing closed is said whenever an opening or a waiting device ends, with the reason the pairing
//!   worked out. A pairing that ends with no reason, as when the host starts again and finds it open,
//!   says `other`.
//! - Only steps say anything. [`crate::session::Driver::started`] puts a fresh pairing in place
//!   without one, so a pairing it finds open closes with no event; the reading after it says none.
//!   A driver gives events through [`crate::session::Driver::step_with_events`] and
//!   [`crate::session::Driver::open_pairing_with_events`]; its other calls (`step`, `open_pairing`,
//!   `device`, `started`, `read`) say none, so a host that wants every event steps through those two.
//!
//! A device's name and its code never go to a log. [`PairingEvent`]'s `Debug` leaves both out, so a
//! `{:?}` in a log line cannot print them. The JSON has them, as it is for the screen that shows them.

use crate::pairing::{Millis, Pairing};
use crate::share::CloseReason;
use serde::{Deserialize, Serialize};
use std::fmt;

/// The field that names which event a JSON object is.
pub const TAG: &str = "pairing_event";

/// One change in a pairing. Its JSON is an object with [`TAG`] naming the event and the event's own
/// fields beside it.
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

/// Whole seconds since 1970 for a time in milliseconds since 1970, rounded up.
pub fn unix_seconds(ms: Millis) -> u64 {
    ms.div_ceil(1000)
}

/// Whole seconds from `now` to `until`, rounded up, 0 when it is not ahead, and at most `u32::MAX`.
pub fn seconds_left(until: Millis, now: Millis) -> u32 {
    u32::try_from(until.saturating_sub(now).div_ceil(1000)).unwrap_or(u32::MAX)
}

/// The events a step made, in order, from the pairing before it, the pairing after it, and the
/// `now` the step was given.
pub fn pairing_events(before: &Pairing, after: &Pairing, now: Millis) -> Vec<PairingEvent> {
    let mut events = Vec::new();
    let (was, is) = (before.view(), after.view());

    if let Some(end) = after.open_end()
        && before.open_end() != Some(end)
    {
        events.push(PairingEvent::PairingOpen { until_unix_seconds: unix_seconds(end) });
    }

    if let (Some(pending), Some(until)) = (&is.pending, is.pending_until_ms)
        && was.pending.as_ref().map(|before| &before.fingerprint) != Some(&pending.fingerprint)
    {
        events.push(PairingEvent::DeviceWaits {
            name: pending.name.clone(),
            fingerprint: pending.fingerprint.clone(),
            code: pending.code.clone(),
            seconds: seconds_left(until, now),
        });
    }

    if before.busy() && !after.busy() {
        let reason = is.closed_reason.unwrap_or(CloseReason::Other);
        if reason == CloseReason::Paired
            && let (Some(fingerprint), Some(name)) = (after.paired_fingerprint(), &is.paired)
        {
            events.push(PairingEvent::Paired { fingerprint: fingerprint.to_string(), name: name.clone() });
        }
        events.push(PairingEvent::PairingClosed { reason });
    }
    events
}

#[cfg(test)]
mod tests;
