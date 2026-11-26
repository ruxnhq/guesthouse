//! Pairing a new device, as a state machine with no I/O and no clock in it.
//!
//! remoteman's host in a guest opens pairing with `pair on` and then has no limit: it waits for a
//! device, holds that device for an answer, and refuses it itself after a minute. So the time an
//! opening lasts, who may answer, and why it ended are kept here. The host's program feeds in
//! what happened (a reading of `status`, a button, the time) and sends the commands that come
//! back, in order. Nothing here sends, reads or waits.
//!
//! The rules it keeps:
//! - The clock counts only while nobody waits. When it runs out, a reading is asked for first, and
//!   `pair off` goes only if no device has come, since `pair off` declines the device that waits.
//! - `pair off` is never sent while a device waits. Stopping then declines it, which is the same
//!   and says so.
//! - An opening is used by one device, answered or not. Pairing is closed after it, and a new one
//!   is asked for.
//! - Why it closed is worked out from what changed: a new fingerprint among the devices is
//!   `paired`; a waiting device that went without one is `timeout` if its time was up and `declined`
//!   otherwise; the rest is `asked` or `other`.
//! - Pairing is closed at start-up if it was found open, and at shutdown.
//! - A device let in has its clipboard and files turned off at once. With `hold_host_wide`, the
//!   host-wide clipboard and files are turned off before pairing opens and put back once those two
//!   lines have gone out, so that the device has neither between being let in and being limited.
//!   The host takes the features it offers when a device connects, not when it is let in, so this
//!   has to start with the opening and not with the device's arrival.
//!
//! Nothing here can be rebuilt from a reading alone except what a reading shows: a device waiting,
//! pairing left open, the devices. What was held (see [`Pairing::held`]) and a device let in whose
//! limits were not yet sent are lost if the program ends there, so a program that wants them keeps
//! `held`.

use crate::ctl::{self, Words};
use crate::policy::Policy;
use crate::share::{CloseReason, Pending, pairing_seconds};
use crate::status::Status;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Milliseconds since 1970, from whoever owns the clock.
pub type Millis = u64;

/// How long after `pair on` a reading may still say pairing is not open, as the command and the
/// reading cross.
pub const OPEN_GRACE_MS: Millis = 5_000;
/// How many readings in a row may show pairing closed, with nobody waiting and nobody new, before
/// it is taken as closed. One is not enough: the host turns pairing off as a device connects, and
/// has the device's name only a moment later.
pub const CLOSED_READINGS: u8 = 2;
/// How long a device that was accepted has to appear among the devices.
pub const ACCEPT_GRACE_MS: Millis = 10_000;
/// How early a waiting device that went counts as having run out of time.
pub const TIMEOUT_SLACK_MS: Millis = 2_000;
/// How often to read the status while pairing is open or a device waits. It also keeps the host
/// from refusing a device early because nobody has looked.
pub const POLL_MS: Millis = 1_000;

/// What is said when asked to answer a device and none waits.
pub const NOBODY_WAITING: &str = "Nobody is waiting.";
/// What is said when asked to stop and pairing is not open.
pub const NOT_OPEN: &str = "Pairing is not open.";
/// What is said when asked to open while a device waits.
pub const DEVICE_WAITING: &str = "A device is waiting for an answer.";
/// What is said when asked to open before the host has been read.
pub const NOT_READ_YET: &str = "The host has not been read yet. Try again in a moment.";

/// The host-wide switches that were on, and are to be put back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostWide {
    pub clipboard: bool,
    pub files: bool,
}

