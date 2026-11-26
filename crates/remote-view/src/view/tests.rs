use super::*;
use crate::pairing::Event;
use crate::policy::Policy;
use crate::share::{CloseReason, Pending};
use crate::status::Client;

const VIEWER: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const TABLET: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn facts() -> Facts<'static> {
    Facts {
        machine: "win",
        state: ShareState::Listening,
        port: Some(7793),
        fingerprint: Some("beef"),
        reason: None,
    }
}

fn device(fingerprint: &str, name: &str, connected: bool) -> crate::share::DeviceInfo {
    crate::share::DeviceInfo {
        fingerprint: fingerprint.into(),
        name: name.into(),
        connected,
        permissions: Default::default(),
    }
}

/// A host where the viewer and a tablet are trusted, and both are called what the host last heard
/// them call themselves.
fn status(viewer_connected: bool, tablet_connected: bool) -> Status {
    let connected = viewer_connected || tablet_connected;
    Status {
        fingerprint: Some("cafe".into()),
        devices: vec![
            device(VIEWER, "testclient", viewer_connected),
            device(TABLET, "testclient", tablet_connected),
        ],
        client: connected.then(|| Client { name: "testclient".into(), placement: None }),
        ..Status::default()
    }
}

#[test]
fn the_state_the_program_knows_and_the_reading_say_are_joined() {
    let pairing = Pairing::new(Policy::linux());
    let info = share_info(&facts(), Some(&status(false, true)), &pairing);
    assert_eq!((info.machine.as_str(), info.state, info.port), ("win", ShareState::Listening, Some(7793)));
    // The reading's fingerprint wins; the program's stands in until there is one.
    assert_eq!(info.fingerprint.as_deref(), Some("cafe"));
    assert_eq!(share_info(&facts(), None, &pairing).fingerprint.as_deref(), Some("beef"));
    assert_eq!(info.viewer.as_deref(), Some("testclient"));
    assert_eq!(info.devices.len(), 2);
    assert!(!info.pairing && info.pending.is_none() && info.closed_reason.is_none());
    assert_eq!(share_info(&facts(), None, &pairing).devices, []);
}

#[test]
fn a_failure_carries_its_reason() {
    let info = share_info(
        &Facts { state: ShareState::Failed, reason: Some("no port"), ..facts() },
        None,
        &Pairing::new(Policy::linux()),
    );
    assert_eq!((info.state, info.reason.as_deref()), (ShareState::Failed, Some("no port")));
}

#[test]
fn the_hidden_viewer_is_left_out_of_the_devices_by_fingerprint_not_by_name() {
    let policy = Policy::windows(VIEWER);
    let pairing = Pairing::new(policy.clone());
    let info = share_info(&facts(), Some(&status(false, false)), &pairing);
    let fingerprints: Vec<_> = info.devices.iter().map(|device| device.fingerprint.as_str()).collect();
    assert_eq!(fingerprints, [TABLET]);
    // The two are called the same, and only the fingerprint tells them apart.
    assert_eq!(visible_devices(&status(false, false), &policy).len(), 1);
}

#[test]
fn the_hidden_viewer_connected_is_not_shown_as_a_viewer() {
    let policy = Policy::windows(VIEWER);
    assert_eq!(viewer(&status(true, false), &policy), None);
    assert_eq!(viewer(&status(false, true), &policy).as_deref(), Some("testclient"));
    // Both at once, as when a second device joins: a person's device is connected.
    assert_eq!(viewer(&status(true, true), &policy).as_deref(), Some("testclient"));
    assert_eq!(viewer(&status(false, false), &policy), None);
    // A host with nothing to hide shows it.
    assert_eq!(viewer(&status(true, false), &Policy::linux()).as_deref(), Some("testclient"));
}

#[test]
fn pairing_shows_what_the_state_machine_has() {
    let mut pairing = Pairing::new(Policy::linux());
    let idle = Status::default();
    let mut now = 5_000_000;
    pairing = pairing.step(now, Event::Reading(&idle)).pairing;
    pairing = pairing.step(now, Event::Open { seconds: 90 }).pairing;
    let info = share_info(&facts(), Some(&idle), &pairing);
    assert!(info.pairing);
    assert_eq!(info.pairing_until_ms, Some(now + 90_000));

    now += 3_000;
    let waiting = Status {
        pending: Some(Pending {
            name: "Tab".into(),
            fingerprint: TABLET.into(),
            code: "123 456".into(),
            seconds: 57,
        }),
        ..Status::default()
    };
    pairing = pairing.step(now, Event::Reading(&waiting)).pairing;
    let info = share_info(&facts(), Some(&waiting), &pairing);
    assert!(!info.pairing && info.pairing_until_ms.is_none());
    assert_eq!(info.pending.as_ref().map(|device| device.code.as_str()), Some("123 456"));

    now += 3_000;
    pairing = pairing.step(now, Event::Decline).pairing;
    let info = share_info(&facts(), Some(&idle), &pairing);
    assert!(info.pending.is_none());
    assert_eq!(info.closed_reason, Some(CloseReason::Declined));
}

#[test]
fn the_name_of_the_device_that_paired_is_given() {
    let mut pairing = Pairing::new(Policy::linux());
    let idle = Status::default();
    pairing = pairing.step(0, Event::Reading(&idle)).pairing;
    pairing = pairing.step(0, Event::Open { seconds: 30 }).pairing;
    let paired = Status { devices: vec![device(TABLET, "Kitchen tablet", false)], ..Status::default() };
    pairing = pairing.step(1_000, Event::Reading(&paired)).pairing;
    let info = share_info(&facts(), Some(&paired), &pairing);
    assert_eq!(
        (info.paired.as_deref(), info.closed_reason),
        (Some("Kitchen tablet"), Some(CloseReason::Paired))
    );
}
