//! Pairing a new device with a running machine's remoteman, and changing the devices it trusts,
//! through the shared pairing driver with the Windows policy: Guesthouse's own viewer is kept out
//! of sight and out of reach, and the host-wide clipboard and files are held off while pairing is
//! open, as on Linux.
//!
//! remoteman's host has no time limit on pairing, turns a waiting device away if nobody reads its
//! status for 30 seconds, and holds it for at most a minute. So while pairing is open or a device
//! waits, a thread of the machine's own reads the status about once a second and lets pairing's
//! clock run, whether or not the app is asking. The shared `Pairing` decides everything else: when
//! the time is up, that one device closes it, and what is sent.
//!
//! What pairing holds off is kept in memory only. The service never outlives its machines (it stops
//! them as it stops, and ends any it finds running as it starts), and every start of a Windows guest
//! turns the host-wide clipboard and files on again (`screen.ps1`), so nothing held can be left off
//! past the machine's run. A pairing found open when the keeper starts, or a device found waiting,
//! is closed or met as the driver's `started` does.
//!
//! The devices the user removed are kept in the machine's own folder (`revoked`), as a snapshot
//! gone back to brings back the guest's list from before. Once the guest first answers in each run,
//! it's told again to stop trusting each of them, and the machine's view waits for that.
//!
//! There is no `drop` here: remoteman's `drop` cuts whichever device leads, which may be
//! Guesthouse's own window, and the serial channel doesn't carry it.

use super::remote_view::policy;
use super::{Service, Work, no_machine};
use guesthouse_control::Answer;
use guesthouse_host_windows::guest_ctl::GuestCtl;
use guesthouse_remote_view_core::ctl::Words;
use guesthouse_remote_view_core::pairing::{Event, Pairing};
use guesthouse_remote_view_core::policy::Policy;
use guesthouse_remote_view_core::revoked;
use guesthouse_remote_view_core::session::StepEvent;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// How often the pairing keeper looks at its machine.
const LOOK: Duration = Duration::from_millis(100);
/// How often it reads the guest until the first reading of a run comes back, which is what closes
/// a pairing found open.
const FIRST_READING_EVERY: Duration = Duration::from_secs(5);

pub const NOT_RUNNING: &str = "The machine isn't running.";
pub const NOT_ON: &str = "Viewing from other devices is off. Turn it on first.";

impl<W: Work> Service<W> {
    /// Opens pairing on the running machine `name` for `seconds`, once it's read.
    pub(super) fn open_pairing(&self, name: &str, seconds: u64) -> Result<(), String> {
        let (on, ctl) = {
            let entries = self.machines.lock();
            let entry = &entries[name];
            (entry.remote_view.allow_remote_view, entry.ctl.clone())
        };
        if !on {
            return Err(NOT_ON.into());
        }
        let ctl = ctl.ok_or_else(|| NOT_RUNNING.to_owned())?;
        match self.drive(name, &ctl, |driver| driver.open_pairing(seconds)) {
            Some(why) => Err(why),
            None => Ok(()),
        }
    }

    /// Feeds one of the person's answers to the machine's pairing: stop, accept or decline.
    pub(super) fn pairing_event(self: &Arc<Self>, asked: &str, event: Event<'static>) -> Answer {
        let Some(name) = self.machines.find(asked) else { return no_machine(asked) };
        let Some(ctl) = self.running_ctl(&name) else {
            return Answer::Refused { reason: NOT_RUNNING.into() };
        };
        if let Some(reason) = self.drive(&name, &ctl, |driver| driver.step(StepEvent::Plain(event))) {
            return Answer::Refused { reason };
        }
        self.remote_view(&name)
    }

    /// Lets in the device waiting to pair, which is no longer one that was removed.
    pub(super) fn accept(self: &Arc<Self>, asked: &str) -> Answer {
        let name = self.machines.find(asked);
        let waiting = name.as_ref().and_then(|name| {
            self.sessions.with(name, |session| {
                session.reading.as_ref()?.pending.as_ref().map(|pending| pending.fingerprint.clone())
            })
        });
        let answer = self.pairing_event(asked, Event::Accept);
        if let (Answer::Share { .. }, Some(name), Some(fingerprint)) = (&answer, name, waiting)
            && let Err(error) = revoked::forget(&self.machines.store.folder(&name), &fingerprint)
        {
            self.work.log(&format!(
                "{name}: a device paired again couldn't be taken off its removed list: {error}"
            ));
        }
        answer
    }