/// What happened.
#[derive(Debug, Clone, Copy)]
pub enum Event<'a> {
    /// A person asked to open pairing for this many seconds, or for longer or shorter while it is
    /// open.
    Open { seconds: u64 },
    /// A reading of the host's status arrived. Its time is the `now` of the step.
    Reading(&'a Status),
    /// A person accepted the device that waits.
    Accept,
    /// A person declined it.
    Decline,
    /// A person stopped pairing.
    Stop,
    /// Time went by. Nothing else is needed for the clock.
    Tick,
    /// The switch for viewing from other devices went off.
    SwitchOff,
    /// The program is quitting.
    Shutdown,
    /// The program started, or found the machine running: whatever is open was not opened by this
    /// run. `held` is what an earlier run wrote down of [`Pairing::held`].
    Started { held: Option<HostWide> },
}

/// What a step gives back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub pairing: Pairing,
    /// The `ctl` commands to send, in this order, each only once the one before has been answered.
    pub commands: Vec<Words>,
    /// A reading of the status is wanted now, not at the next poll.
    pub read_now: bool,
    /// Why what was asked is not done, in words for the person.
    pub refused: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Opening {
    left_ms: Millis,
    counted_to: Millis,
    opened_at: Millis,
    /// A reading has shown pairing open since this was asked for.
    seen_open: bool,
    /// Readings in a row with pairing closed and nothing to show for it.
    quiet: u8,
    /// The time is up and a reading is waited for before closing.
    expired: bool,
}

/// A device that waits, or was accepted, as last seen.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Device {
    pending: Pending,
    seen_at: Millis,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Phase {
    /// No reading yet.
    Unknown,
    Closed,
    Open(Opening),
    Waiting(Device),
    /// Accepted, and waited for among the devices.
    Accepted(Device, Millis),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Outcome {
    reason: CloseReason,
    paired: Option<String>,
}

/// Where pairing stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pairing {
    policy: Policy,
    phase: Phase,
    /// The fingerprints among the devices at the last reading, without the hidden one.
    known: Option<BTreeSet<String>>,
    /// What the last reading said of the host-wide clipboard and files.
    host_wide: (Option<bool>, Option<bool>),
    held: Option<HostWide>,
    outcome: Option<Outcome>,
}

/// What a screen needs of pairing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    /// Pairing is open and waiting for a device.
    pub open: bool,
    /// When it closes, by this clock.
    pub until_ms: Option<Millis>,
    /// The device that waits, until it is answered and has appeared among the devices.
    pub pending: Option<Pending>,
    /// When that device's own time runs out, by this clock.
    pub pending_until_ms: Option<Millis>,
    pub closed_reason: Option<CloseReason>,
    /// The name of the device that paired since pairing was last opened.
    pub paired: Option<String>,
}

impl Pairing {
    pub fn new(policy: Policy) -> Pairing {
        Pairing {
            policy,
            phase: Phase::Unknown,
            known: None,
            host_wide: (None, None),
            held: None,
            outcome: None,
        }
    }

    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    /// The host-wide switches this has turned off and owes back. A program that can be stopped
    /// without a goodbye writes this down and gives it to [`Event::Started`].
    pub fn held(&self) -> Option<HostWide> {
        self.held
    }

    /// How often to read the status now, or `None` when nothing needs it sooner than the program's
    /// own pace.
    pub fn poll_ms(&self) -> Option<Millis> {
        match self.phase {
            Phase::Open(_) | Phase::Waiting(_) | Phase::Accepted(..) => Some(POLL_MS),
            Phase::Unknown | Phase::Closed => None,
        }
    }

    /// Pairing is open or a device is being answered: what a Pair button waits for.
    pub fn busy(&self) -> bool {
        matches!(self.phase, Phase::Open(_) | Phase::Waiting(_) | Phase::Accepted(..))
    }

    pub fn view(&self) -> View {
        let (open, until_ms) = match &self.phase {
            Phase::Open(opening) if !opening.expired => (true, Some(opening.counted_to + opening.left_ms)),
            Phase::Open(opening) => (true, Some(opening.counted_to)),
            _ => (false, None),
        };
        let device = match &self.phase {
            Phase::Waiting(device) | Phase::Accepted(device, _) => Some(device),
            _ => None,
        };
        View {
            open,
            until_ms,
            pending: device.map(|device| device.pending.clone()),
            pending_until_ms: device.map(|device| device.seen_at + device.pending.seconds * 1000),
            closed_reason: self.outcome.as_ref().map(|outcome| outcome.reason),
            paired: self.outcome.as_ref().and_then(|outcome| outcome.paired.clone()),
        }
    }

