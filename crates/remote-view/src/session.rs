//! Pairing driven against a running guest: each machine's [`Session`] (its pairing, its last
//! reading of the host's status and why that failed), and the [`Driver`] that feeds pairing its
//! events, sends the `ctl` commands that come back, and reads the host again when it is asked to.
//!
//! What only a host can do comes in through [`Guest`]: running `ctl` words in the guest, writing
//! its log, and, on a host that can be stopped without a goodbye while pairing holds something
//! off, keeping that where its next run finds it. The clock comes in as a function, and threads,
//! polling and the switch are the host's own.
//!
//! A session is locked only for a moment, never while the guest is asked something, so a screen
//! can always be shown what was last read.
//!
//! A step ends. Pairing sends a command again only when a reading still shows what the command was
//! to change, so a guest whose status never shows a command taken, or a stale reading, would have
//! it sent for ever. A step sends the same commands at most [`SAME_COMMANDS_PER_STEP`] times, once
//! and once more for a reading that crossed them, and sends at most [`ROUNDS_PER_STEP`] rounds of
//! commands, far more than a pairing ever needs between two quiet readings. Past either, or when
//! the guest does not answer a command, the step stops with what it has not sent unsent, and the
//! pairing goes on from before the round that could not go out, so the next reading works out
//! again what is still to do. Nothing held off is forgotten. A command the guest did not answer
//! may or may not have run, so it is never sent again blind: the next step reads the host first.

use crate::ctl::{self, Ack, Words, parse_ack};
use crate::events::{PairingEvent, pairing_events};
use crate::pairing::{Event, HostWide, Millis, Pairing};
use crate::policy::Policy;
use crate::share::{ShareInfo, ShareState};
use crate::status::{Reply, Status, parse_reply};
use crate::view::{Facts, share_info};
use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

/// How old a reading may be before a screen has another taken behind it.
pub const STALE_MS: Millis = 2_000;
/// What is said when a device command finds the guest's host shutting down.
pub const HOST_STOPPING: &str = "remoteman's host is stopping.";
/// How many times one step sends the same commands: once, and once more after a reading that
/// crossed them.
pub const SAME_COMMANDS_PER_STEP: usize = 2;
/// How many rounds of commands one step sends. A pairing needs at most three: one for the event,
/// one for the device or the switches the reading after it shows, and one more if a reading
/// crossed them. No run of readings today's pairing can be fed gets here without sending the same
/// commands a third time first, so this is a guard for a pairing that changes.
pub const ROUNDS_PER_STEP: usize = 16;

/// The guest gave no answer to a command, or not all of one, in the host's words: the command may
/// or may not have run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unanswered(pub String);

/// What is said when a step would act while the guest has not answered since a command that may
/// or may not have run.
pub const STILL_UNSURE: &str =
    "The guest has not answered since a command it may or may not have run. Try again once it does.";

/// Why a step stopped: the guest's status still asks for what was sent twice.
fn not_taken(commands: &[Words]) -> String {
    let sent = commands.iter().map(|words| words.join(" ")).collect::<Vec<_>>().join(", ");
    format!("The guest did not take {sent}. It is asked again at the next reading.")
}

fn too_many_rounds() -> String {
    format!(
        "The guest's status kept asking for more after {ROUNDS_PER_STEP} rounds of commands. It is read \
         again at the next reading."
    )
}

fn unanswered(words: &Words, why: &Unanswered) -> String {
    format!(
        "The guest did not answer {}: {}. It is read again before anything more is sent.",
        words.join(" "),
        why.0
    )
}

/// What the driver asks of the host it runs on.
pub trait Guest {
    /// Runs one `ctl` command in the guest's remoteman host, and gives what it printed. A host
    /// whose way into the guest can tell a lost or broken answer from one the guest wrote gives
    /// [`Unanswered`] for it; one that can't gives what it has, as text.
    fn ctl(&self, words: &Words) -> Result<String, Unanswered>;

    /// Writes one line in the host's log.
    fn log(&self, line: &str);

    /// What an earlier run wrote down of [`Pairing::held`]. A host with nowhere to keep it gives
    /// nothing. It is asked with the sessions locked, so it must not touch them.
    fn held(&self) -> Option<HostWide> {
        None
    }

    /// Writes down [`Pairing::held`] after every step, once its commands have gone out, so that a
    /// run that ends with something held off gives it back at the next. A host with nowhere to keep
    /// it does nothing.
    fn keep_held(&self, _held: Option<HostWide>) {}
}

