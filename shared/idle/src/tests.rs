//! The answers the Linux host's pins read, read here too, so they run wherever the crate builds. They
//! are written from the guest script's own layout; none is a capture from a guest.

use super::*;

const QUIET: &str = include_str!("../../../linux/app-core/tests/idle_rules/quiet.txt");
const BUSY: &str = include_str!("../../../linux/app-core/tests/idle_rules/busy.txt");
const ANOTHER_LANGUAGE: &str = include_str!("../../../linux/app-core/tests/idle_rules/another_language.txt");
const FAILED: &str = include_str!("../../../linux/app-core/tests/idle_rules/failed.txt");
const SCRIPT: &str = include_str!("../../../guest/windows/idle.ps1");

#[test]
fn each_answer_is_read_as_on_the_linux_host() {
    let quiet = parse_report(QUIET);
    assert_eq!(
        (quiet.remote_sessions, quiet.power_requests, quiet.input),
        (Ok(0), Ok(Vec::new()), Ok(Input::Seconds(7200)))
    );
    let busy = parse_report(BUSY);
    assert_eq!(busy.remote_sessions, Ok(2));
    let kinds: Vec<PowerKind> = busy.power_requests.unwrap().iter().map(|request| request.kind).collect();
    assert_eq!(kinds, [PowerKind::Display, PowerKind::System, PowerKind::System, PowerKind::Execution]);
    assert_eq!(busy.input, Ok(Input::Seconds(12)));
    let other = parse_report(ANOTHER_LANGUAGE);
    assert_eq!(
        (other.remote_sessions, other.power_requests, other.input),
        (Ok(0), Ok(Vec::new()), Ok(Input::NobodyLoggedIn))
    );
    let failed = parse_report(FAILED);
    assert_eq!(failed.remote_sessions, Err(Unread::new("the guest could not read its sessions")));
    assert_eq!(failed.power_requests, Err(Unread::new("the guest could not read its power requests")));
    assert_eq!(failed.input, Err(Unread::new("nothing in the user's session said when the last input was")));
    for answer in [QUIET, BUSY, ANOTHER_LANGUAGE, FAILED] {
        assert_eq!(parse_report(&answer.replace('\n', "\r\n")), parse_report(answer));
    }
}

#[test]
fn the_script_says_its_parts_in_the_words_read_here() {
    for line in ["'sessions ok'", "'power ok'", "'input nobody'", "\"input $idleSeconds\"", "'input failed'"]
    {
        assert!(SCRIPT.contains(line), "{line}");
    }
}
