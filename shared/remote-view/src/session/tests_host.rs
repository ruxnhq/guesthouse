//! A host that keeps pairing's time itself, driven by what it says: told words, never asked.

use super::*;
use crate::share::CloseReason;
use std::cell::{Cell, RefCell};

const PHONE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

/// A host that is told words: it writes each down, has no way to say `allow` words, and is never
/// asked anything.
#[derive(Default)]
struct Pusher {
    sent: RefCell<Vec<String>>,
    logged: RefCell<Vec<String>>,
    asked: Cell<usize>,
}

impl Guest for Pusher {
    fn ctl(&self, _words: &Words) -> Result<String, Unanswered> {
        self.asked.set(self.asked.get() + 1);
        Err(Unanswered("asked".into()))
    }

    fn log(&self, line: &str) {
        self.logged.borrow_mut().push(line.to_string());
    }

    fn send(&self, words: &Words) -> Result<(), NotDelivered> {
        let line = words.join(" ");
        if line.contains(" allow ") {
            return Err(NotDelivered("no such line".into()));
        }
        self.sent.borrow_mut().push(line);
        Ok(())
    }
}

#[test]
fn a_host_that_says_what_happens_drives_its_pairing_with_no_reading_and_no_clock() {
    let sessions = Sessions::default();
    let guest = Pusher::default();
    let policy = Policy::linux();
    let clock = Cell::new(1_000_000u64);
    let reads = Cell::new(0);
    let now = || {
        reads.set(reads.get() + 1);
        clock.get()
    };
    let driver = Driver { sessions: &sessions, machine: "m", policy: &policy, guest: &guest, now: &now };

    assert_eq!(driver.host_event(Event::Open { seconds: 120 }), None);
    assert_eq!(*guest.sent.borrow(), ["pair on 120"]);
    assert_eq!(driver.host_says(&PairingEvent::PairingOpen { until_unix_seconds: 1_120 }), None);
    assert_eq!(driver.poll_ms(), None);
    let shown = driver.shown(ShareState::Listening, Some(7792), None);
    assert!(shown.pairing);
    assert_eq!(shown.pairing_until_ms, Some(1_120_000));

    clock.set(1_010_000);
    driver.host_says(&PairingEvent::DeviceWaits {
        name: "Phone".into(),
        fingerprint: PHONE.into(),
        code: "123 456".into(),
        seconds: 58,
    });
    let shown = driver.shown(ShareState::Listening, Some(7792), None);
    assert!(!shown.pairing);
    assert_eq!(shown.pending.as_ref().map(|pending| pending.seconds), Some(58));
    assert_eq!(driver.host_event(Event::Accept), None);
    driver.host_says(&PairingEvent::Paired { fingerprint: PHONE.into(), name: "Phone".into() });
    driver.host_says(&PairingEvent::PairingClosed { reason: CloseReason::Paired });
    let shown = driver.shown(ShareState::Listening, Some(7792), None);
    assert_eq!((shown.closed_reason, shown.paired.as_deref()), (Some(CloseReason::Paired), Some("Phone")));

    // `pair accept` went; the policy's `allow` words did not, and that was logged, not refused.
    assert_eq!(*guest.sent.borrow(), ["pair on 120".to_string(), format!("pair accept {PHONE}")]);
    assert_eq!(guest.logged.borrow().len(), 2);
    assert!(guest.logged.borrow().iter().all(|line| line.ends_with(": no such line")));
    // Never asked, and the clock read once per call.
    assert_eq!(guest.asked.get(), 0);
    assert_eq!(reads.get(), 6);
}

#[test]
fn a_process_that_ended_closes_as_other_and_the_next_starts_closed() {
    let sessions = Sessions::default();
    let guest = Pusher::default();
    let policy = Policy::linux();
    let now = || 5_000_000u64;
    let driver = Driver { sessions: &sessions, machine: "m", policy: &policy, guest: &guest, now: &now };
    driver.host_says(&PairingEvent::PairingOpen { until_unix_seconds: 5_100 });
    driver.host_says(&PairingEvent::PairingClosed { reason: CloseReason::Other });
    assert_eq!(driver.shown(ShareState::Listening, None, None).closed_reason, Some(CloseReason::Other));
    driver.host_started();
    let shown = driver.shown(ShareState::Listening, None, None);
    assert!(!shown.pairing && shown.closed_reason.is_none());
    assert_eq!(driver.host_event(Event::Open { seconds: 60 }), None);
    assert_eq!(guest.asked.get(), 0);
}

#[test]
fn a_pairing_read_from_status_is_not_told() {
    let sessions = Sessions::default();
    let guest = Pusher::default();
    let policy = Policy::linux();
    let now = || 1u64;
    let driver = Driver { sessions: &sessions, machine: "m", policy: &policy, guest: &guest, now: &now };
    sessions.with("m", |session| session.pairing = Some(Pairing::new(policy.clone())));
    assert_eq!(
        driver.host_says(&PairingEvent::PairingClosed { reason: CloseReason::Asked }).as_deref(),
        Some(NOT_HOST_TIMED)
    );
    assert_eq!(driver.host_event(Event::Stop).as_deref(), Some(NOT_HOST_TIMED));
    assert!(guest.sent.borrow().is_empty());
}

#[test]
fn a_host_that_answers_gives_its_words_through_ctl_by_default() {
    struct Answers(RefCell<Vec<String>>);
    impl Guest for Answers {
        fn ctl(&self, words: &Words) -> Result<String, Unanswered> {
            self.0.borrow_mut().push(words.join(" "));
            Ok("ok".into())
        }
        fn log(&self, _line: &str) {}
    }
    let guest = Answers(RefCell::new(Vec::new()));
    assert_eq!(guest.send(&ctl::pair_open()), Ok(()));
    assert_eq!(*guest.0.borrow(), ["pair on"]);
}
