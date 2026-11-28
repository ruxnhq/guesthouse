//! The calls for a host that answers `status` refuse a host-timed pairing, and do for a polled one
//! what they always did.

use super::*;
use std::cell::{Cell, RefCell};

const PHONE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

/// A host that answers: `status` from a line the test sets, `ok` to the rest, writing every line down.
struct Answers {
    status: RefCell<String>,
    asked: RefCell<Vec<String>>,
}

impl Guest for Answers {
    fn ctl(&self, words: &Words) -> Result<String, Unanswered> {
        let line = words.join(" ");
        self.asked.borrow_mut().push(line.clone());
        Ok(if line == "status" { self.status.borrow().clone() } else { "ok".into() })
    }

    fn log(&self, _line: &str) {}
}

fn answers(status: &str) -> Answers {
    Answers { status: RefCell::new(status.into()), asked: RefCell::new(Vec::new()) }
}

#[test]
fn a_host_timed_pairing_is_refused_by_every_call_that_reads_and_nothing_is_sent_or_changed() {
    let sessions = Sessions::default();
    let guest = answers(r#"{"pairing":false,"devices":[]}"#);
    let policy = Policy::linux();
    let reads = Cell::new(0);
    let now = || {
        reads.set(reads.get() + 1);
        1_000u64
    };
    let driver = Driver { sessions: &sessions, machine: "m", policy: &policy, guest: &guest, now: &now };
    let pairing = Pairing::host_timed(policy.clone());
    sessions.with("m", |session| session.pairing = Some(pairing.clone()));

    assert_eq!(HOST_TIMED, "This machine's pairing is told by its host, not read from its status.");
    assert_eq!(driver.step(StepEvent::Read).as_deref(), Some(HOST_TIMED));
    assert_eq!(driver.step(StepEvent::Plain(Event::Open { seconds: 60 })).as_deref(), Some(HOST_TIMED));
    assert_eq!(driver.open_pairing(60).as_deref(), Some(HOST_TIMED));
    assert_eq!(driver.device(&ctl::device_drop()), Err(HOST_TIMED.to_string()));
    assert_eq!(driver.read(), None);
    driver.started();

    assert!(guest.asked.borrow().is_empty());
    assert_eq!(reads.get(), 0);
    sessions.with("m", |session| {
        assert_eq!(session.pairing.as_ref(), Some(&pairing));
        assert!(session.read_at.is_none() && session.reading.is_none() && !session.unsure);
    });
}

#[test]
fn a_polled_pairing_sends_and_reads_the_clock_as_it_always_did() {
    let sessions = Sessions::default();
    let guest = answers(r#"{"pairing":false,"devices":[]}"#);
    let policy = Policy { hold_host_wide: false, hide: None };
    let reads = Cell::new(0);
    let now = || {
        reads.set(reads.get() + 1);
        1_000_000u64
    };
    let driver = Driver { sessions: &sessions, machine: "m", policy: &policy, guest: &guest, now: &now };
    driver.started();
    assert_eq!(driver.open_pairing(60), None);
    *guest.status.borrow_mut() = format!(
        r#"{{"pairing":false,"pending":{{"name":"Phone","fingerprint":"{PHONE}","code":"123 456","seconds":60}},"devices":[]}}"#
    );
    assert_eq!(driver.step(StepEvent::Read), None);
    *guest.status.borrow_mut() =
        format!(r#"{{"pairing":false,"devices":[{{"fingerprint":"{PHONE}","name":"Phone"}}]}}"#);
    assert_eq!(driver.step(StepEvent::Plain(Event::Accept)), None);
    assert_eq!(driver.device(&ctl::device_drop()), Ok(()));
    assert_eq!(
        *guest.asked.borrow(),
        [
            "status",
            "status",
            "pair on",
            "status",
            "status",
            format!("pair accept {PHONE}").as_str(),
            "status",
            format!("device {PHONE} allow clipboard off").as_str(),
            format!("device {PHONE} allow files off").as_str(),
            "status",
            "drop",
            "status",
        ]
    );
    assert_eq!(reads.get(), 15);
}
