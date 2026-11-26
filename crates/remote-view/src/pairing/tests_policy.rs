//! The host-wide clipboard and files held while a device is let in, and the viewer kept out of
//! sight.

use super::tests::*;
use super::*;

fn with_switches(open: bool, waiting: Option<Pending>, devices: &[(&str, &str)], on: (bool, bool)) -> Status {
    Status { clipboard: Some(on.0), files: Some(on.1), ..reading(open, waiting, devices) }
}

fn linux_run(on: (bool, bool)) -> Run {
    let mut run = Run::new(Policy::linux());
    run.read(&with_switches(false, None, &[], on));
    run
}

const LIMITS: [&str; 2] = [
    "device aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa allow clipboard off",
    "device aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa allow files off",
];

#[test]
fn a_linux_host_turns_the_switches_off_before_pairing_opens() {
    let mut run = linux_run((true, true));
    assert_eq!(run.open(120), ["allow clipboard off", "allow files off", "pair on"]);
    assert_eq!(run.pairing.held(), Some(HostWide { clipboard: true, files: true }));
}

#[test]
fn only_what_was_on_is_held_and_given_back() {
    for (on, held, off_lines, back_lines) in [
        (
            (true, false),
            HostWide { clipboard: true, files: false },
            vec!["allow clipboard off"],
            vec!["allow clipboard on"],
        ),
        (
            (false, true),
            HostWide { clipboard: false, files: true },
            vec!["allow files off"],
            vec!["allow files on"],
        ),
    ] {
        let mut run = linux_run(on);
        let mut want: Vec<&str> = off_lines;
        want.push("pair on");
        assert_eq!(run.open(120), want);
        assert_eq!(run.pairing.held(), Some(held));
        assert_eq!(run.event(Event::Stop), [vec!["pair off"], back_lines].concat());
        assert_eq!(run.pairing.held(), None);
    }
}

#[test]
fn switches_that_are_off_or_not_said_are_left_alone() {
    let mut run = linux_run((false, false));
    assert_eq!(run.open(120), ["pair on"]);
    assert_eq!(run.pairing.held(), None);
    assert_eq!(run.event(Event::Stop), ["pair off"]);

    let mut silent = Run::new(Policy::linux());
    silent.read(&reading(false, None, &[]));
    assert_eq!(silent.open(120), ["pair on"]);
    assert_eq!(silent.pairing.held(), None);
}

#[test]
fn the_device_is_limited_before_the_switches_come_back() {
    let mut run = linux_run((true, true));
    run.open(120);
    run.read(&with_switches(true, None, &[], (false, false)));
    run.read(&with_switches(false, Some(pending(KITCHEN, "Tab", 50)), &[], (false, false)));
    // Still held: a waiting device changes nothing about that.
    assert_eq!(run.pairing.held(), Some(HostWide { clipboard: true, files: true }));
    assert_eq!(run.event(Event::Accept), ["pair accept"]);
    let lines = run.read(&with_switches(false, None, &[(KITCHEN, "Tab")], (false, false)));
    assert_eq!(lines, [LIMITS[0], LIMITS[1], "allow clipboard on", "allow files on"]);
    assert_eq!(run.pairing.held(), None);
}

#[test]
fn a_declined_timed_out_or_stopped_pairing_gives_the_switches_back() {
    let waiting = |run: &mut Run| {
        run.open(120);
        run.read(&with_switches(true, None, &[], (false, false)));
        run.read(&with_switches(false, Some(pending(KITCHEN, "Tab", 50)), &[], (false, false)));
    };
    let mut run = linux_run((true, true));
    waiting(&mut run);
    assert_eq!(run.event(Event::Decline), ["pair decline", "allow clipboard on", "allow files on"]);

    let mut run = linux_run((true, true));
    waiting(&mut run);
    assert_eq!(run.event(Event::Stop), ["pair decline", "allow clipboard on", "allow files on"]);

    let mut run = linux_run((true, true));
    waiting(&mut run);
    run.pass(61_000);
    let gone = run.read(&with_switches(false, None, &[], (false, false)));
    assert_eq!(gone, ["allow clipboard on", "allow files on"]);

    let mut run = linux_run((true, true));
    run.open(5);
    run.read(&with_switches(true, None, &[], (false, false)));
    run.pass(5_000);
    let lines = run.read(&with_switches(true, None, &[], (false, false)));
    assert_eq!(lines, ["pair off", "allow clipboard on", "allow files on"]);

    for event in [Event::SwitchOff, Event::Shutdown] {
        let mut run = linux_run((true, true));
        run.open(120);
        assert_eq!(run.event(event), ["pair off", "allow clipboard on", "allow files on"]);
    }
}

