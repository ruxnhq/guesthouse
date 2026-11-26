use super::*;

/// A listing entry as every host writes it: an event line with its version and name, a device
/// that is connected, one that waits, and a reason this version does not know.
const LISTED: &str = r#"{"v":1,"event":"share","machine":"win","state":"listening","port":7793,
"fingerprint":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","pairing":false,
"pairing_until_ms":null,"viewer":"Pat's iPad","paired":"Pat's iPad","closed_reason":"paired",
"pending":{"name":"Kitchen tablet","fingerprint":"abababababababababababababababababababababababababababababababab","code":"123 456","seconds":42},
"devices":[{"fingerprint":"cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd","name":"Pat's iPad","connected":true,
"permissions":{"input":true,"clipboard":false,"files":false,"sound":true,"mic":true,"camera":true}}],"reason":null}"#;

#[test]
fn a_listing_entry_reads_with_the_names_every_host_uses() {
    let info: ShareInfo = serde_json::from_str(LISTED).unwrap();
    assert_eq!(info.machine, "win");
    assert_eq!(info.state, ShareState::Listening);
    assert_eq!(info.port, Some(7793));
    assert_eq!(info.viewer.as_deref(), Some("Pat's iPad"));
    assert_eq!(info.closed_reason, Some(CloseReason::Paired));
    let pending = info.pending.as_ref().unwrap();
    assert_eq!(
        (pending.name.as_str(), pending.code.as_str(), pending.seconds),
        ("Kitchen tablet", "123 456", 42)
    );
    let device = &info.devices[0];
    assert!(
        device.connected
            && device.permissions.input
            && !device.permissions.clipboard
            && !device.permissions.files
    );
}

#[test]
fn writing_it_back_keeps_every_field_name() {
    let info: ShareInfo = serde_json::from_str(LISTED).unwrap();
    let value = serde_json::to_value(&info).unwrap();
    let object = value.as_object().unwrap();
    for name in [
        "machine",
        "state",
        "port",
        "fingerprint",
        "pairing",
        "pairing_until_ms",
        "viewer",
        "paired",
        "closed_reason",
        "pending",
        "devices",
        "reason",
    ] {
        assert!(object.contains_key(name), "{name}");
    }
    assert_eq!(object.len(), 12);
    assert_eq!(value["state"], "listening");
    assert_eq!(value["closed_reason"], "paired");
    assert_eq!(value["devices"][0]["permissions"]["mic"], true);
    assert_eq!(serde_json::from_value::<ShareInfo>(value).unwrap(), info);
}

#[test]
fn an_entry_without_the_newer_fields_still_reads() {
    let older = r#"{"machine":"w","state":"off","port":null,"fingerprint":null,"pairing":false,
        "pairing_until_ms":null,"viewer":null,"paired":null,"reason":null}"#;
    let info: ShareInfo = serde_json::from_str(older).unwrap();
    assert_eq!(info, ShareInfo::off("w"));
}

#[test]
fn the_states_and_the_close_reasons_are_the_shared_words() {
    for (word, state) in [
        ("off", ShareState::Off),
        ("starting", ShareState::Starting),
        ("listening", ShareState::Listening),
        ("failed", ShareState::Failed),
    ] {
        assert_eq!(serde_json::to_string(&state).unwrap(), format!("\"{word}\""));
        assert_eq!(serde_json::from_str::<ShareState>(&format!("\"{word}\"")).unwrap(), state);
    }
    for (word, reason) in [
        ("paired", CloseReason::Paired),
        ("declined", CloseReason::Declined),
        ("timeout", CloseReason::Timeout),
        ("asked", CloseReason::Asked),
        ("other", CloseReason::Other),
    ] {
        assert_eq!(serde_json::to_string(&reason).unwrap(), format!("\"{word}\""));
        assert_eq!(serde_json::from_str::<CloseReason>(&format!("\"{word}\"")).unwrap(), reason);
    }
    assert_eq!(serde_json::from_str::<CloseReason>("\"something new\"").unwrap(), CloseReason::Other);
    assert!(serde_json::from_str::<ShareState>("\"asleep\"").is_err());
}

#[test]
fn what_a_device_may_do_is_named_in_one_list() {
    let none =
        Permissions { input: false, clipboard: false, files: false, sound: false, mic: false, camera: false };
    for what in PERMISSIONS {
        assert_eq!(none.get(what), Some(false), "{what}");
        assert_eq!(Permissions::default().get(what), Some(true), "{what}");
    }
    assert_eq!(Permissions::default().get("root"), None);
    // A host that says nothing about a permission allows it.
    let partial: Permissions = serde_json::from_str(r#"{"files":false}"#).unwrap();
    assert!(partial.input && !partial.files);
}

#[test]
fn the_times_pairing_may_stay_open_are_checked() {
    assert_eq!(pairing_seconds(None), Ok(120));
    assert_eq!(pairing_seconds(Some(1)), Ok(1));
    assert_eq!(pairing_seconds(Some(3600)), Ok(3600));
    for bad in [0, 3601, u64::MAX] {
        assert_eq!(pairing_seconds(Some(bad)).unwrap_err(), "Pairing can stay open for 1 to 3600 seconds.");
    }
}

#[test]
fn the_ports_for_a_machine_are_the_agreed_range() {
    assert_eq!((FIRST_PORT, LAST_PORT), (7792, 7899));
}