    /// What follows from `event` at time `now`.
    pub fn step(&self, now: Millis, event: Event) -> Step {
        let mut work = Work { next: self.clone(), commands: Vec::new(), read_now: false, refused: None };
        match event {
            Event::Open { seconds } => work.next.open(now, seconds, &mut work.commands, &mut work.refused),
            Event::Reading(status) => work.next.reading(now, status, &mut work.commands),
            Event::Accept => work.next.accept(now, &mut work.commands, &mut work.refused),
            Event::Decline => work.next.decline(&mut work.commands, &mut work.refused, CloseReason::Declined),
            Event::Stop => work.next.stop(&mut work.commands, &mut work.refused),
            Event::Tick => work.next.tick(now, &mut work.read_now),
            Event::SwitchOff | Event::Shutdown => work.next.end(&mut work.commands),
            Event::Started { held } => {
                work.next = Pairing { held, ..Pairing::new(self.policy.clone()) };
            }
        }
        Step { pairing: work.next, commands: work.commands, read_now: work.read_now, refused: work.refused }
    }
}

struct Work {
    next: Pairing,
    commands: Vec<Words>,
    read_now: bool,
    refused: Option<String>,
}

type Commands = Vec<Words>;

impl Pairing {
    fn open(&mut self, now: Millis, seconds: u64, commands: &mut Commands, refused: &mut Option<String>) {
        let seconds = match pairing_seconds(Some(seconds)) {
            Ok(seconds) => seconds,
            Err(why) => return *refused = Some(why),
        };
        let limit = seconds * 1000;
        match &mut self.phase {
            Phase::Unknown => *refused = Some(NOT_READ_YET.to_string()),
            Phase::Waiting(_) | Phase::Accepted(..) => *refused = Some(DEVICE_WAITING.to_string()),
            // Again while it is open sets a new end, with nothing sent: it is open already.
            Phase::Open(opening) => {
                *opening =
                    Opening { left_ms: limit, counted_to: now, expired: false, quiet: 0, ..opening.clone() }
            }
            Phase::Closed => {
                self.outcome = None;
                self.hold(commands);
                commands.push(ctl::pair_open());
                self.phase = Phase::Open(Opening {
                    left_ms: limit,
                    counted_to: now,
                    opened_at: now,
                    seen_open: false,
                    quiet: 0,
                    expired: false,
                });
            }
        }
    }

    fn accept(&mut self, now: Millis, commands: &mut Commands, refused: &mut Option<String>) {
        match &self.phase {
            Phase::Waiting(device) => {
                commands.push(ctl::pair_accept());
                self.phase = Phase::Accepted(device.clone(), now);
            }
            // Already accepted: pressed twice.
            Phase::Accepted(..) => {}
            _ => *refused = Some(NOBODY_WAITING.to_string()),
        }
    }

    /// Turns a waiting device away. `reason` is why, for the one who asked.
    fn decline(&mut self, commands: &mut Commands, refused: &mut Option<String>, reason: CloseReason) {
        match self.phase {
            Phase::Waiting(_) => {
                commands.push(ctl::pair_decline());
                self.close(reason, None, commands);
            }
            Phase::Accepted(..) => {}
            _ => *refused = Some(NOBODY_WAITING.to_string()),
        }
    }

