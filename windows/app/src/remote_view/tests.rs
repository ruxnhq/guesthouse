use super::*;
use guesthouse_remote_view_core::share::{CloseReason, Pending, Permissions};
use std::net::Ipv4Addr;

const NOW: u64 = 1_800_000_000_000;
const FINGERPRINT: &str = "9f2c41ab07d3e85c9f2c41ab07d3e85c9f2c41ab07d3e85c9f2c41ab07d3e85c";

fn running() -> Facts {
    Facts { running: true, bridged: false, switch: true, port: Some(7792) }
}

fn listening() -> ShareInfo {
    ShareInfo {
        state: ShareState::Listening,
        port: Some(7792),
        fingerprint: Some(FINGERPRINT.to_owned()),
        ..ShareInfo::off("win")
    }
}

fn device(name: &str, connected: bool) -> DeviceInfo {
    DeviceInfo {
        fingerprint: FINGERPRINT.to_owned(),
        name: name.to_owned(),
        connected,
        permissions: Permissions::default(),
    }
}

fn waiting(seconds: u64) -> ShareInfo {
    ShareInfo {
        pending: Some(Pending {
            name: "Phone".into(),
            fingerprint: "ab".repeat(32),
            code: "482 917".into(),
            seconds,
        }),
        ..listening()
    }
}

fn viewing(info: ShareInfo) -> Viewing {
    let mut viewing = Viewing::new("win");
    viewing.take(info, Some(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20))), NOW);
    viewing
}

#[test]
fn each_control_asks_the_service_in_its_words() {
    let words = |ask: Ask| serde_json::to_value(ask.request("win")).unwrap();
    assert_eq!(
        words(Ask::Switch(true)),
        serde_json::json!({"ask": "share", "name": "win", "on": true, "pairing": null, "pairing_seconds": null})
    );
    assert_eq!(
        words(Ask::OpenPairing),
        serde_json::json!({"ask": "share", "name": "win", "on": true, "pairing": true, "pairing_seconds": 120})
    );
    assert_eq!(words(Ask::StopPairing), serde_json::json!({"ask": "pair_close", "name": "win"}));
    assert_eq!(words(Ask::Accept), serde_json::json!({"ask": "pair_accept", "name": "win"}));
    assert_eq!(words(Ask::Decline), serde_json::json!({"ask": "pair_decline", "name": "win"}));
    assert_eq!(
        words(Ask::Allow { fingerprint: "ab".into(), what: "clipboard", on: true }),
        serde_json::json!({"ask": "device_allow", "name": "win", "fingerprint": "ab", "what": "clipboard", "on": true})
    );
    assert_eq!(
        words(Ask::Revoke("ab".into())),
        serde_json::json!({"ask": "device_revoke", "name": "win", "fingerprint": "ab"})
    );
}

#[test]
fn off_shows_the_switch_and_nothing_else() {
    let facts = Facts { switch: false, ..running() };
    let shown = Viewing::new("win").shown(&facts);
    assert!(!shown.on);
    assert_eq!(shown.headline, "Off. Nothing is listening for other devices.");
    assert_eq!((shown.address, shown.pairing, shown.devices.len()), (None, None, 0));
}

#[test]
fn on_and_stopped_keeps_its_port_and_cant_pair_yet() {
    let facts = Facts { running: false, ..running() };
    let mut viewing = viewing(ShareInfo { state: ShareState::Starting, ..listening() });
    viewing.address = None;
    let shown = viewing.shown(&facts);
    assert_eq!(shown.headline, "On. The screen is shared once the machine is running.");
    assert_eq!(shown.address.as_deref(), Some("7792"));
    assert_eq!(shown.pairing, Some(PairingShown::Closed { text: "Closed".into(), can_open: false }));
}

#[test]
fn the_list_says_on_before_any_answer_has_come() {
    let shown = Viewing::new("win").shown(&running());
    assert!(shown.on);
    assert_eq!(shown.address.as_deref(), Some("7792"));
}

#[test]
fn ready_shows_the_address_to_type_and_pairing_can_open() {
    let shown = viewing(listening()).shown(&running());
    assert_eq!(shown.headline, "Ready. A paired device can connect.");
    assert_eq!(shown.address.as_deref(), Some("192.168.1.20:7792"));
    assert_eq!(shown.fingerprint.as_deref(), Some(FINGERPRINT));
    assert_eq!(shown.pairing, Some(PairingShown::Closed { text: "Closed".into(), can_open: true }));
    assert!(!shown.held);
}