/// What one machine's pairing has, between calls.
#[derive(Debug, Default)]
pub struct Session {
    pub pairing: Option<Pairing>,
    /// The last status the host gave.
    pub reading: Option<Status>,
    /// When the host was last asked for its status.
    pub read_at: Option<Millis>,
    /// Why the last reading failed, in the host's words.
    pub reason: Option<String>,
    /// A reading or a clock is running behind the screen. The host's own: the driver never sets it.
    pub busy: bool,
    /// The host's fingerprint, kept from the last reading that had it.
    pub fingerprint: Option<String>,
    /// The guest did not answer a command, which may or may not have run: the next step reads the
    /// host before anything else. A reading clears it.
    pub unsure: bool,
}

impl Session {
    /// What a screen shows, from what was last read. `state` and `port` are the host's, and
    /// `reason` is a failure of its own, which goes before a failed reading's.
    pub fn shown(
        &self,
        machine: &str,
        policy: &Policy,
        state: ShareState,
        port: Option<u16>,
        reason: Option<String>,
    ) -> ShareInfo {
        let reason = reason.or(self.reason.clone());
        let pairing = self.pairing.clone().unwrap_or_else(|| Pairing::new(policy.clone()));
        let facts = Facts {
            machine,
            state,
            port,
            fingerprint: self.fingerprint.as_deref(),
            reason: reason.as_deref(),
        };
        share_info(&facts, self.reading.as_ref(), &pairing)
    }

    /// Whether the last reading is old enough at `now` that another should be taken. A clock that
    /// went back counts as old.
    pub fn stale(&self, now: Millis) -> bool {
        self.read_at.is_none_or(|at| now.checked_sub(at).is_none_or(|age| age >= STALE_MS))
    }
}

/// Every machine's session, by the machine's name.
#[derive(Debug, Default)]
pub struct Sessions {
    all: Mutex<HashMap<String, Session>>,
}

impl Sessions {
    /// Runs `change` on the machine's session, made empty if it has none, with every session
    /// locked. Never ask the guest from inside it.
    pub fn with<R>(&self, machine: &str, change: impl FnOnce(&mut Session) -> R) -> R {
        let mut all = self.all.lock().unwrap_or_else(PoisonError::into_inner);
        change(all.entry(machine.to_string()).or_default())
    }

    /// Forgets the machine's session.
    pub fn remove(&self, machine: &str) {
        self.all.lock().unwrap_or_else(PoisonError::into_inner).remove(machine);
    }

    /// The machines that have a session.
    pub fn machines(&self) -> Vec<String> {
        self.all.lock().unwrap_or_else(PoisonError::into_inner).keys().cloned().collect()
    }
}

/// What a step is fed.
#[derive(Debug, Clone, Copy)]
pub enum StepEvent {
    Plain(Event<'static>),
    /// Read the host's status, and feed the reading in.
    Read,
}

/// A pairing for this run of the host: whatever an earlier run held is owed back.
pub fn fresh_pairing(policy: Policy, held: Option<HostWide>) -> Pairing {
    Pairing::new(policy).step(0, Event::Started { held }).pairing
}

/// One machine's pairing, as one call drives it.
pub struct Driver<'a> {
    pub sessions: &'a Sessions,
    pub machine: &'a str,
    pub policy: &'a Policy,
    pub guest: &'a dyn Guest,
    /// Milliseconds since 1970, read before each step and each reading.
    pub now: &'a dyn Fn() -> Millis,
}

