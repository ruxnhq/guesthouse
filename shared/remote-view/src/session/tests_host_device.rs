//! Device commands for a host that times pairing itself: told, never asked; and the polled `device`
//! as it was.

use super::*;
use std::cell::RefCell;

const PHONE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

/// A host that is told words, refuses `drop` as one it has no line for, and is never asked.
#[derive(Default)]
struct Pusher {
    sent: RefCell<Vec<String>>,
    logged: RefCell<Vec<String>>,
    asked: RefCell<Vec<String>>,
}

impl Guest for Pusher {
    fn ctl(&self, words: &Words) -> Result<String, Unanswered> {
        self.asked.borrow_mut().push(words.join(" "));
        Ok("ok".into())
    }

    fn log(&self, line: &str) {
        self.logged.borrow_mut().push(line.to_string());
    }

    fn send(&self, words: &Words) -> Result<(), NotDelivered> {
        let line = words.join(" ");
        if line == "drop" {
            return Err(NotDelivered("no such line".into()));
        }
        self.sent.borrow_mut().push(line);
        Ok(())
    }
}

fn driver<'a>(sessions: &'a Sessions, guest: &'a Pusher, policy: &'a Policy) -> Driver<'a> {
    Driver { sessions, machine: "m", policy, guest, now: &|| 1_000 }
}

#[test]
fn every_device_command_goes_to_the_host_and_nothing_is_read_or_changed() {
    let (sessions, guest, policy) = (Sessions::default(), Pusher::default(), Policy::linux());
    let driver = driver(&sessions, &guest, &policy);
    driver.host_says(&PairingEvent::DeviceWaits {
        name: "Phone".into(),
        fingerprint: PHONE.into(),
        code: "123 456".into(),
        seconds: 60,
    });
    let before = sessions.with("m", |session| session.pairing.clone());
    let upper = PHONE.to_uppercase();
    for words in [
        policy.device_revoke(&upper).unwrap(),
        policy.device_allow(PHONE, "clipboard", false).unwrap(),
        policy.device_allow(PHONE, "files", true).unwrap(),
        policy.device_allow(PHONE, "input", false).unwrap(),
    ] {
        assert_eq!(driver.host_device(&words), Ok(()));
    }
    assert_eq!(
        *guest.sent.borrow(),
        [
            format!("revoke {PHONE}"),
            format!("device {PHONE} allow clipboard off"),
            format!("device {PHONE} allow files on"),
            format!("device {PHONE} view-only on"),
        ]
    );
    // Not delivered: logged with why, and given back.
    assert_eq!(driver.host_device(&ctl::device_drop()), Err("no such line".to_string()));
    assert_eq!(*guest.logged.borrow(), ["drop: no such line"]);
    assert!(guest.asked.borrow().is_empty());
    assert_eq!(sessions.with("m", |session| session.pairing.clone()), before);
    // Never the waiting device's name or code in a log line.
    assert!(guest.logged.borrow().iter().all(|line| !line.contains("Phone") && !line.contains("123")));
}

#[test]
fn a_bad_fingerprint_is_refused_before_anything_is_sent() {
    let (sessions, guest, policy) = (Sessions::default(), Pusher::default(), Policy::linux());
    let driver = driver(&sessions, &guest, &policy);
    for words in [policy.device_revoke("not-a-fingerprint"), policy.device_allow("abc", "files", false)] {
        let why = words.expect_err("refused");
        assert!(!why.is_empty());
    }
    assert!(guest.sent.borrow().is_empty() && guest.logged.borrow().is_empty());
    let _ = driver;
}

#[test]
fn a_polled_pairing_is_not_told() {
    let (sessions, guest, policy) = (Sessions::default(), Pusher::default(), Policy::linux());
    sessions.with("m", |session| session.pairing = Some(Pairing::new(policy.clone())));
    let driver = driver(&sessions, &guest, &policy);
    assert_eq!(driver.host_device(&ctl::device_drop()), Err(NOT_HOST_TIMED.to_string()));
    assert!(guest.sent.borrow().is_empty() && guest.asked.borrow().is_empty());
}

#[test]
fn the_polled_device_call_asks_what_it_always_did() {
    let (sessions, guest, policy) = (Sessions::default(), Pusher::default(), Policy::linux());
    let driver = driver(&sessions, &guest, &policy);
    assert_eq!(driver.device(&policy.device_revoke(PHONE).unwrap()), Ok(()));
    assert_eq!(driver.device(&policy.device_allow(PHONE, "input", true).unwrap()), Ok(()));
    assert_eq!(driver.device(&ctl::device_drop()), Ok(()));
    assert_eq!(
        *guest.asked.borrow(),
        [
            format!("revoke {PHONE}"),
            "status".to_string(),
            format!("device {PHONE} view-only off"),
            "status".to_string(),
            "drop".to_string(),
            "status".to_string(),
        ]
    );
    assert!(guest.sent.borrow().is_empty());
}
