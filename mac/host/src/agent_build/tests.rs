// SPDX-License-Identifier: GPL-2.0-or-later
use super::*;
use std::io::Write;

fn mine() -> Identity {
    Identity {
        version: "0.1.20261129".into(),
        build: "20261129".into(),
        path: "/Applications/Guesthouse.app/Contents/MacOS/guesthouse-agent".into(),
        stamp: "900.1700000000".into(),
    }
}

fn listing(agent: Option<&Identity>, machines: &[(&str, &str)], v: u32) -> Value {
    let mut value = json!({
        "v": v, "id": 1, "ok": true,
        "machines": machines.iter().map(|(m, s)| json!({"machine": m, "state": s})).collect::<Vec<_>>(),
    });
    if let Some(agent) = agent {
        value["agent"] = serde_json::to_value(agent).unwrap();
    }
    value
}

fn verdict(answer: &Value) -> Value {
    judge(&mine(), answer)
}

#[test]
fn an_agent_of_the_same_build_is_the_apps_own() {
    let answer = listing(Some(&mine()), &[("win", "running")], VERSION);
    assert_eq!(verdict(&answer)["verdict"], "own");
}

#[test]
fn another_path_another_build_or_a_replaced_file_is_not() {
    let other_folder = Identity {
        path: "/Users/x/Downloads/Guesthouse.app/Contents/MacOS/guesthouse-agent".into(),
        ..mine()
    };
    let yesterday = Identity { build: "20261128".into(), version: "0.1.20261128".into(), ..mine() };
    let rebuilt = Identity { stamp: "900.1700000099".into(), ..mine() };
    for other in [other_folder, yesterday, rebuilt] {
        let answer = listing(Some(&other), &[], VERSION);
        assert_eq!(verdict(&answer)["verdict"], "replace", "{other:?}");
    }
}

#[test]
fn an_agent_with_no_identity_is_older_than_any() {
    let said = verdict(&listing(None, &[("win", "off")], VERSION));
    assert_eq!(said["verdict"], "replace");
    assert_eq!(said["older"], "an older build");
}

#[test]
fn an_older_agent_with_nothing_running_is_replaced_and_one_with_machines_is_not() {
    let old = Identity { build: "1".into(), ..mine() };
    let idle = listing(Some(&old), &[("a", "off"), ("b", "failed")], VERSION);
    assert_eq!(verdict(&idle)["verdict"], "replace");
    assert_eq!(verdict(&idle)["older"], "0.1.20261129 (1)");
    for state in ["starting", "running", "stopping", "stopping_stalled"] {
        let busy = verdict(&listing(Some(&old), &[("a", "off"), ("Windows 11", state)], VERSION));
        assert_eq!(busy["verdict"], "busy", "{state}");
        assert_eq!(busy["running"], json!(["Windows 11"]));
        assert_eq!(busy["can_continue"], true);
    }
}

#[test]
fn an_install_in_progress_counts_as_running_and_is_named_once() {
    let mut answer = listing(None, &[("Ubuntu", "running")], VERSION);
    answer["installs"] = json!([
        {"machine": "Ubuntu", "state": "running"}, {"machine": "Win", "state": "running"},
        {"machine": "Old", "state": "done"}
    ]);
    let said = verdict(&answer);
    assert_eq!(said["running"], json!(["Ubuntu", "Win"]));
    assert_eq!(said["installing"], json!(["Ubuntu", "Win"]));
}

#[test]
fn a_protocol_this_app_does_not_speak_cannot_be_carried_on_with() {
    let busy = verdict(&listing(None, &[("w", "running")], VERSION + 1));
    assert_eq!(busy["verdict"], "busy");
    assert_eq!(busy["can_continue"], false);
}

#[test]
fn the_identity_follows_the_file_its_path_its_size_and_its_time() {
    let dir = std::env::temp_dir().join(format!("gh-build-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let program = dir.join("agent");
    fs::write(&program, b"one").unwrap();
    let first = identity_of(&program, "0.1", "7").unwrap();
    assert_eq!(first, identity_of(&program, "0.1", "7").unwrap());
    assert_ne!(first, identity_of(&program, "0.1", "8").unwrap());
    // A link to it is the same file.
    let link = dir.join("link");
    std::os::unix::fs::symlink(&program, &link).unwrap();
    assert_eq!(first, identity_of(&link, "0.1", "7").unwrap());
    // Replaced in place with another size: another identity, with the same version and build.
    fs::File::create(&program).unwrap().write_all(b"another build").unwrap();
    assert_ne!(first, identity_of(&program, "0.1", "7").unwrap());
    assert!(identity_of(&dir.join("none"), "0.1", "7").is_none());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn the_requests_the_app_makes_are_answered() {
    let ask = |value: Value| serde_json::from_str::<Value>(&answer(&value.to_string())).unwrap();
    let judged =
        ask(json!({"what": "judge", "expected": mine(), "answer": listing(Some(&mine()), &[], VERSION)}));
    assert_eq!(judged["verdict"], "own");
    assert!(
        ask(json!({"what": "identity", "program": "/no/such", "version": "1", "build": "1"}))["identity"]
            .is_null()
    );
    assert!(ask(json!({"what": "judge"}))["error"].is_string());
    assert!(ask(json!({"what": "else"}))["error"].is_string());
}