#[test]
fn open_pairing_counts_down_and_holds_the_clipboard() {
    let until = NOW + 95_000;
    let shown =
        viewing(ShareInfo { pairing: true, pairing_until_ms: Some(until), ..listening() }).shown(&running());
    assert_eq!(shown.pairing, Some(PairingShown::Open { until_ms: Some(until) }));
    assert!(shown.held);
    assert_eq!(pairing_left(Some(until), NOW), "Closes in 1:35");
    assert_eq!(pairing_left(Some(until), until), proposed::CLOSING);
}

#[test]
fn a_waiting_device_counts_down_from_when_it_was_first_seen() {
    let mut viewing = viewing(waiting(60));
    let Some(PairingShown::Waiting { name, code, until_ms }) = viewing.shown(&running()).pairing else {
        panic!("not waiting")
    };
    assert_eq!((name.as_str(), code.as_str(), until_ms), ("Phone", "482 917", NOW + 60_000));
    // A later reading says fewer seconds, read late: the time to answer stays where it was.
    viewing.take(waiting(50), None, NOW + 12_000);
    assert!(
        matches!(viewing.shown(&running()).pairing, Some(PairingShown::Waiting { until_ms, .. }) if until_ms == NOW + 60_000)
    );
    assert_eq!(waiting_left(NOW + 60_000, NOW + 8_000), "52 s left to answer");
    assert!(viewing.shown(&running()).held);
}

#[test]
fn another_device_waiting_gets_its_own_time() {
    let mut viewing = viewing(waiting(60));
    let mut other = waiting(60);
    other.pending.as_mut().unwrap().code = "111 222".into();
    viewing.take(other, None, NOW + 30_000);
    assert_eq!(viewing.waiting_until.as_ref().unwrap().1, NOW + 90_000);
}

#[test]
fn readings_that_differ_only_in_the_seconds_left_are_the_same() {
    assert_eq!(settled(&waiting(60)), settled(&waiting(41)));
    assert_ne!(settled(&waiting(60)), settled(&listening()));
}

#[test]
fn how_pairing_closed_is_said_with_pair_a_new_device() {
    let info =
        ShareInfo { closed_reason: Some(CloseReason::Paired), paired: Some("Phone".into()), ..listening() };
    assert_eq!(
        viewing(info).shown(&running()).pairing,
        Some(PairingShown::Closed { text: "Closed. Phone was paired.".into(), can_open: true })
    );
}

#[test]
fn failed_hides_pairing_and_says_why_with_the_devices_still_listed() {
    let info = ShareInfo {
        state: ShareState::Failed,
        reason: Some("Port 7792 is already in use by another program.".into()),
        devices: vec![device("Phone", false)],
        ..listening()
    };
    let shown = viewing(info).shown(&running());
    assert_eq!(
        shown.headline,
        "The screen could not be shared: Port 7792 is already in use by another program."
    );
    assert_eq!(shown.pairing, None);
    assert_eq!(shown.devices.len(), 1);
}

#[test]
fn a_device_viewing_has_the_screen_in_any_placement_and_it_doesnt_come_back_by_itself() {
    let info = ShareInfo {
        viewer: Some("Phone".into()),
        viewer_placement: Some("only".into()),
        devices: vec![device("Phone", true)],
        ..listening()
    };
    let shown = viewing(info).shown(&running());
    assert_eq!(shown.headline, "Phone is viewing this machine.");
    assert_eq!(shown.viewer.as_deref(), Some("Phone"));
    let takes = proposed::takes_screen("Phone");
    assert!(
        takes.contains("Open screen takes it back") && takes.contains("Neither connects again by itself")
    );
    assert!(!takes.contains("comes back when"));
    assert_eq!(
        proposed::screen_line("Phone"),
        "Phone has this machine's screen now. Open screen takes it back."
    );
    assert_eq!(shown.devices[0].line, "Viewing now \u{b7} 9f2c 41ab");
}

