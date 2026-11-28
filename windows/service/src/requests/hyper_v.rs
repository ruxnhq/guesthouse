//! Turning on Windows' Hyper-V feature, which bridging needs, for a user who isn't an administrator:
//! the service runs as the system, so it does it with DISM, through [`Network::enable_hyper_v`], and
//! never restarts the PC. It's done only while no machine runs, is being made or has work going on,
//! as what turning it on does to a running machine before the restart hasn't been seen, and the
//! restart that finishes it stops every machine anyway; and no machine starts or is made while it's
//! being done. While it's done, and after it failed, `Status` says so, from any window.
//!
//! [`Network::enable_hyper_v`]: guesthouse_host_windows::network::Network::enable_hyper_v

use super::{Connection, Service, Work};
use guesthouse_control::{Answer, Bridging, Event, HyperV, State};
use guesthouse_host_windows::hyper_v::Enabled;
use guesthouse_host_windows::network::not_offered;
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;

use super::CHECK;

/// How turning Hyper-V on stands in the service, beside what Windows says of the feature.
#[derive(Debug, Default)]
pub(super) struct Turning {
    /// Being turned on now, and how far DISM last said it had got.
    pub on: Option<Option<u8>>,
    /// Why the last try failed, and DISM's log, until another try or a restart of the service.
    pub failed: Option<(String, String)>,
}

/// Why turning Hyper-V on waits for the machines, in words.
const WHY_STOPPED: &str = "Hyper-V is turned on only while no machine is running or being worked on: what turning it \
                           on does to a running machine hasn't been tried, and the restart that finishes it stops \
                           every machine anyway.";

