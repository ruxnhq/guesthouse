//! A pairing the host times itself: fed what the host said, with no readings and no clock of its own.

use super::tests::{KITCHEN, PHONE, Run, plain};
use super::*;
use crate::events::PairingEvent;

fn host_run() -> Run {
    let mut run = Run::new(plain());
    run.pairing = Pairing::host_timed(plain());
    run
}

/// Feeds what the host said, and gives the lines sent.
fn says(run: &mut Run, said: PairingEvent) -> Vec<String> {
    let step = run.pairing.host_says(run.now, &said);
    assert!(step.refused.is_none() && !step.read_now);
    run.pairing = step.pairing;
    step.commands.iter().map(|words| words.join(" ")).collect()
}

fn open_until(seconds: u64) -> PairingEvent {
    PairingEvent::PairingOpen { until_unix_seconds: seconds }
}

fn waits(fingerprint: &str, name: &str, seconds: u32) -> PairingEvent {
    PairingEvent::DeviceWaits {
        name: name.into(),
        fingerprint: fingerprint.into(),
        code: "123 456".into(),
        seconds,
    }
}

fn closed(reason: CloseReason) -> PairingEvent {
    PairingEvent::PairingClosed { reason }
}

#[test]
fn it_starts_closed_and_opens_with_the_time_for_the_host_without_a_reading() {
    let mut run = host_run();
    assert!(run.pairing.is_host_timed() && !run.pairing.busy());
    assert_eq!(run.open(90), ["pair on 90"]);
    assert_eq!(run.pairing.view().until_ms, Some(run.now + 90_000));
    // The host's own end replaces ours.
    assert!(says(&mut run, open_until(1_100)).is_empty());
    assert_eq!(run.pairing.view().until_ms, Some(1_100_000));
    // A new time while open is told to the host.
    assert_eq!(run.open(300), ["pair on 300"]);
    assert_eq!(run.pairing.poll_ms(), None);
}

#[test]
fn time_never_closes_it_and_nothing_asks_for_a_reading() {
    let mut run = host_run();
    says(&mut run, open_until(1_010));
    for _ in 0..10 {
        assert!(run.pass(60_000).is_empty());
        assert!(!run.read_now);
    }
    assert!(run.pairing.view().open);
    assert!(says(&mut run, closed(CloseReason::Timeout)).is_empty());
    let view = run.pairing.view();
    assert!(!view.open);
    assert_eq!(view.closed_reason, Some(CloseReason::Timeout));
}

#[test]
fn a_device_waits_with_the_seconds_the_host_gave_and_is_paired() {
    let mut run = host_run();
    says(&mut run, open_until(1_120));
    run.now += 4_000;
    says(&mut run, waits(KITCHEN, "Kitchen tablet", 56));
    let view = run.pairing.view();
    assert!(!view.open);
    assert_eq!(view.pending.as_ref().unwrap().code, "123 456");
    assert_eq!(view.pending_until_ms, Some(run.now + 56_000));
    assert_eq!(run.event(Event::Accept), ["pair accept"]);
    // The policy's limits for the new device go out as it is let in.
    let lines =
        says(&mut run, PairingEvent::Paired { fingerprint: KITCHEN.into(), name: "Kitchen tablet".into() });
    assert_eq!(
        lines,
        [format!("device {KITCHEN} allow clipboard off"), format!("device {KITCHEN} allow files off")]
    );
    assert!(says(&mut run, closed(CloseReason::Paired)).is_empty());
    let view = run.pairing.view();
    assert_eq!(
        (view.closed_reason, view.paired.as_deref()),
        (Some(CloseReason::Paired), Some("Kitchen tablet"))
    );
    assert!(view.pending.is_none() && !run.pairing.busy());
}