#[test]
fn opening_again_at_once_holds_again_before_a_new_reading() {
    let mut run = linux_run((true, true));
    run.open(120);
    run.event(Event::Stop);
    assert_eq!(run.open(120), ["allow clipboard off", "allow files off", "pair on"]);
}

#[test]
fn a_device_found_waiting_is_met_with_the_switches_off() {
    let mut run = linux_run((true, true));
    let lines = run.read(&with_switches(false, Some(pending(KITCHEN, "Tab", 50)), &[], (true, true)));
    assert_eq!(lines, ["allow clipboard off", "allow files off"]);
    assert_eq!(run.pairing.held(), Some(HostWide { clipboard: true, files: true }));
}

#[test]
fn a_device_let_in_by_someone_else_is_limited_too() {
    let mut run = linux_run((true, true));
    run.open(120);
    run.read(&with_switches(true, None, &[], (false, false)));
    let lines = run.read(&with_switches(false, None, &[(KITCHEN, "Tab")], (false, false)));
    assert_eq!(lines, [LIMITS[0], LIMITS[1], "allow clipboard on", "allow files on"]);
}

/// Everything a Windows host must never be sent: the host-wide switches, and what cuts Guesthouse's
/// own viewer.
fn forbidden(line: &str) -> bool {
    line.starts_with("sharing") || line == "drop" || line.starts_with("allow ") || line.contains(VIEWER)
}

#[test]
fn a_windows_host_is_never_sent_what_would_cut_its_own_viewer() {
    let policy = Policy::windows(VIEWER);
    let on = |status: Status| Status { clipboard: Some(true), files: Some(true), ..status };
    let ours = (VIEWER, "Guesthouse");
    let mut sent = Vec::new();

    // Accepted, declined, stopped, run out, switched off, quit, opened twice, found open.
    let mut run = Run::new(policy.clone());
    run.read(&on(reading(false, None, &[ours])));
    run.open(120);
    run.read(&on(reading(true, None, &[ours])));
    run.read(&on(reading(false, Some(pending(KITCHEN, "Tab", 50)), &[ours])));
    run.event(Event::Accept);
    run.read(&on(reading(false, None, &[ours, (KITCHEN, "Tab")])));
    sent.extend(run.sent.clone());

    for answer in [Event::Decline, Event::Stop] {
        let mut run = Run::idle(policy.clone(), &[ours]);
        run.open(120);
        run.open(300);
        run.read(&on(reading(true, None, &[ours])));
        run.read(&on(reading(false, Some(pending(PHONE, "Phone", 50)), &[ours])));
        run.event(answer);
        sent.extend(run.sent.clone());
    }
    for event in [Event::SwitchOff, Event::Shutdown] {
        let mut run = Run::idle(policy.clone(), &[ours]);
        run.open(120);
        run.event(event);
        sent.extend(run.sent.clone());
    }
    let mut run = Run::idle(policy.clone(), &[ours]);
    run.open(5);
    run.read(&on(reading(true, None, &[ours])));
    run.pass(5_000);
    run.read(&on(reading(true, None, &[ours])));
    sent.extend(run.sent.clone());

    let mut run = Run::new(policy.clone());
    run.event(Event::Started { held: None });
    run.read(&on(reading(true, None, &[ours])));
    sent.extend(run.sent.clone());

    assert!(sent.len() > 10, "{sent:?}");
    for line in &sent {
        assert!(!forbidden(line), "{line}");
    }
    // The new device is still limited.
    assert!(sent.contains(&format!("device {KITCHEN} allow clipboard off")));
    assert_eq!(run.pairing.held(), None);
}

#[test]
fn the_hidden_viewer_appearing_is_not_a_device_paired() {
    let mut run = Run::idle(Policy::windows(VIEWER), &[]);
    run.open(120);
    run.read(&reading(true, None, &[]));
    // The installer ran again meanwhile and put the viewer on the list.
    assert!(run.read(&reading(true, None, &[(&VIEWER.to_uppercase(), "Guesthouse")])).is_empty());
    assert!(run.pairing.view().open);
    assert_eq!(run.pairing.view().closed_reason, None);
}

#[test]
fn the_same_reading_on_a_linux_host_would_count_as_a_device() {
    let mut run = Run::idle(plain(), &[]);
    run.open(120);
    run.read(&reading(true, None, &[]));
    assert!(!run.read(&reading(true, None, &[(VIEWER, "Guesthouse")])).is_empty());
}