/// `names` as a list in words: `a`, `a and b`, `a, b and c`.
fn listed(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

impl<W: Work> Service<W> {
    /// What bridging offers, with where turning Hyper-V on stands in the service laid over what
    /// Windows says: being turned on now, or failed last time while it's still off.
    pub(super) fn with_turning(&self, mut bridging: Bridging) -> Bridging {
        let turning = self.turning.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let laid = match (&turning.on, &turning.failed, &bridging.hyper_v) {
            (Some(percent), _, _) => Some(HyperV::TurningOn { percent: *percent }),
            (None, Some((reason, log)), HyperV::Off | HyperV::Unknown { .. }) => {
                Some(HyperV::Failed { reason: reason.clone(), log: Some(log.clone()) })
            }
            _ => None,
        };
        if let Some(hyper_v) = laid {
            bridging.available = false;
            bridging.adapters.clear();
            bridging.reason = Some(not_offered(&hyper_v));
            bridging.hyper_v = hyper_v;
        }
        bridging
    }

    /// Whether Hyper-V is being turned on now, when no machine may start or be made.
    pub(super) fn turning_on(&self) -> bool {
        self.turning.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).on.is_some()
    }

    /// Why Hyper-V can't be turned on now for the machines, if it can't: the ones that are running
    /// or being worked on, or the one being made. Marks it as being turned on otherwise, with the
    /// machines held, so none starts between the look and the mark.
    fn hold_for_turning(&self) -> Result<(), String> {
        let entries = self.machines.lock();
        // Held to the end, as a machine being made checks for this under it.
        let making = self.making.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(making) = &*making {
            return Err(format!("{making} is being made. {WHY_STOPPED} Turn it on once it's made."));
        }
        let busy: Vec<String> = entries
            .iter()
            .filter(|(_, entry)| {
                entry.stop.is_some()
                    || entry.busy.is_some()
                    || !matches!(entry.state, State::Off | State::Failed)
            })
            .map(|(name, _)| name.clone())
            .collect();
        if !busy.is_empty() {
            let are = if busy.len() == 1 { "is" } else { "are" };
            return Err(format!(
                "{} {are} running or being worked on. {WHY_STOPPED} Stop {} first.",
                listed(&busy),
                if busy.len() == 1 { "it" } else { "them" }
            ));
        }
        let mut turning = self.turning.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if turning.on.is_some() {
            return Err("Hyper-V is being turned on already.".to_owned());
        }
        turning.on = Some(None);
        turning.failed = None;
        Ok(())
    }

    /// Turns Hyper-V on, answering with how far it has got as it goes and then `Done`. Asked when
    /// it's on already or waits for a restart, it answers `Done` at once and runs nothing.
    pub(super) fn enable_hyper_v(self: &Arc<Self>, connection: &mut dyn Connection) {
        let refuse = |connection: &mut dyn Connection, reason: String| {
            let _ = connection.send(&Answer::Refused { reason });
        };
        match self.bridging().hyper_v {
            HyperV::On => {
                let _ =
                    connection.send(&Answer::Event { event: Event::Note("Hyper-V is on already.".into()) });
                let _ = connection.send(&Answer::Done { reason: None });
                return;
            }
            HyperV::RestartPending => {
                let note = "Hyper-V is turned on already. Windows has to restart to finish it.";
                let _ = connection.send(&Answer::Event { event: Event::Note(note.into()) });
                let _ = connection.send(&Answer::Done { reason: None });
                return;
            }
            HyperV::NotInEdition => {
                return refuse(
                    connection,
                    "This edition of Windows doesn't have Hyper-V: Windows Home has none.".into(),
                );
            }
            HyperV::NoVirtualization => {
                return refuse(
                    connection,
                    "This PC's processor says virtualization is off in its firmware, or it has none, so Hyper-V \
                     wouldn't run. Turn virtualization on in the firmware's settings first."
                        .into(),
                );
            }
            HyperV::TurningOn { .. } => {
                return refuse(connection, "Hyper-V is being turned on already.".into());
            }
            HyperV::Off | HyperV::Failed { .. } | HyperV::Unknown { .. } | HyperV::Unsaid => {}
        }
        if let Err(reason) = self.hold_for_turning() {
            return refuse(connection, reason);
        }
        self.work.log("Turning on Windows' Hyper-V feature, as the app asked.");
        let (told, heard) = mpsc::channel::<u8>();
        let service = Arc::clone(self);
        let work = thread::spawn(move || {
            service.work.network().enable_hyper_v(&mut |percent| {
                service.turning.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).on =
                    Some(Some(percent));
                let _ = told.send(percent);
            })
        });
        let mut window_gone = false;
        let mut pass_on = |percent: u8, connection: &mut dyn Connection| {
            let event = Event::Note(format!("Turning on Hyper-V: {percent}%"));
            if !window_gone && connection.send(&Answer::Event { event }).is_err() {
                window_gone = true;
            }
        };
        loop {
            match heard.recv_timeout(CHECK) {
                Ok(percent) => pass_on(percent, connection),
                Err(RecvTimeoutError::Timeout) if work.is_finished() => break,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        let enabled = work.join().unwrap_or_else(|_| Enabled::Failed {
            reason: "Turning on Hyper-V failed unexpectedly.".into(),
            log: String::new(),
        });
        let reason = {
            let mut turning = self.turning.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            turning.on = None;
            match enabled {
                Enabled::Done | Enabled::Already => None,
                Enabled::NotInEdition => {
                    Some("This edition of Windows doesn't have Hyper-V: Windows Home has none.".to_owned())
                }
                Enabled::Failed { reason, log } => {
                    let said = if log.is_empty() {
                        reason.clone()
                    } else {
                        format!("{reason} DISM's log is {log}.")
                    };
                    turning.failed = Some((reason, log));
                    Some(said)
                }
            }
        };
        // Windows says something new of the feature now, so it's read again at the next look.
        self.forget_bridging();
        match &reason {
            None => self.work.log("Hyper-V is turned on. Windows has to restart to finish it."),
            Some(why) => self.work.log(&format!("Hyper-V wasn't turned on: {why}")),
        }
        let _ = connection.send(&Answer::Done { reason });
    }
}

#[cfg(test)]
mod tests {
    use super::listed;

    #[test]
    fn names_are_listed_as_words() {
        let names = |list: &[&str]| list.iter().map(|name| (*name).to_owned()).collect::<Vec<_>>();
        assert_eq!(listed(&names(&[])), "");
        assert_eq!(listed(&names(&["win11"])), "win11");
        assert_eq!(listed(&names(&["win11", "ubuntu"])), "win11 and ubuntu");
        assert_eq!(listed(&names(&["a", "b", "c"])), "a, b and c");
    }
}
