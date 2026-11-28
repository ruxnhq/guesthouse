// SPDX-License-Identifier: GPL-2.0-or-later
//! Where a machine stands, and the moves between states. It is a pure function of the state and
//! an event, so every rule is tested without a process: the agent turns what happens (a request,
//! QEMU answering, QEMU exiting) into events and applies them one at a time.

use guesthouse_state_core::common::State;
use std::fmt;

/// Where a machine stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Stopped,
    /// QEMU is started but has not opened its control socket yet.
    Starting,
    Running,
    /// The guest was asked to power off, or QEMU is being forced off. `stalled` is set when it
    /// has not gone in time; only then does the card offer Force Off.
    ShuttingDown {
        stalled: bool,
    },
    /// It could not start or it stopped unexpectedly, with a one-line reason.
    Failed {
        reason: String,
    },
}

impl Status {
    /// The shared state this is. The wire name and the word shown come from the shared crate, so
    /// every host says them alike; the reason of a failure stays here, beside the state.
    pub fn shared(&self) -> State {
        match self {
            Status::Stopped => State::Off,
            Status::Starting => State::Starting,
            Status::Running => State::Running,
            Status::ShuttingDown { stalled: false } => State::Stopping,
            Status::ShuttingDown { stalled: true } => State::StoppingStalled,
            Status::Failed { .. } => State::Failed,
        }
    }

    /// The name a state goes by in messages: the shared wire name (`off`, `stopping_stalled`...).
    pub fn name(&self) -> String {
        match serde_json::to_value(self.shared()) {
            Ok(serde_json::Value::String(name)) => name,
            _ => String::new(),
        }
    }

    /// The word a state is shown with (Off, Still stopping...), the shared one.
    pub fn title(&self) -> &'static str {
        self.shared().name()
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Status::Failed { reason } => Some(reason),
            _ => None,
        }
    }

    pub fn stalled(&self) -> bool {
        matches!(self, Status::ShuttingDown { stalled: true })
    }

    /// Whether a QEMU process is, or may be, there.
    pub fn is_alive(&self) -> bool {
        matches!(self, Status::Starting | Status::Running | Status::ShuttingDown { .. })
    }
}

/// Something that happened to a machine. The first three come from the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Start,
    ShutDown,
    ForceOff,
    /// QEMU could not be started at all.
    StartRefused {
        reason: String,
    },
    /// QEMU opened its control socket.
    Ready,
    /// QEMU did not open its control socket in time.
    StartTimedOut,
    /// The guest has not gone in time after being asked, or after a force off.
    ShutdownStalled,
    /// The QEMU process is gone. `clean` is an exit with status 0, or one whose status is not
    /// known (a QEMU found again after the agent restarted).
    Exited {
        clean: bool,
        reason: String,
    },
}

/// A request that does not fit where the machine stands, in words for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused(pub String);

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The state after `event`. What the user asks for that does not fit is refused; what the
/// machinery reports late (QEMU answering after a force off, say) changes nothing.
pub fn next(status: &Status, event: &Event) -> Result<Status, Refused> {
    use Status::*;
    let unchanged = || Ok(status.clone());
    match event {
        Event::Start => match status {
            Stopped | Failed { .. } => Ok(Starting),
            _ => Err(refuse(format!("the machine is {}", words(status)))),
        },
        Event::ShutDown => match status {
            Running => Ok(ShuttingDown { stalled: false }),
            Starting => Err(refuse("the machine is still starting; use Force Off".into())),
            ShuttingDown { .. } => Err(refuse("the machine is already shutting down".into())),
            Stopped | Failed { .. } => Err(refuse("the machine is not running".into())),
        },
        Event::ForceOff => match status {
            Starting | Running | ShuttingDown { .. } => Ok(ShuttingDown { stalled: false }),
            Stopped | Failed { .. } => Err(refuse("the machine is not running".into())),
        },
        Event::StartRefused { reason } => match status {
            Starting => Ok(Failed { reason: reason.clone() }),
            _ => unchanged(),
        },
        Event::Ready => match status {
            Starting => Ok(Running),
            _ => unchanged(),
        },
        Event::StartTimedOut => match status {
            Starting => Ok(Failed { reason: "QEMU did not open its control socket in time".into() }),
            _ => unchanged(),
        },
        Event::ShutdownStalled => match status {
            ShuttingDown { .. } => Ok(ShuttingDown { stalled: true }),
            _ => unchanged(),
        },
        Event::Exited { clean, reason } => match status {
            Starting => Ok(Failed {
                reason: if reason.is_empty() {
                    "QEMU exited before it was ready".into()
                } else {
                    reason.clone()
                },
            }),
            Running if *clean => Ok(Stopped),
            Running => Ok(Failed { reason: reason.clone() }),
            // Whatever the status, it went because it was asked to.
            ShuttingDown { .. } => Ok(Stopped),
            Stopped | Failed { .. } => unchanged(),
        },
    }
}

