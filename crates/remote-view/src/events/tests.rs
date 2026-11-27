use super::*;

const KITCHEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn waits() -> PairingEvent {
    PairingEvent::DeviceWaits {
        name: "Kitchen tablet".into(),
        fingerprint: KITCHEN.into(),
        code: "123 456".into(),
        seconds: 55,
    }
}

/// Each event's JSON, word for word: another host reads these, so the names never change.
#[test]
fn each_event_has_the_json_it_always_has() {
    let table = [
        (
            PairingEvent::PairingOpen { until_unix_seconds: 1_700_000_121 },
            r#"{"pairing_event":"pairing_open","until_unix_seconds":1700000121}"#.to_string(),
        ),
        (
            waits(),
            format!(
                r#"{{"pairing_event":"device_waits","name":"Kitchen tablet","fingerprint":"{KITCHEN}","code":"123 456","seconds":55}}"#
            ),
        ),
        (
            PairingEvent::Paired { fingerprint: KITCHEN.into(), name: "Kitchen tablet".into() },
            format!(r#"{{"pairing_event":"paired","fingerprint":"{KITCHEN}","name":"Kitchen tablet"}}"#),
        ),
        (
            PairingEvent::PairingClosed { reason: CloseReason::Paired },
            r#"{"pairing_event":"pairing_closed","reason":"paired"}"#.to_string(),
        ),
    ];
    for (event, json) in table {
        assert_eq!(serde_json::to_string(&event).unwrap(), json);
        assert_eq!(serde_json::from_str::<PairingEvent>(&json).unwrap(), event);
    }
}

#[test]
fn every_close_reason_is_its_word_and_an_unknown_one_reads_as_other() {
    for (reason, word) in [
        (CloseReason::Paired, "paired"),
        (CloseReason::Declined, "declined"),
        (CloseReason::Timeout, "timeout"),
        (CloseReason::Asked, "asked"),
        (CloseReason::Other, "other"),
    ] {
        let json = format!(r#"{{"pairing_event":"pairing_closed","reason":"{word}"}}"#);
        assert_eq!(serde_json::to_string(&PairingEvent::PairingClosed { reason }).unwrap(), json);
    }
    let later = r#"{"pairing_event":"pairing_closed","reason":"cancelled"}"#;
    assert_eq!(
        serde_json::from_str::<PairingEvent>(later).unwrap(),
        PairingEvent::PairingClosed { reason: CloseReason::Other }
    );
}

#[test]
fn the_tag_is_the_field_the_json_uses() {
    let json = serde_json::to_value(waits()).unwrap();
    assert_eq!(json[TAG], "device_waits");
}

#[test]
fn debug_never_prints_a_name_or_a_code() {
    let events =
        [waits(), PairingEvent::Paired { fingerprint: KITCHEN.into(), name: "Kitchen tablet".into() }];
    for event in &events {
        for printed in [format!("{event:?}"), format!("{event:#?}")] {
            assert!(!printed.contains("Kitchen"), "{printed}");
            assert!(!printed.contains("123"), "{printed}");
            assert!(!printed.contains("456"), "{printed}");
            assert!(printed.contains(KITCHEN), "{printed}");
        }
    }
    assert_eq!(
        format!("{:?}", events[0]),
        format!("DeviceWaits {{ fingerprint: \"{KITCHEN}\", seconds: 55, .. }}")
    );
    assert_eq!(
        format!("{:?}", PairingEvent::PairingOpen { until_unix_seconds: 7 }),
        "PairingOpen { until_unix_seconds: 7 }"
    );
    assert_eq!(
        format!("{:?}", PairingEvent::PairingClosed { reason: CloseReason::Timeout }),
        "PairingClosed { reason: Timeout }"
    );
    // Nor in a list of them, as a log line might print one.
    assert!(!format!("{events:?}").contains("Kitchen"));
}

#[test]
fn seconds_are_rounded_up_and_never_below_zero() {
    assert_eq!(unix_seconds(0), 0);
    assert_eq!(unix_seconds(1), 1);
    assert_eq!(unix_seconds(1_000), 1);
    assert_eq!(unix_seconds(1_700_000_120_001), 1_700_000_121);
    assert_eq!(seconds_left(10_000, 10_000), 0);
    assert_eq!(seconds_left(10_000, 20_000), 0);
    assert_eq!(seconds_left(10_001, 10_000), 1);
    assert_eq!(seconds_left(65_000, 10_000), 55);
    assert_eq!(seconds_left(u64::MAX, 0), u32::MAX);
}
