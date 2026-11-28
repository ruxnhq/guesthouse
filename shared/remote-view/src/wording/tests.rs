use super::*;
use crate::share::Permissions;

fn share(state: ShareState) -> ShareInfo {
    ShareInfo { state, ..ShareInfo::off("w") }
}

#[test]
fn the_headline_says_where_sharing_stands() {
    let viewing = ShareInfo { viewer: Some("Pat's iPad".into()), ..share(ShareState::Listening) };
    let failed = ShareInfo { reason: Some("no port".into()), ..share(ShareState::Failed) };
    let table = [
        (false, false, share(ShareState::Off), "Off. Nothing is listening for other devices."),
        (false, true, share(ShareState::Listening), "Off. Nothing is listening for other devices."),
        (true, false, share(ShareState::Off), "On. The screen is shared once the machine is running."),
        (true, true, share(ShareState::Off), "Starting to share the screen\u{2026}"),
        (true, true, share(ShareState::Starting), "Starting to share the screen\u{2026}"),
        (true, true, share(ShareState::Listening), "Ready. A paired device can connect."),
        (true, true, viewing, "Pat's iPad is viewing this machine."),
        (true, true, failed, "The screen could not be shared: no port"),
        (true, true, share(ShareState::Failed), "The screen could not be shared: Something went wrong."),
    ];
    for (allowed, running, info, want) in table {
        assert_eq!(headline(allowed, running, &info), want);
    }
}

#[test]
fn the_three_steps_of_pairing() {
    let steps = pairing_steps();
    assert_eq!(steps[0], "In remoteman on the other device, add a computer and enter the address above.");
    assert_eq!(steps[1], "If remoteman shows a fingerprint, check that it matches the one above.");
    assert!(steps[2].starts_with("The device waits, and its name and a code appear here."));
    assert!(steps[2].ends_with("Pairing closes when the device has been answered, or when the time is up."));
}

#[test]
fn what_is_said_while_pairing_is_closed() {
    let table = [
        (Some(CloseReason::Paired), Some("Kitchen tablet"), "Closed. Kitchen tablet was paired."),
        (Some(CloseReason::Paired), None, "Closed. A device was paired."),
        (
            Some(CloseReason::Declined),
            None,
            "Closed. The device was turned away. Press Pair a New Device to try again.",
        ),
        (Some(CloseReason::Timeout), None, "Closed. Nobody paired in time."),
        (Some(CloseReason::Asked), None, "Closed"),
        (Some(CloseReason::Other), Some("x"), "Closed"),
        (None, None, "Closed"),
    ];
    for (reason, paired, want) in table {
        assert_eq!(closed_text(reason, paired), want);
    }
}

#[test]
fn the_fixed_sentences_are_word_for_word() {
    assert_eq!(wants_to_pair("Pat's iPad"), "Pat's iPad wants to pair");
    assert_eq!(remove_question("Den"), "Remove Den?");
    assert_eq!(
        COMPARE_CODE_LINE,
        "Check that this code matches the one shown on the device before you press Accept."
    );
    assert_eq!(
        PAIRED_HINT,
        "A device that is already paired connects to the address above. To add another device, press Pair a New Device."
    );
    assert_eq!(NO_DEVICES, "No device has been paired yet. Press Pair a New Device to add one.");
    assert_eq!(
        REMOVE_MESSAGE,
        "It will be disconnected and will have to be paired again to view this machine."
    );
    assert_eq!(
        NETWORK_WARNING,
        "Devices on the same network can reach this machine's screen once they are paired."
    );
    assert_eq!(
        PAIRING_NOTE,
        "Only devices you accepted here can connect. Anyone on the same network can ask to pair while pairing is open."
    );
    assert_eq!(
        [ALLOW_TITLE, PAIRING_TITLE, CONNECT_TITLE, DEVICES_TITLE],
        [
            "Allow viewing from other devices",
            "Pairing",
            "Connect from remoteman",
            "Devices that can view this machine"
        ]
    );
    assert_eq!(
        [ACCEPT, DECLINE, STOP, PAIR_NEW_DEVICE, COPY_ADDRESS, DISCONNECT, REMOVE],
        ["Accept", "Decline", "Stop", "Pair a New Device", "Copy Address", "Disconnect", "Remove"]
    );
}

#[test]
fn a_device_is_viewing_or_not_connected() {
    let mut device = DeviceInfo {
        fingerprint: "ab".into(),
        name: "A".into(),
        connected: true,
        permissions: Permissions::default(),
    };
    assert_eq!(device_status(&device), "Viewing now");
    device.connected = false;
    assert_eq!(device_status(&device), "Not connected");
}

#[test]
fn every_permission_has_a_label_and_control_is_input() {
    use crate::share::PERMISSIONS;
    let labels: Vec<_> = PERMISSIONS.iter().map(|what| permission_label(what).unwrap()).collect();
    assert_eq!(labels, ["Control", "Clipboard", "Files", "Sound", "Microphone", "Camera"]);
    assert_eq!(permission_label("root"), None);
}

#[test]
fn the_fingerprint_is_cut_to_sixteen_in_fours() {
    let whole = "0123456789abcdef0123456789abcdef";
    assert_eq!(short_fingerprint(whole), "0123 4567 89ab cdef\u{2026}");
    assert_eq!(short_fingerprint("0123456789abcdef"), "0123 4567 89ab cdef");
    assert_eq!(short_fingerprint("012345"), "0123 45");
    assert_eq!(short_fingerprint(""), "");
}

#[test]
fn the_time_left_counts_whole_seconds_up() {
    assert_eq!(seconds_until(10_000, 0), Some(10));
    assert_eq!(seconds_until(10_001, 0), Some(11));
    assert_eq!(seconds_until(1, 0), Some(1));
    assert_eq!(seconds_until(5_000, 5_000), None);
    assert_eq!(seconds_until(5_000, 6_000), None);
    let open = ShareInfo { pairing: true, pairing_until_ms: Some(90_500), ..ShareInfo::off("w") };
    assert_eq!(pairing_seconds_left(&open, 60_000), Some(31));
    assert_eq!(pairing_seconds_left(&ShareInfo { pairing: false, ..open.clone() }, 60_000), None);
    assert_eq!(pairing_seconds_left(&ShareInfo { pairing_until_ms: None, ..open }, 60_000), None);
}

#[test]
fn what_the_host_said_is_kept_or_replaced_when_empty() {
    assert_eq!(host_said("no such device\r\n"), "no such device");
    assert_eq!(host_said(" \n"), "The host said nothing.");
}

#[test]
fn two_devices_with_one_name_are_told_apart_by_the_start_of_their_fingerprints() {
    use crate::share::{DeviceInfo, Permissions};
    let device = |fingerprint: &str, connected| DeviceInfo {
        fingerprint: fingerprint.into(),
        name: "Studio laptop".into(),
        connected,
        permissions: Permissions::default(),
    };
    assert_eq!(device_tag("9F2C41AB07D3E85C"), "9f2c 41ab");
    assert_eq!(device_line(&device(&"9f2c41ab".repeat(8), true)), "Viewing now \u{b7} 9f2c 41ab");
    assert_eq!(device_line(&device(&"5e7d0a42".repeat(8), false)), "Not connected \u{b7} 5e7d 0a42");
}