#[test]
fn said_again_changes_nothing_more() {
    let mut run = host_run();
    says(&mut run, open_until(1_120));
    says(&mut run, open_until(1_130));
    assert_eq!(run.pairing.view().until_ms, Some(1_130_000));
    says(&mut run, waits(KITCHEN, "Kitchen tablet", 60));
    run.now += 5_000;
    says(&mut run, waits(KITCHEN, "Kitchen tablet", 55));
    assert_eq!(run.pairing.view().pending_until_ms, Some(run.now + 55_000));
    run.event(Event::Accept);
    // Said again while accepted, it stays accepted.
    says(&mut run, waits(KITCHEN, "Kitchen tablet", 50));
    assert!(run.event(Event::Accept).is_empty() && run.refused.is_none());
    let paired = PairingEvent::Paired { fingerprint: KITCHEN.into(), name: "Kitchen tablet".into() };
    assert_eq!(says(&mut run, paired.clone()).len(), 2);
    assert!(says(&mut run, paired).is_empty());
    // Closed after paired, for whatever reason, stays paired.
    says(&mut run, closed(CloseReason::Other));
    says(&mut run, closed(CloseReason::Paired));
    assert_eq!(run.pairing.view().closed_reason, Some(CloseReason::Paired));
}

#[test]
fn out_of_order_the_host_wins() {
    // Paired after closed for another reason: paired.
    let mut run = host_run();
    says(&mut run, waits(KITCHEN, "Kitchen tablet", 60));
    says(&mut run, closed(CloseReason::Declined));
    says(&mut run, PairingEvent::Paired { fingerprint: KITCHEN.into(), name: "Kitchen tablet".into() });
    assert_eq!(run.pairing.view().closed_reason, Some(CloseReason::Paired));
    // A device waits with nothing open; a new opening ends its wait; another device replaces it.
    let mut run = host_run();
    says(&mut run, waits(KITCHEN, "Kitchen tablet", 60));
    assert!(run.pairing.busy());
    says(&mut run, open_until(1_200));
    assert!(run.pairing.view().open && run.pairing.view().pending.is_none());
    says(&mut run, waits(PHONE, "Phone", 60));
    assert_eq!(run.pairing.view().pending.unwrap().name, "Phone");
    // Closed with nothing open is kept only when no reason is known yet.
    let mut run = host_run();
    says(&mut run, closed(CloseReason::Timeout));
    assert_eq!(run.pairing.view().closed_reason, Some(CloseReason::Timeout));
    says(&mut run, closed(CloseReason::Other));
    assert_eq!(run.pairing.view().closed_reason, Some(CloseReason::Timeout));
}

#[test]
fn the_persons_answers_go_as_words_and_the_host_closes() {
    let mut run = host_run();
    says(&mut run, waits(KITCHEN, "Kitchen tablet", 60));
    assert_eq!(run.event(Event::Decline), ["pair decline"]);
    assert_eq!(run.pairing.view().closed_reason, Some(CloseReason::Declined));
    says(&mut run, closed(CloseReason::Declined));
    assert_eq!(run.pairing.view().closed_reason, Some(CloseReason::Declined));
    run.open(60);
    assert_eq!(run.event(Event::Stop), ["pair off"]);
    assert_eq!(run.pairing.view().closed_reason, Some(CloseReason::Asked));
}

#[test]
fn starting_again_keeps_it_host_timed_and_closed() {
    let mut run = host_run();
    says(&mut run, open_until(1_200));
    run.event(Event::Started { held: None });
    assert!(run.pairing.is_host_timed() && !run.pairing.busy());
    assert_eq!(run.open(30), ["pair on 30"]);
}

#[test]
fn a_pairing_read_from_status_is_unchanged_by_being_able_to_be_told() {
    // The plain pairing still waits for a reading before opening, and sends `pair on` with no time.
    let mut run = Run::new(plain());
    assert!(!run.pairing.is_host_timed());
    assert!(run.open(60).is_empty());
    run.read(&super::tests::reading(false, None, &[]));
    assert_eq!(run.open(60), ["pair on"]);
    assert_eq!(run.pairing.poll_ms(), Some(POLL_MS));
}