#[test]
fn a_device_has_a_switch_for_each_permission_with_the_shared_labels() {
    let mut info = listening();
    info.devices.push(DeviceInfo {
        permissions: Permissions { clipboard: false, files: false, ..Permissions::default() },
        ..device("", false)
    });
    let shown = viewing(info).shown(&running());
    let device = &shown.devices[0];
    assert_eq!(device.name, "9f2c 41ab 07d3 e85c\u{2026}");
    let labels: Vec<(&str, bool)> = device.permissions.iter().map(|p| (p.label, p.on)).collect();
    assert_eq!(
        labels,
        [
            ("Control", true),
            ("Clipboard", false),
            ("Files", false),
            ("Sound", true),
            ("Microphone", true),
            ("Camera", true)
        ]
    );
}

#[test]
fn a_switch_shows_what_was_asked_until_the_answer() {
    let mut viewing = viewing(listening());
    let ask = Ask::Allow { fingerprint: FINGERPRINT.into(), what: "clipboard", on: false };
    viewing.info.as_mut().unwrap().devices.push(device("Phone", false));
    viewing.asked(&ask);
    assert!(!viewing.shown(&running()).devices[0].permissions[1].on);
    // Refused: it goes back to what the service last said, and the refusal is said.
    assert_eq!(viewing.answered(&ask, Err("No.".into()), NOW).as_deref(), Some("No."));
    assert!(viewing.shown(&running()).devices[0].permissions[1].on);
}

#[test]
fn a_refused_switch_goes_back_and_says_why_beside_it() {
    let facts = Facts { switch: false, ..running() };
    let mut viewing = Viewing::new("win");
    viewing.asked(&Ask::Switch(true));
    let shown = viewing.shown(&facts);
    assert!(shown.on && !shown.switch_enabled);
    let reason = "win has no network, so no other device can reach it. Turn its network on first.";
    assert_eq!(viewing.answered(&Ask::Switch(true), Err(reason.into()), NOW), None);
    let shown = viewing.shown(&facts);
    assert!(!shown.on && shown.switch_enabled);
    assert_eq!(shown.refused.as_deref(), Some(reason));
    // Turning it again clears what was said.
    viewing.asked(&Ask::Switch(true));
    assert_eq!(viewing.shown(&facts).refused, None);
}

#[test]
fn an_answer_about_another_machine_is_let_go() {
    let mut viewing = Viewing::new("win");
    viewing.take(ShareInfo::off("other"), None, NOW);
    assert_eq!(viewing.info, None);
}

#[test]
fn pairing_left_open_is_known_for_when_the_app_goes() {
    assert!(!viewing(listening()).pairing_open());
    assert!(viewing(ShareInfo { pairing: true, ..listening() }).pairing_open());
    assert!(viewing(waiting(60)).pairing_open());
}

#[test]
fn bridged_says_so() {
    assert!(viewing(listening()).shown(&Facts { bridged: true, ..running() }).bridged);
}

#[test]
fn a_preview_pairs_a_device_with_clipboard_and_files_off() {
    let opened = pretend(&listening(), &Ask::OpenPairing, NOW).unwrap();
    assert!(opened.pairing && opened.pairing_until_ms == Some(NOW + 120_000));
    let accepted = pretend(&waiting(60), &Ask::Accept, NOW).unwrap();
    assert_eq!(accepted.devices.len(), 1);
    assert!(!accepted.devices[0].permissions.clipboard && !accepted.devices[0].permissions.files);
    assert_eq!(accepted.closed_reason, Some(CloseReason::Paired));
    assert!(pretend(&listening(), &Ask::Decline, NOW).is_err());
    let removed = pretend(&accepted, &Ask::Revoke(accepted.devices[0].fingerprint.clone()), NOW).unwrap();
    assert!(removed.devices.is_empty());
}

#[test]
fn a_code_is_read_digit_by_digit() {
    assert_eq!(spoken_code("482 917"), "Code 4 8 2 9 1 7");
    assert_eq!(proposed::clock(9), "0:09");
}

#[test]
fn a_reason_beside_a_working_state_is_a_note_and_not_a_failure() {
    let reason = "It isn't listed by name on the network, as another device has that name. Its address and port still work.";
    let shown = viewing(ShareInfo { reason: Some(reason.into()), ..listening() }).shown(&running());
    assert_eq!(shown.headline, "Ready. A paired device can connect.");
    assert_eq!(shown.note.as_deref(), Some(reason));
    assert!(shown.pairing.is_some());
    let failed = viewing(ShareInfo { state: ShareState::Failed, reason: Some(reason.into()), ..listening() });
    assert_eq!(failed.shown(&running()).note, None);
}
