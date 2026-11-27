//! The events a driver's step gives, from every change it kept.

use super::*;
use crate::events::PairingEvent;
use crate::pairing::Event;
use crate::share::CloseReason;
use std::cell::{Cell, RefCell};

const PHONE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

/// A guest whose host says `status` from a line the test sets, and `ok` to anything else, unless it
/// is told to lose the next command's answer.
#[derive(Default)]
struct Fake {
    status: RefCell<String>,
    lose: Cell<bool>,
}

impl Guest for Fake {
    fn ctl(&self, words: &Words) -> Result<String, Unanswered> {
        if words.join(" ") == "status" {
            return Ok(self.status.borrow().clone());
        }
        if self.lose.take() {
            return Err(Unanswered("lost".into()));
        }
        Ok("ok".into())
    }

    fn log(&self, _line: &str) {}
}

fn status(pairing: bool, pending: Option<(&str, &str, u64)>, devices: &[(&str, &str)]) -> String {
    let pending = pending.map(|(fingerprint, name, seconds)| {
        serde_json::json!({"name": name, "fingerprint": fingerprint, "code": "123 456", "seconds": seconds})
    });
    let devices: Vec<_> = devices
        .iter()
        .map(|(fingerprint, name)| serde_json::json!({"fingerprint": fingerprint, "name": name}))
        .collect();
    serde_json::json!({"pairing": pairing, "pending": pending, "devices": devices}).to_string()
}

#[test]
fn a_step_gives_the_events_of_every_change_it_kept_and_step_alone_is_unchanged() {
    let sessions = Sessions::default();
    let guest = Fake { status: RefCell::new(status(false, None, &[])), ..Fake::default() };
    let policy = Policy { hold_host_wide: false, hide: None };
    let clock = Cell::new(1_700_000_000_000u64);
    let now = || clock.get();
    let driver = Driver { sessions: &sessions, machine: "w", policy: &policy, guest: &guest, now: &now };

    assert_eq!(driver.step_with_events(StepEvent::Read), (None, Vec::new()));
    let (refused, events) = driver.step_with_events(StepEvent::Plain(Event::Open { seconds: 120 }));
    assert_eq!(refused, None);
    assert_eq!(events, [PairingEvent::PairingOpen { until_unix_seconds: 1_700_000_120 }]);

    *guest.status.borrow_mut() = status(false, Some((PHONE, "Phone", 60)), &[]);
    clock.set(clock.get() + 1_000);
    let (_, events) = driver.step_with_events(StepEvent::Read);
    assert_eq!(
        events,
        [PairingEvent::DeviceWaits {
            name: "Phone".into(),
            fingerprint: PHONE.into(),
            code: "123 456".into(),
            seconds: 60
        }]
    );

    // Accepting sends `pair accept` and reads again; the device is among the devices by then, so the
    // one step says it paired and pairing closed.
    *guest.status.borrow_mut() = status(false, None, &[(PHONE, "Phone")]);
    let (refused, events) = driver.step_with_events(StepEvent::Plain(Event::Accept));
    assert_eq!(refused, None);
    assert_eq!(
        events,
        [
            PairingEvent::Paired { fingerprint: PHONE.into(), name: "Phone".into() },
            PairingEvent::PairingClosed { reason: CloseReason::Paired }
        ]
    );
}

#[test]
fn a_round_that_did_not_go_out_says_nothing() {
    let sessions = Sessions::default();
    let guest = Fake { status: RefCell::new(status(false, None, &[])), ..Fake::default() };
    let policy = Policy { hold_host_wide: false, hide: None };
    let now = || 1_000_000u64;
    let driver = Driver { sessions: &sessions, machine: "w", policy: &policy, guest: &guest, now: &now };
    driver.step(StepEvent::Read);
    guest.lose.set(true);
    let (refused, events) = driver.step_with_events(StepEvent::Plain(Event::Open { seconds: 60 }));
    assert!(refused.is_some());
    assert!(events.is_empty());
    assert!(!sessions.with("w", |session| session.pairing.as_ref().is_some_and(Pairing::busy)));
}

#[test]
fn opening_through_the_driver_says_it_opened() {
    let sessions = Sessions::default();
    let guest = Fake { status: RefCell::new(status(false, None, &[])), ..Fake::default() };
    let policy = Policy { hold_host_wide: false, hide: None };
    let now = || 2_000_000u64;
    let driver = Driver { sessions: &sessions, machine: "w", policy: &policy, guest: &guest, now: &now };
    assert_eq!(
        driver.open_pairing_with_events(60),
        (None, vec![PairingEvent::PairingOpen { until_unix_seconds: 2_060 }])
    );
    // Asked again while open: a new end, said again.
    assert_eq!(
        driver.open_pairing_with_events(120),
        (None, vec![PairingEvent::PairingOpen { until_unix_seconds: 2_120 }])
    );
}
