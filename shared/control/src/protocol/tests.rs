//! The pipe's version both ways, with lines as an app and a service from before it wrote them.

use super::*;
use crate::client::{Unreached, answer_from};
use crate::{Answer, Event, State};
use serde_json::json;

#[test]
fn a_request_carries_the_version_beside_its_words() {
    assert_eq!(request_line(&Request::Status), json!({"ask": "status", "protocol": 2}));
    assert_eq!(
        request_line(&Request::Start { name: "win".into() }),
        json!({"ask": "start", "name": "win", "protocol": 2})
    );
    assert_eq!(request_from(request_line(&Request::Status)), Ok(Request::Status));
    let start = Request::Start { name: "win".into() };
    assert_eq!(request_from(request_line(&start)), Ok(start));
}

/// A service from before the version reads `Request` as it is here, unchanged, and passes over the
/// version, so it answers a new app as it always has, and the app finds its `Status` says no version.
#[test]
fn a_service_from_before_reads_a_new_apps_request_as_it_always_did() {
    let start = Request::Start { name: "win".into() };
    for request in [Request::Status, start] {
        assert_eq!(serde_json::from_value::<Request>(request_line(&request)).unwrap(), request);
    }
}

#[test]
fn an_app_from_before_is_told_to_update_in_words_it_shows() {
    for line in [
        json!({"ask": "status"}),
        json!({"ask": "start", "name": "win"}),
        json!({"ask": "status", "protocol": 3}),
    ] {
        assert_eq!(request_from(line.clone()), Err(OTHER_VERSION.to_owned()), "{line}");
    }
    // An app from before reads a refusal as this, and shows its reason.
    let refused = Answer::Refused { reason: OTHER_VERSION.into() };
    assert_eq!(
        serde_json::to_value(&refused).unwrap(),
        json!({"answer": "refused", "reason": OTHER_VERSION})
    );
    assert_eq!(request_from(json!({"ask": "format the host", "protocol": 2})), Err(NOT_A_REQUEST.to_owned()));
}

#[test]
fn a_new_app_tells_a_service_from_before_from_one_it_cant_read() {
    let machine = json!({"name": "win", "system": "windows", "state": "stopped", "set_up": true, "memory_mb": 8192,
        "processors": 4, "card": null, "address": null});
    let before = json!({"answer": "status", "version": "0.11.0", "machines": [machine], "cards": []});
    assert_eq!(answer_from(before), Err(Unreached::OtherVersion));
    assert_eq!(Unreached::OtherVersion.words(), OTHER_VERSION);
    let newer = json!({"answer": "status", "version": "9.0.0", "protocol": 3, "machines": [], "cards": []});
    assert_eq!(answer_from(newer), Err(Unreached::OtherVersion));
    let now = json!({"answer": "status", "version": "0.12.0", "protocol": 2, "machines": [], "cards": []});
    assert!(matches!(answer_from(now), Ok(Answer::Status { protocol: PROTOCOL, .. })));
    // Only Status says the version. The other answers read as they are.
    assert_eq!(answer_from(json!({"answer": "ok"})), Ok(Answer::Ok));
    let event = json!({"answer": "event", "event": {"kind": "state", "text": "off"}});
    assert_eq!(answer_from(event), Ok(Answer::Event { event: Event::State(State::Off) }));
    assert_eq!(answer_from(json!({"answer": "nonsense"})), Err(Unreached::Garbled));
}

/// Turning Hyper-V on is a request and a status field added within version 2, as an app and a
/// service of either age still read each other: the new request at an old service is a request it
/// doesn't know, which it refuses in words an app shows, and the new field at an old app is one it
/// passes over.
#[test]
fn turning_hyper_v_on_is_added_within_the_version_both_ways() {
    assert_eq!(request_line(&Request::EnableHyperV), json!({"ask": "enable_hyper_v", "protocol": 2}));
    assert_eq!(request_from(request_line(&Request::EnableHyperV)), Ok(Request::EnableHyperV));
    // A service from before reads requests as these, and has no `enable_hyper_v`.
    #[derive(serde::Deserialize, Debug)]
    #[serde(tag = "ask", rename_all = "snake_case")]
    #[allow(dead_code)]
    enum Older {
        Status,
        Start { name: String },
    }
    assert!(serde_json::from_value::<Older>(json!({"ask": "enable_hyper_v", "protocol": 2})).is_err());
    assert_eq!(
        request_from(json!({"ask": "enable_hyper_v_now", "protocol": 2})),
        Err(NOT_A_REQUEST.to_owned())
    );
    // An app from before reads a new service's bridging, Hyper-V and all, with what it knows.
    let new = json!({"answer": "status", "version": "0.12.0", "protocol": 2, "machines": [], "cards": [],
        "bridging": {"available": false, "reason": "Bridging needs Windows' Hyper-V feature.", "adapters": [],
            "hyper_v": {"state": "restart_pending"}}});
    #[derive(serde::Deserialize)]
    struct OlderBridging {
        available: bool,
        reason: Option<String>,
    }
    let older: OlderBridging = serde_json::from_value(new["bridging"].clone()).unwrap();
    assert!(!older.available && older.reason.is_some());
    match answer_from(new) {
        Ok(Answer::Status { bridging, .. }) => assert_eq!(bridging.hyper_v, crate::HyperV::RestartPending),
        other => panic!("{other:?}"),
    }
}