    /// Stops trusting a device, and keeps it on the machine's list of removed devices, which each
    /// start tells the guest again.
    pub(super) fn revoke_device(self: &Arc<Self>, asked: &str, fingerprint: &str) -> Answer {
        let answer = self.device(asked, |policy| policy.device_revoke(fingerprint));
        if let (Answer::Share { .. }, Some(name)) = (&answer, self.machines.find(asked))
            && let Err(error) = revoked::add(&self.machines.store.folder(&name), fingerprint)
        {
            self.work.log(&format!("{name}: a removed device couldn't be kept on its list: {error}"));
        }
        answer
    }

    /// Tells the running guest again to stop trusting each device removed from it, and lets its view
    /// open once each was answered. Whether it was: one the guest didn't answer is told again later.
    fn tell_removed(&self, name: &str, ctl: &GuestCtl) -> bool {
        let policy = policy(&self.machines.store.disk(name));
        let mut answered = true;
        for fingerprint in revoked::removed(&self.machines.store.folder(name)) {
            let Ok(words) = policy.device_revoke(&fingerprint) else { continue };
            if let Err(reason) = self.drive(name, ctl, |driver| driver.device(&words)) {
                self.work.log(&format!("{name}: a removed device wasn't revoked again: {reason}"));
            }
            answered &= !self.sessions.with(name, |session| session.unsure);
        }
        if answered && let Some(entry) = self.machines.lock().get_mut(name) {
            entry.removed_told = true;
        }
        answered
    }

    /// Sends one command about a paired device, made by `words` with the machine's policy, which
    /// refuses Guesthouse's own viewer.
    pub(super) fn device(
        self: &Arc<Self>,
        asked: &str,
        words: impl FnOnce(&Policy) -> Result<Words, String>,
    ) -> Answer {
        let Some(name) = self.machines.find(asked) else { return no_machine(asked) };
        let words = match words(&policy(&self.machines.store.disk(&name))) {
            Ok(words) => words,
            Err(reason) => return Answer::Refused { reason },
        };
        let Some(ctl) = self.running_ctl(&name) else {
            return Answer::Refused { reason: NOT_RUNNING.into() };
        };
        if let Err(reason) = self.drive(&name, &ctl, |driver| driver.device(&words)) {
            return Answer::Refused { reason };
        }
        self.remote_view(&name)
    }

    /// While `name` runs with its switch on: reads the guest until it first answers, which starts
    /// this run's pairing, and then about once a second while pairing is open or a device waits,
    /// until `over` is set.
    ///
    /// The first reading does what the driver's `started` does. The session is new at every start,
    /// so the first step of a run, whoever takes it, begins with a fresh pairing that owes nothing
    /// (this host keeps nothing held between runs, see the top), and its reading closes a pairing
    /// found open and meets a device found waiting. Calling `started` here as well would throw away
    /// a reading the app had just taken.
    pub(super) fn keep_pairing(&self, name: &str, over: &AtomicBool) {
        let mut last: Option<Instant> = None;
        let mut told = false;
        let mut last_told: Option<Instant> = None;
        while !over.load(Ordering::SeqCst) {
            thread::sleep(LOOK);
            let (on, ctl) = match self.machines.lock().get(name) {
                Some(entry) => (entry.remote_view.allow_remote_view, entry.ctl.clone()),
                None => return,
            };
            let Some(ctl) = ctl.filter(|_| on) else { continue };
            let answered = self.sessions.with(name, |session| session.reading.is_some());
            if answered && !told && last_told.is_none_or(|at| at.elapsed() >= FIRST_READING_EVERY) {
                last_told = Some(Instant::now());
                told = self.tell_removed(name, &ctl);
            }
            let poll =
                self.sessions.with(name, |session| session.pairing.as_ref().and_then(Pairing::poll_ms));
            let every = match poll {
                Some(ms) => Duration::from_millis(ms),
                None if !answered => FIRST_READING_EVERY,
                None => continue,
            };
            if last.is_none_or(|at| at.elapsed() >= every) {
                last = Some(Instant::now());
                self.read_now(name, &ctl, true);
            }
        }
    }
}
