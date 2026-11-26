//! A program that starts, or starts again, finds the host as it is: what a reading shows is all it
//! has to go on.

use super::tests::*;
use super::*;

fn started(policy: Policy, held: Option<HostWide>) -> Run {
    let mut run = Run::new(policy);
    run.event(Event::Started { held });
    run
}

#[test]
fn an_idle_host_is_closed_and_nothing_is_sent() {
    let mut run = started(plain(), None);
    assert_eq!(run.pairing.poll_ms(), None);
    assert!(run.read(&reading(false, None, &[(OLD, "Old phone")])).is_empty());
    assert_eq!(run.open(60), ["pair on"]);
}

#[test]
fn pairing_found_open_is_closed() {
    let mut run = started(plain(), None);
    assert_eq!(run.read(&reading(true, None, &[])), ["pair off"]);
    let view = run.pairing.view();
    assert!(!view.open);
    assert_eq!(view.closed_reason, None);
}

#[test]
fn a_device_found_waiting_is_taken_over_and_never_closed_under() {
    let mut run = started(plain(), None);
    assert!(run.read(&reading(false, Some(pending(KITCHEN, "Tab", 44)), &[(OLD, "Old phone")])).is_empty());
    let view = run.pairing.view();
    assert_eq!(view.pending.as_ref().map(|device| device.name.as_str()), Some("Tab"));
    // Even with pairing said to be open as well, nothing is closed while it waits.
    assert!(run.read(&reading(true, Some(pending(KITCHEN, "Tab", 43)), &[(OLD, "Old phone")])).is_empty());
    assert_eq!(run.event(Event::Accept), ["pair accept"]);
    let lines = run.read(&reading(false, None, &[(OLD, "Old phone"), (KITCHEN, "Tab")]));
    assert_eq!(lines.len(), 2);
    assert_eq!(run.pairing.view().paired.as_deref(), Some("Tab"));
}

#[test]
fn a_device_waiting_with_the_program_idle_is_taken_over_too() {
    let mut run = Run::idle(plain(), &[]);
    assert!(run.read(&reading(false, Some(pending(KITCHEN, "Tab", 44)), &[])).is_empty());
    assert!(run.pairing.busy());
    assert_eq!(run.event(Event::Decline), ["pair decline"]);
}

#[test]
fn pairing_left_open_while_idle_is_closed_once_and_reading_stays_quiet_after() {
    let mut run = Run::idle(plain(), &[]);
    assert_eq!(run.read(&reading(true, None, &[])), ["pair off"]);
    assert!(run.read(&reading(false, None, &[])).is_empty());
}

#[test]
fn what_was_held_is_given_back_when_nobody_waits() {
    let held = HostWide { clipboard: true, files: false };
    let mut run = started(Policy::linux(), Some(held));
    assert_eq!(run.pairing.held(), Some(held));
    let status = Status { clipboard: Some(false), files: Some(false), ..reading(false, None, &[]) };
    assert_eq!(run.read(&status), ["allow clipboard on"]);
    assert_eq!(run.pairing.held(), None);
}

#[test]
fn what_was_held_stays_held_while_a_device_waits_and_comes_back_after() {
    let held = HostWide { clipboard: true, files: true };
    let mut run = started(Policy::linux(), Some(held));
    let off = |waiting: Option<Pending>, devices: &[(&str, &str)]| Status {
        clipboard: Some(false),
        files: Some(false),
        ..reading(false, waiting, devices)
    };
    assert!(run.read(&off(Some(pending(KITCHEN, "Tab", 40)), &[])).is_empty());
    assert_eq!(run.pairing.held(), Some(held));
    run.event(Event::Accept);
    let lines = run.read(&off(None, &[(KITCHEN, "Tab")]));
    assert_eq!(lines.len(), 4);
    assert_eq!(&lines[2..], ["allow clipboard on", "allow files on"]);
}

#[test]
fn starting_again_forgets_the_devices_it_knew() {
    let mut run = Run::idle(plain(), &[(OLD, "Old phone")]);
    run.open(120);
    run.event(Event::Started { held: None });
    // The first reading after a start only learns what is there: nothing counts as new.
    assert!(run.read(&reading(false, None, &[(OLD, "Old phone"), (PHONE, "Phone")])).is_empty());
    assert_eq!(run.pairing.view().paired, None);
    assert_eq!(run.open(30), ["pair on"]);
}

#[test]
fn a_device_accepted_when_the_program_died_is_left_as_the_host_has_it() {
    // What the program cannot know after a restart: that this device was just let in and its
    // limits not yet sent. It reads the device as one that was always there.
    let mut run = started(plain(), None);
    assert!(run.read(&reading(false, None, &[(KITCHEN, "Tab")])).is_empty());
    assert!(!run.pairing.busy());
}

#[test]
fn opening_waits_for_the_first_reading_after_a_start() {
    let mut run = started(plain(), None);
    assert!(run.open(60).is_empty());
    assert_eq!(run.refused.as_deref(), Some(NOT_READ_YET));
}