    fn stop(&mut self, commands: &mut Commands, refused: &mut Option<String>) {
        match self.phase {
            Phase::Open(_) => {
                commands.push(ctl::pair_close());
                self.close(CloseReason::Asked, None, commands);
            }
            // `pair off` would decline it too, and this says so.
            Phase::Waiting(_) => self.decline(commands, refused, CloseReason::Asked),
            Phase::Accepted(..) => {}
            Phase::Unknown | Phase::Closed => *refused = Some(NOT_OPEN.to_string()),
        }
    }

    /// The switch went off, or the program is quitting: nothing is left open or waiting.
    fn end(&mut self, commands: &mut Commands) {
        match self.phase {
            Phase::Open(_) => commands.push(ctl::pair_close()),
            Phase::Waiting(_) => commands.push(ctl::pair_decline()),
            _ => {}
        }
        let was_busy = self.busy();
        if was_busy {
            self.close(CloseReason::Asked, None, commands);
        } else {
            self.restore(commands);
        }
    }

    fn tick(&mut self, now: Millis, read_now: &mut bool) {
        let Phase::Open(opening) = &mut self.phase else { return };
        if opening.expired {
            return;
        }
        opening.left_ms = opening.left_ms.saturating_sub(now.saturating_sub(opening.counted_to));
        opening.counted_to = now.max(opening.counted_to);
        if opening.left_ms == 0 {
            opening.expired = true;
            opening.quiet = 0;
            *read_now = true;
        }
    }

    /// Ends this pairing for `reason`, giving back what was held.
    fn close(&mut self, reason: CloseReason, paired: Option<String>, commands: &mut Commands) {
        self.phase = Phase::Closed;
        self.outcome = Some(Outcome { reason, paired });
        self.restore(commands);
    }

    /// Turns off the host-wide clipboard and files that are on, and remembers them.
    fn hold(&mut self, commands: &mut Commands) {
        if !self.policy.hold_host_wide {
            return;
        }
        let (clipboard, files) = (self.host_wide.0 == Some(true), self.host_wide.1 == Some(true));
        let before = self.held.unwrap_or(HostWide { clipboard: false, files: false });
        if clipboard {
            commands.extend(ctl::host_allow("clipboard", false));
        }
        if files {
            commands.extend(ctl::host_allow("files", false));
        }
        if clipboard || files {
            self.held =
                Some(HostWide { clipboard: before.clipboard || clipboard, files: before.files || files });
            self.host_wide =
                (self.host_wide.0.map(|on| on && !clipboard), self.host_wide.1.map(|on| on && !files));
        }
    }

    /// Puts back what [`Pairing::hold`] turned off.
    fn restore(&mut self, commands: &mut Commands) {
        let Some(held) = self.held.take() else { return };
        if held.clipboard {
            commands.extend(ctl::host_allow("clipboard", true));
            self.host_wide.0 = Some(true);
        }
        if held.files {
            commands.extend(ctl::host_allow("files", true));
            self.host_wide.1 = Some(true);
        }
    }

    /// A device was let in: its clipboard and files go off, then what was held comes back.
    fn paired(&mut self, fingerprint: &str, name: &str, commands: &mut Commands) {
        for what in ["clipboard", "files"] {
            commands.extend(self.policy.device_allow(fingerprint, what, false));
        }
        self.close(CloseReason::Paired, Some(name.to_string()), commands);
    }

    /// A waiting device went without appearing: its time was up, or it was turned away.
    fn gone(&mut self, now: Millis, device: &Device, commands: &mut Commands) {
        let waited = now.saturating_sub(device.seen_at) + TIMEOUT_SLACK_MS;
        let reason = if device.pending.seconds * 1000 <= waited {
            CloseReason::Timeout
        } else {
            CloseReason::Declined
        };
        self.close(reason, None, commands);
    }