impl Driver<'_> {
    /// A pairing for this run, from what the guest's host kept of an earlier one.
    pub fn fresh_pairing(&self) -> Pairing {
        fresh_pairing(self.policy.clone(), self.guest.held())
    }

    /// Feeds one event to the machine's pairing, sends the commands it gives in order, keeps
    /// `held`, and reads the host again when asked to. Gives why the event was refused, or why the
    /// step stopped short (see the module's top).
    pub fn step(&self, event: StepEvent) -> Option<String> {
        self.step_with_events(event).0
    }

    /// [`Driver::step`], and the [`PairingEvent`]s of every change it kept, in order (see
    /// [`crate::events`]). A round whose commands did not all go out is not kept, so it says nothing.
    pub fn step_with_events(&self, event: StepEvent) -> (Option<String>, Vec<PairingEvent>) {
        let mut said = Vec::new();
        let (mut pairing, unsure) = self.sessions.with(self.machine, |session| {
            (session.pairing.get_or_insert_with(|| self.fresh_pairing()).clone(), session.unsure)
        });
        // The last pop goes first.
        let read_first = unsure && matches!(event, StepEvent::Plain(_));
        let mut events = if read_first { vec![event, StepEvent::Read] } else { vec![event] };
        let mut refused = None;
        let mut sent: Vec<Vec<Words>> = Vec::new();
        while let Some(event) = events.pop() {
            let reading;
            let now;
            let step = match &event {
                StepEvent::Plain(_) if read_first && self.sessions.with(self.machine, |s| s.unsure) => {
                    refused = Some(STILL_UNSURE.to_string());
                    break;
                }
                StepEvent::Plain(event) => {
                    now = (self.now)();
                    pairing.step(now, *event)
                }
                StepEvent::Read => {
                    reading = self.read();
                    let Some(status) = &reading else { continue };
                    now = (self.now)();
                    pairing.step(now, Event::Reading(status))
                }
            };
            refused = refused.or(step.refused);
            if !step.commands.is_empty() {
                let times = sent.iter().filter(|commands| **commands == step.commands).count();
                let stop = if times >= SAME_COMMANDS_PER_STEP {
                    Some(not_taken(&step.commands))
                } else if sent.len() >= ROUNDS_PER_STEP {
                    Some(too_many_rounds())
                } else {
                    None
                };
                if let Some(why) = stop {
                    self.guest.log(&why);
                    refused = refused.or(Some(why));
                    break;
                }
                sent.push(step.commands.clone());
            }
            if let Some(why) = self.send(&step.commands) {
                // Some of the round may have gone out: what it held may be off now.
                pairing.owe(step.pairing.held());
                self.guest.keep_held(pairing.held());
                self.guest.log(&why);
                refused = refused.or(Some(why));
                self.sessions.with(self.machine, |session| session.unsure = true);
                break;
            }
            said.extend(pairing_events(&pairing, &step.pairing, now));
            pairing = step.pairing;
            self.guest.keep_held(pairing.held());
            if step.read_now || !step.commands.is_empty() {
                events.push(StepEvent::Read);
            }
        }
        self.sessions.with(self.machine, |session| session.pairing = Some(pairing));
        (refused, said)
    }

    /// Sends a round's commands in order, each once the one before has been answered. Gives why
    /// it stopped, when the guest did not answer one.
    fn send(&self, commands: &[Words]) -> Option<String> {
        for words in commands {
            match self.guest.ctl(words) {
                Ok(said) => {
                    if let Ack::Refused(why) = parse_ack(&said) {
                        self.guest.log(&format!("ctl {}: {why}", words.join(" ")));
                    }
                }
                Err(why) => return Some(unanswered(words, &why)),
            }
        }
        None
    }

    /// Reads the host's status, and keeps it.
    pub fn read(&self) -> Option<Status> {
        let said = self.guest.ctl(&ctl::status());
        let now = (self.now)();
        self.sessions.with(self.machine, |session| {
            session.read_at = Some(now);
            let said = match said {
                Ok(said) => said,
                Err(why) => {
                    session.reason = Some(why.0);
                    return None;
                }
            };
            match parse_reply(&said) {
                Reply::Status(status) => {
                    session.reason = None;
                    session.unsure = false;
                    session.fingerprint = status.fingerprint.clone().or(session.fingerprint.take());
                    session.reading = Some(*status.clone());
                    Some(*status)
                }
                Reply::Stopping => None,
                Reply::Error(why) => {
                    session.reason = Some(why);
                    None
                }
            }
        })
    }

    /// Opens pairing for `seconds`, once the host has been read, as it has to have been. Gives why
    /// it did not open.
    pub fn open_pairing(&self, seconds: u64) -> Option<String> {
        self.step(StepEvent::Read);
        self.step(StepEvent::Plain(Event::Open { seconds }))
    }

    /// [`Driver::open_pairing`], and the [`PairingEvent`]s of both its steps, in order.
    pub fn open_pairing_with_events(&self, seconds: u64) -> (Option<String>, Vec<PairingEvent>) {
        let (_, mut said) = self.step_with_events(StepEvent::Read);
        let (refused, opened) = self.step_with_events(StepEvent::Plain(Event::Open { seconds }));
        said.extend(opened);
        (refused, said)
    }

    /// Sends one command about a device, then reads the host again. Gives why the host would not,
    /// or that the guest did not answer.
    pub fn device(&self, words: &Words) -> Result<(), String> {
        let said = match self.guest.ctl(words) {
            Ok(said) => said,
            Err(why) => {
                self.sessions.with(self.machine, |session| session.unsure = true);
                return Err(unanswered(words, &why));
            }
        };
        match parse_ack(&said) {
            Ack::Done(_) | Ack::NotKept(_) => {}
            Ack::Refused(why) => return Err(why),
            Ack::Stopping => return Err(HOST_STOPPING.to_string()),
        }
        self.read();
        Ok(())
    }

    /// The host found the machine running at its start: whatever pairing an earlier run left open
    /// is closed and what it held is given back.
    pub fn started(&self) {
        let fresh = self.fresh_pairing();
        self.sessions.with(self.machine, |session| session.pairing = Some(fresh));
        self.step(StepEvent::Read);
    }

    /// How soon to read again, while pairing is open or a device waits.
    pub fn poll_ms(&self) -> Option<Millis> {
        let all = self.sessions.all.lock().unwrap_or_else(PoisonError::into_inner);
        all.get(self.machine).and_then(|session| session.pairing.as_ref()).and_then(Pairing::poll_ms)
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_events;
