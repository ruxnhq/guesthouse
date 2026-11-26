use super::*;

fn status(text: &str) -> Status {
    match parse_reply(text) {
        Reply::Status(status) => *status,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_host_that_says_little_reads_as_idle() {
    let idle = status(r#"{"pairing":false,"pending":null,"devices":[]}"#);
    assert_eq!(idle, Status::default());
    assert_eq!(status("{}\n"), Status::default());
}

#[test]
fn what_the_host_adds_later_is_ignored() {
    let said = r#"{"pairing":true,"brand_new":{"a":[1,2]},"second_device":"watch","devices":[{"fingerprint":"ab","name":"X","connected":true,"joined":false,"since":5,"permissions":{"input":true,"clipboard":true,"files":true,"sound":true,"mic":true,"camera":true,"pen":true}}]}"#;
    let parsed = status(said);
    assert!(parsed.pairing);
    assert_eq!(parsed.devices.len(), 1);
    assert!(parsed.devices[0].connected);
}

#[test]
fn a_device_from_an_older_host_has_every_permission() {
    let parsed = status(r#"{"devices":[{"fingerprint":"ab","name":"Old"}]}"#);
    let device = &parsed.devices[0];
    assert!(!device.connected);
    assert!(device.permissions.input && device.permissions.mic && device.permissions.files);
}

#[test]
fn the_host_wide_switches_are_told_apart_from_not_said() {
    assert_eq!(status(r#"{"clipboard":false,"files":true}"#).clipboard, Some(false));
    assert_eq!(status(r#"{"clipboard":false,"files":true}"#).files, Some(true));
    assert_eq!(status(r#"{}"#).clipboard, None);
}

#[test]
fn a_pending_device_keeps_its_code_and_time() {
    let waiting = status(r#"{"pending":{"name":"Tab","fingerprint":"cd","code":"111 222","seconds":31}}"#);
    let pending = waiting.pending.unwrap();
    assert_eq!((pending.code.as_str(), pending.seconds), ("111 222", 31));
}

#[test]
fn a_warning_before_the_line_does_not_matter() {
    let output = "WARNING: something the shell said\r\n{\"pairing\":true}\r\n";
    assert!(status(output).pairing);
}

#[test]
fn the_host_stopping_and_other_words_are_answers_too() {
    assert_eq!(parse_reply("the host is stopping\n"), Reply::Stopping);
    assert_eq!(parse_reply("  The host is stopping\r\n"), Reply::Stopping);
    assert_eq!(parse_reply(""), Reply::Error("The host said nothing.".into()));
    assert_eq!(parse_reply("this host keeps no log\n"), Reply::Error("this host keeps no log".into()));
}

#[test]
fn a_line_that_is_not_json_or_has_the_wrong_kind_of_value_is_an_error() {
    for text in ["{not json", r#"{"pairing":"yes"}"#, r#"{"devices":{}}"#, r#"{"pending":{"name":"x"}}"#] {
        match parse_reply(text) {
            Reply::Error(why) => assert!(why.starts_with("The host's status could not be read"), "{why}"),
            other => panic!("{text}: {other:?}"),
        }
    }
}

#[test]
fn a_device_is_found_by_its_fingerprint_in_either_case() {
    let parsed = status(r#"{"devices":[{"fingerprint":"abcd","name":"A"}]}"#);
    assert_eq!(parsed.device("ABCD").unwrap().name, "A");
    assert!(parsed.device("ef").is_none());
}
