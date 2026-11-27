//! The sentences as data: their keys, their placeholders, and that each fills in to what the
//! functions say.

use super::data::*;
use super::*;
use crate::share::Permissions;

fn text(key: &str) -> String {
    let key = format!("{KEY_PREFIX}{key}");
    sentences().into_iter().find(|sentence| sentence.key == key).unwrap_or_else(|| panic!("{key}")).en
}

/// Every key, as released. A key is never renamed or taken away; a new one is added here.
#[test]
fn the_keys_are_these_and_never_change() {
    let keys: Vec<String> = sentences().into_iter().map(|sentence| sentence.key).collect();
    let want = [
        "allow_title",
        "pairing_title",
        "connect_title",
        "devices_title",
        "network_warning",
        "pairing_note",
        "paired_hint",
        "compare_code_line",
        "no_devices",
        "remove_message",
        "accept",
        "decline",
        "stop",
        "pair_new_device",
        "copy_address",
        "disconnect",
        "remove",
        "headline.off",
        "headline.not_running",
        "headline.starting",
        "headline.ready",
        "headline.viewing",
        "headline.failed",
        "headline.no_reason",
        "pairing_steps.1",
        "pairing_steps.2",
        "pairing_steps.3",
        "closed_text.paired_name",
        "closed_text.paired",
        "closed_text.declined",
        "closed_text.timeout",
        "closed_text.closed",
        "wants_to_pair",
        "remove_question",
        "only_notice",
        "device_status.viewing_now",
        "device_status.not_connected",
        "device_line",
        "permission_label.input",
        "permission_label.clipboard",
        "permission_label.files",
        "permission_label.sound",
        "permission_label.mic",
        "permission_label.camera",
        "host_said.nothing",
        "remote_view_title",
        "address_label",
        "port_label",
        "fingerprint_label",
        "pairing_open_title",
        "cancel",
        "held_while_pairing",
        "bridged_untried",
        "closing",
        "closes_in",
        "left_to_answer",
        "switch_name",
        "takes_screen",
        "screen_line",
    ]
    .map(|key| format!("{KEY_PREFIX}{key}"));
    assert_eq!(keys, want);
}

#[test]
fn keys_are_lower_case_words_and_unique() {
    let all = sentences();
    for sentence in &all {
        assert!(
            sentence
                .key
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.'),
            "{}",
            sentence.key
        );
        assert_eq!(all.iter().filter(|other| other.key == sentence.key).count(), 1, "{}", sentence.key);
        assert!(!sentence.en.is_empty(), "{}", sentence.key);
    }
}

