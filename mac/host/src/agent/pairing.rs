// SPDX-License-Identifier: GPL-2.0-or-later
//! Pairing, from the shared remote-view crate's host-timed pairing, over rmhost.
//!
//! rmhost keeps pairing's time itself, closes it itself and says what happened on its output
//! (pairing opened until a time, a device waits, a device paired, pairing closed and why). So the
//! pairing is `Pairing::host_timed`: those lines are fed in as `PairingEvent`s with
//! `Driver::host_says`, a person's Open, Accept, Decline and Stop go in with `Driver::host_event`,
//! and the commands that follow go to rmhost's standard input through [`RmhostGuest`]. Nothing
//! reads a status and no clock of the crate's runs for it; what a screen shows comes back from
//! `Driver::shown`.

use super::Agent;
use crate::remote;
use guesthouse_remote_view_core::ctl::Words;
use guesthouse_remote_view_core::events::PairingEvent;
use guesthouse_remote_view_core::pairing::{Event, Millis};
use guesthouse_remote_view_core::policy::Policy;
use guesthouse_remote_view_core::session::{Driver, Guest, NotDelivered, Unanswered};
use guesthouse_remote_view_core::share::{CloseReason, ShareInfo};
use std::time::{SystemTime, UNIX_EPOCH};

/// The agent's way to one machine's rmhost: words go to its standard input as lines.
struct RmhostGuest<'a> {
    agent: &'a Agent,
    machine: &'a str,
}

impl Guest for RmhostGuest<'_> {
    /// rmhost answers no word, so there is nothing to ask: its events say what happened.
    fn ctl(&self, _words: &Words) -> Result<String, Unanswered> {
        Err(Unanswered("rmhost answers no word, it says what happened".into()))
    }

    fn send(&self, words: &Words) -> Result<(), NotDelivered> {
        let line = remote::rmhost_line(words).map_err(NotDelivered)?;
        self.agent.send_to_rmhost(self.machine, &line).map_err(NotDelivered)
    }

    fn log(&self, line: &str) {
        eprintln!("guesthouse-agent: {}: pairing: {line}", self.machine);
    }
}

fn now_ms() -> Millis {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_millis() as Millis)
}

impl Agent {
    /// Runs `ask` with the shared driver for this machine's pairing.
    fn with_pairing_driver<R>(&self, id: &str, ask: impl FnOnce(&Driver) -> R) -> R {
        let policy = Policy::default();
        let guest = RmhostGuest { agent: self, machine: id };
        let now = now_ms;
        ask(&Driver {
            sessions: &self.shared.pairing,
            machine: id,
            policy: &policy,
            guest: &guest,
            now: &now,
        })
    }

    /// A person's event: open pairing, answer a device that waits, or stop. Gives why it is
    /// refused.
    pub(super) fn pairing_event(&self, id: &str, event: Event<'static>) -> Result<(), String> {
        match self.with_pairing_driver(id, |driver| driver.host_event(event)) {
            Some(why) => Err(why),
            None => Ok(()),
        }
    }

    /// A command about a device, built by the shared policy and given to rmhost with no answer
    /// waited for: what happened comes back as what rmhost says. Gives why it did not go.
    pub(super) fn pairing_device(
        &self,
        id: &str,
        words: impl FnOnce(&Policy) -> Result<Words, String>,
    ) -> Result<(), String> {
        let policy = Policy::default();
        let words = words(&policy)?;
        self.with_pairing_driver(id, |driver| driver.host_device(&words))
    }

    /// What rmhost said about pairing, in the order it said it.
    pub(super) fn pairing_says(&self, id: &str, said: &PairingEvent) {
        if let Some(why) = self.with_pairing_driver(id, |driver| driver.host_says(said)) {
            eprintln!("guesthouse-agent: {id}: pairing: {why}");
        }
    }

    /// A new rmhost for the machine: whatever pairing the last one had open went with it.
    pub(super) fn pairing_restart(&self, id: &str) {
        self.with_pairing_driver(id, |driver| driver.host_started());
    }

    /// `info` with what a screen shows of pairing put in from the shared pairing: whether it is
    /// open and until when, the device that waits, who paired and why it closed.
    pub(super) fn with_pairing_shown(&self, id: &str, mut info: ShareInfo) -> ShareInfo {
        let shown = self.with_pairing_driver(id, |driver| driver.shown(info.state, info.port, None));
        info.pairing = shown.pairing;
        info.pairing_until_ms = shown.pairing_until_ms;
        info.pending = shown.pending;
        info.paired = shown.paired;
        info.closed_reason = shown.closed_reason;
        info
    }

    /// The machine's rmhost went: a pairing that was open is closed, for no reason of its own.
    pub(super) fn pairing_ended(&self, id: &str) {
        self.pairing_says(id, &PairingEvent::PairingClosed { reason: CloseReason::Other });
    }
}