fn refuse(message: String) -> Refused {
    Refused(message)
}

fn words(status: &Status) -> &'static str {
    match status {
        Status::Stopped => "stopped",
        Status::Starting => "already starting",
        Status::Running => "already running",
        Status::ShuttingDown { .. } => "shutting down",
        Status::Failed { .. } => "failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Status::*;

    fn exited(clean: bool, reason: &str) -> Event {
        Event::Exited { clean, reason: reason.into() }
    }

    fn failed(reason: &str) -> Status {
        Failed { reason: reason.into() }
    }

    #[test]
    fn a_normal_life() {
        let mut status = Stopped;
        for (event, expected) in [
            (Event::Start, Starting),
            (Event::Ready, Running),
            (Event::ShutDown, ShuttingDown { stalled: false }),
            (exited(true, ""), Stopped),
        ] {
            status = next(&status, &event).unwrap();
            assert_eq!(status, expected);
        }
    }

    #[test]
    fn start_is_for_stopped_and_failed_machines_only() {
        assert_eq!(next(&Stopped, &Event::Start), Ok(Starting));
        assert_eq!(next(&failed("x"), &Event::Start), Ok(Starting));
        for status in [Starting, Running, ShuttingDown { stalled: false }] {
            assert!(next(&status, &Event::Start).is_err());
        }
    }

    #[test]
    fn shut_down_needs_a_running_machine() {
        assert_eq!(next(&Running, &Event::ShutDown), Ok(ShuttingDown { stalled: false }));
        for status in [Stopped, Starting, failed("x"), ShuttingDown { stalled: true }] {
            assert!(next(&status, &Event::ShutDown).is_err(), "{status:?}");
        }
    }

    #[test]
    fn force_off_works_on_anything_alive() {
        for status in [Starting, Running, ShuttingDown { stalled: true }] {
            assert_eq!(next(&status, &Event::ForceOff), Ok(ShuttingDown { stalled: false }));
        }
        assert!(next(&Stopped, &Event::ForceOff).is_err());
        assert!(next(&failed("x"), &Event::ForceOff).is_err());
    }

    #[test]
    fn a_stall_is_only_marked_while_shutting_down() {
        assert_eq!(
            next(&ShuttingDown { stalled: false }, &Event::ShutdownStalled),
            Ok(ShuttingDown { stalled: true })
        );
        assert_eq!(next(&Running, &Event::ShutdownStalled), Ok(Running));
        assert_eq!(next(&Stopped, &Event::ShutdownStalled), Ok(Stopped));
    }

    #[test]
    fn a_failed_start_keeps_its_reason() {
        let refused = Event::StartRefused { reason: "no QEMU".into() };
        assert_eq!(next(&Starting, &refused), Ok(failed("no QEMU")));
        assert_eq!(next(&Starting, &exited(false, "cannot open disk")), Ok(failed("cannot open disk")));
        // Even a clean exit before the machine was up is a failure.
        assert_eq!(next(&Starting, &exited(true, "")), Ok(failed("QEMU exited before it was ready")));
        assert!(matches!(next(&Starting, &Event::StartTimedOut), Ok(Failed { .. })));
    }

    #[test]
    fn exiting_by_itself_is_told_from_a_crash() {
        assert_eq!(next(&Running, &exited(true, "")), Ok(Stopped));
        assert_eq!(
            next(&Running, &exited(false, "QEMU exited with status 3")),
            Ok(failed("QEMU exited with status 3"))
        );
    }

    #[test]
    fn leaving_while_shutting_down_is_a_stop_even_after_a_signal() {
        assert_eq!(next(&ShuttingDown { stalled: true }, &exited(false, "signal 15")), Ok(Stopped));
    }

    #[test]
    fn late_news_changes_nothing() {
        assert_eq!(next(&Stopped, &Event::Ready), Ok(Stopped));
        assert_eq!(next(&Running, &Event::Ready), Ok(Running));
        assert_eq!(next(&Running, &Event::StartTimedOut), Ok(Running));
        assert_eq!(next(&Stopped, &exited(true, "")), Ok(Stopped));
        assert_eq!(next(&failed("x"), &exited(true, "")), Ok(failed("x")));
        let late = Event::StartRefused { reason: "r".into() };
        assert_eq!(next(&Running, &late), Ok(Running));
    }

    #[test]
    fn names_and_reasons_for_messages() {
        assert_eq!(ShuttingDown { stalled: true }.name(), "stopping_stalled");
        assert_eq!(ShuttingDown { stalled: false }.name(), "stopping");
        assert_eq!(Stopped.name(), "off");
        assert_eq!(Stopped.title(), "Off");
        assert!(ShuttingDown { stalled: true }.stalled());
        assert!(!Running.stalled());
        assert_eq!(failed("why").reason(), Some("why"));
        assert_eq!(Running.reason(), None);
        assert!(Starting.is_alive() && !Stopped.is_alive() && !failed("x").is_alive());
    }
}