#[test]
fn each_placeholder_is_a_param_and_no_sentence_has_a_brace_of_its_own() {
    for sentence in sentences() {
        let names = placeholders(&sentence.en);
        assert_eq!(names, sentence.params, "{}", sentence.key);
        for name in &names {
            assert!(!name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'), "{name}");
        }
        // With every placeholder taken out, no brace is left.
        let bare = fill(&sentence.en, &names.iter().map(|name| (name.as_str(), "")).collect::<Vec<_>>());
        assert!(!bare.contains('{') && !bare.contains('}'), "{}", sentence.key);
    }
}

/// Filled in with real values, each sentence that takes one is word for word what its function says.
#[test]
fn filled_in_each_is_what_its_function_says() {
    let viewing =
        ShareInfo { viewer: Some("Pat's iPad".into()), state: ShareState::Listening, ..Default::default() };
    let failed =
        ShareInfo { reason: Some("no port".into()), state: ShareState::Failed, ..Default::default() };
    let no_reason = ShareInfo { state: ShareState::Failed, ..Default::default() };
    let fingerprint = "9F2C41AB00000000000000000000000000000000000000000000000000000000";
    let device = DeviceInfo {
        fingerprint: fingerprint.into(),
        name: "Den".into(),
        connected: true,
        permissions: Permissions::default(),
    };
    let table = [
        (fill(&text("headline.viewing"), &[("viewer", "Pat's iPad")]), headline(true, true, &viewing)),
        (fill(&text("headline.failed"), &[("why", "no port")]), headline(true, true, &failed)),
        (
            fill(&text("headline.failed"), &[("why", &text("headline.no_reason"))]),
            headline(true, true, &no_reason),
        ),
        (
            fill(&text("closed_text.paired_name"), &[("name", "Kitchen tablet")]),
            closed_text(Some(CloseReason::Paired), Some("Kitchen tablet")),
        ),
        (fill(&text("wants_to_pair"), &[("name", "Pat's iPad")]), wants_to_pair("Pat's iPad")),
        (fill(&text("remove_question"), &[("name", "Den")]), remove_question("Den")),
        (fill(&text("only_notice"), &[("name", "Den")]), only_notice("Den")),
        (
            fill(
                &text("device_line"),
                &[("status", &text("device_status.viewing_now")), ("tag", "9f2c 41ab")],
            ),
            device_line(&device),
        ),
        (fill(&text("closes_in"), &[("time", &countdown(105))]), closes_in(105)),
        (fill(&text("closes_in"), &[("time", &countdown(3600))]), closes_in(3600)),
        (fill(&text("left_to_answer"), &[("seconds", "42")]), left_to_answer(42)),
        (
            fill(&text("switch_name"), &[("label", "Control"), ("device", "Phone")]),
            switch_name("Control", "Phone"),
        ),
        (fill(&text("takes_screen"), &[("name", "Phone")]), takes_screen("Phone")),
        (fill(&text("screen_line"), &[("name", "Phone")]), screen_line("Phone")),
    ];
    for (filled, said) in table {
        assert_eq!(filled, said);
    }
    assert_eq!(text("host_said.nothing"), host_said("  "));
    assert_eq!(text("device_status.not_connected"), "Not connected");
    assert_eq!(text("permission_label.input"), "Control");
}

#[test]
fn the_picture_comes_back_sentence_and_the_one_device_sentences_are_kept_apart_by_host() {
    let all = sentences();
    let find = |key: &str| all.iter().find(|sentence| sentence.key == format!("{KEY_PREFIX}{key}")).unwrap();
    assert_eq!(find("only_notice").hosts, [LINUX]);
    assert!(find("only_notice").en.contains("comes back when {name} leaves"));
    assert_eq!(find("takes_screen").hosts, [WINDOWS]);
    assert_eq!(find("screen_line").hosts, [WINDOWS]);
    assert!(all.iter().all(|sentence| sentence.hosts.is_empty() == sentence.note.is_none()));
    // The rest of the shared sentences are every host's.
    assert!(find("wants_to_pair").hosts.is_empty() && find("headline.ready").hosts.is_empty());
}

#[test]
fn the_json_has_its_version_and_every_key() {
    let file: serde_json::Value = serde_json::from_str(&json()).unwrap();
    assert_eq!(file["version"], VERSION);
    let keys = file["keys"].as_object().unwrap();
    assert_eq!(keys.len(), sentences().len());
    let waits = &keys["remote_view.wants_to_pair"];
    assert_eq!(waits["en"], "{name} wants to pair");
    assert_eq!(waits["params"], serde_json::json!(["name"]));
    assert!(waits.get("hosts").is_none() && waits.get("note").is_none());
    assert_eq!(keys["remote_view.accept"]["params"], serde_json::json!([]));
    assert_eq!(keys["remote_view.only_notice"]["hosts"], serde_json::json!(["linux"]));
    assert!(json().ends_with("}\n"));
    assert!(!json().contains('\r'));
}

#[test]
fn a_countdown_is_minutes_and_two_digits_of_seconds() {
    assert_eq!(countdown(0), "0:00");
    assert_eq!(countdown(9), "0:09");
    assert_eq!(countdown(105), "1:45");
    assert_eq!(countdown(3600), "60:00");
}

#[test]
fn filling_in_is_one_pass_and_a_value_with_braces_is_kept_as_it_is() {
    assert_eq!(
        fill("{label} for {device}", &[("label", "{device}"), ("device", "Phone")]),
        "{device} for Phone"
    );
    assert_eq!(fill("{name} wants to pair", &[]), "{name} wants to pair");
    assert_eq!(fill("no braces", &[("name", "x")]), "no braces");
    assert_eq!(fill("open { brace", &[("name", "x")]), "open { brace");
}