    fn reading(&mut self, now: Millis, status: &Status, commands: &mut Commands) {
        let devices: Vec<(String, &str)> = status
            .devices
            .iter()
            .map(|device| (device.fingerprint.to_ascii_lowercase(), device.name.as_str()))
            .filter(|(fingerprint, _)| !self.policy.hides(fingerprint))
            .collect();
        let appeared: Vec<&(String, &str)> = match &self.known {
            Some(known) => devices.iter().filter(|(fingerprint, _)| !known.contains(fingerprint)).collect(),
            None => Vec::new(),
        };
        self.known = Some(devices.iter().map(|(fingerprint, _)| fingerprint.clone()).collect());
        // What the host-wide switches are now, unless they are held, when the reading says only that
        // they are off.
        if self.held.is_none() {
            self.host_wide = (status.clipboard, status.files);
        }
        let waiting = status.pending.as_ref();

        match self.phase.clone() {
            Phase::Unknown => match waiting {
                Some(pending) => self.adopt(now, pending, commands),
                None => {
                    if status.pairing {
                        // Left open by whoever ran before: nobody here asked for it.
                        commands.push(ctl::pair_close());
                    }
                    self.phase = Phase::Closed;
                    self.restore(commands);
                }
            },
            Phase::Closed => match waiting {
                Some(pending) => self.adopt(now, pending, commands),
                None if status.pairing => commands.push(ctl::pair_close()),
                None => {}
            },
            Phase::Open(opening) => self.reading_open(now, opening, status, &appeared, commands),
            Phase::Waiting(device) => {
                let came =
                    appeared.iter().find(|(fingerprint, _)| *fingerprint == device.pending.fingerprint);
                if let Some((fingerprint, name)) = came.or(appeared.first()) {
                    self.paired(fingerprint, name, commands);
                } else if let Some(pending) = waiting {
                    self.phase = Phase::Waiting(Device { pending: pending.clone(), seen_at: now });
                } else {
                    self.gone(now, &device, commands);
                }
            }
            Phase::Accepted(device, since) => {
                let came = devices.iter().find(|(fingerprint, _)| *fingerprint == device.pending.fingerprint);
                if let Some((fingerprint, name)) = came.or(appeared.first().copied()) {
                    self.paired(fingerprint, name, commands);
                } else if waiting.is_some_and(|pending| pending.fingerprint != device.pending.fingerprint) {
                    self.adopt(now, waiting.expect("checked just above"), commands);
                } else if waiting.is_none() && now.saturating_sub(since) >= ACCEPT_GRACE_MS {
                    self.gone(now, &device, commands);
                } else if let Some(pending) = waiting {
                    self.phase = Phase::Accepted(Device { pending: pending.clone(), seen_at: now }, since);
                }
            }
        }
    }

    fn reading_open(
        &mut self,
        now: Millis,
        mut opening: Opening,
        status: &Status,
        appeared: &[&(String, &str)],
        commands: &mut Commands,
    ) {
        if let Some(pending) = &status.pending {
            return self.adopt(now, pending, commands);
        }
        if let Some((fingerprint, name)) = appeared.first() {
            return self.paired(fingerprint, name, commands);
        }
        if status.pairing {
            opening.seen_open = true;
            opening.quiet = 0;
            if opening.expired {
                commands.push(ctl::pair_close());
                return self.close(CloseReason::Timeout, None, commands);
            }
        } else if opening.seen_open || now >= opening.opened_at + OPEN_GRACE_MS {
            opening.quiet += 1;
            // Sharing off closes pairing for good, and needs no second look.
            if opening.quiet >= CLOSED_READINGS || status.paused {
                let reason = if opening.expired { CloseReason::Timeout } else { CloseReason::Other };
                return self.close(reason, None, commands);
            }
        }
        self.phase = Phase::Open(opening);
    }

    /// A device is waiting that this did not see come: it is answered from here.
    fn adopt(&mut self, now: Millis, pending: &Pending, commands: &mut Commands) {
        if self.held.is_none() {
            self.hold(commands);
        }
        self.outcome = None;
        self.phase = Phase::Waiting(Device { pending: pending.clone(), seen_at: now });
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_policy;
#[cfg(test)]
mod tests_rebuild;
