//! The script the Linux host has the guest's agent run for `pair accept FP` and `pair decline FP`
//! (`install::ctl_script`): it answers only the device with that fingerprint. remoteman's host is a
//! function here that says a status with a waiting device and echoes whatever else it is asked.

use crate::{run, shell};
use guesthouse_remote_view_core::install::{GUEST_HOST, ctl_script};

const SHOWN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const OTHER: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";
const HOST: &str = "Fake-Host";

/// What the script printed for `words`, with remoteman's host saying `status` as `status`.
fn answered(shell: &str, words: &[&str], status: &str) -> Vec<String> {
    let words: Vec<String> = words.iter().map(|word| (*word).to_string()).collect();
    let quoted_host = format!("'{GUEST_HOST}'");
    let script = ctl_script(&words);
    assert!(script.contains(&quoted_host), "{script}");
    let script = script.replace(&quoted_host, HOST);
    let fake = format!(
        "function {HOST} {{ $rest = @($args | Select-Object -Skip 3); if ($rest -ceq 'status') {{ {status} }} else {{ 'ran ' + ($rest -join ' ') }} }}\n"
    );
    let said = run(shell, &format!("$ErrorActionPreference = 'Stop'\n{fake}{script}"));
    assert_eq!(said.code, Some(0), "{said}");
    said.lines().into_iter().map(str::to_owned).collect()
}

fn waiting(fingerprint: &str) -> String {
    format!(
        "'warning: a line before it'; '{{\"pairing\":false,\"pending\":{{\"name\":\"Tab\",\"fingerprint\":\"{fingerprint}\",\"code\":\"111 222\",\"seconds\":40}}}}'"
    )
}

#[test]
fn an_answer_goes_only_to_the_device_named() {
    let Some(shell) = shell() else { return };
    for answer in ["accept", "decline"] {
        let named = answered(shell, &["pair", answer, SHOWN], &waiting(&SHOWN.to_ascii_uppercase()));
        assert_eq!(named, [format!("ran pair {answer}")]);
        for status in
            [waiting(OTHER), "'{\"pairing\":true,\"pending\":null}'".into(), "'not json'".into(), "".into()]
        {
            let said = answered(shell, &["pair", answer, SHOWN], &status);
            assert_eq!(said, ["another device is waiting now"], "{answer} with {status}");
        }
    }
}

#[test]
fn an_answer_with_no_device_named_runs_nothing() {
    let Some(shell) = shell() else { return };
    for words in [&["pair", "accept"][..], &["pair", "decline"], &["pair", "accept", "Tab"]] {
        let said = answered_without_host(shell, words);
        assert!(!said.iter().any(|line| line.starts_with("ran")), "{words:?}: {said:?}");
    }
}

/// The script for `words`, which must not call remoteman's host at all.
fn answered_without_host(shell: &str, words: &[&str]) -> Vec<String> {
    let words: Vec<String> = words.iter().map(|word| (*word).to_string()).collect();
    let script = ctl_script(&words);
    assert!(!script.contains(GUEST_HOST), "{script}");
    let said = run(shell, &format!("$ErrorActionPreference = 'Stop'\n{script}"));
    assert_eq!(said.code, Some(0), "{said}");
    said.lines().into_iter().map(str::to_owned).collect()
}
