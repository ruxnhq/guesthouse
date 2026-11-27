//! The events each pairing flow makes, from the pairing before and after each step.

use super::tests::{KITCHEN, PHONE, Run, VIEWER, pending, plain, reading};
use super::*;
use crate::events::{PairingEvent, pairing_events};

/// Applies `event` to the run, and gives the events its step made.
fn said(run: &mut Run, event: Event) -> Vec<PairingEvent> {
    let before = run.pairing.clone();
    run.event(event);
    pairing_events(&before, &run.pairing, run.now)
}

fn read(run: &mut Run, status: &Status) -> Vec<PairingEvent> {
    said(run, Event::Reading(status))
}

fn pass(run: &mut Run, ms: Millis) -> Vec<PairingEvent> {
    run.now += ms;
    said(run, Event::Tick)
}

fn closed(reason: CloseReason) -> PairingEvent {
    PairingEvent::PairingClosed { reason }
}

fn waits(fingerprint: &str, name: &str, seconds: u32) -> PairingEvent {
    PairingEvent::DeviceWaits {
        name: name.into(),
        fingerprint: fingerprint.into(),
        code: "123 456".into(),
        seconds,
    }
}

#[test]
fn opening_says_until_when_in_seconds_rounded_up_and_ticks_say_nothing() {
    let mut run = Run::idle(plain(), &[]);
    run.now = 1_700_000_000_400;
    assert_eq!(
        said(&mut run, Event::Open { seconds: 120 }),
        [PairingEvent::PairingOpen { until_unix_seconds: 1_700_000_121 }]
    );
    assert!(read(&mut run, &reading(true, None, &[])).is_empty());
    assert!(pass(&mut run, 1_000).is_empty());
    assert!(pass(&mut run, 60_000).is_empty());
}

#[test]
fn a_new_end_while_open_is_said_again_and_the_time_running_out_is_not() {
    let mut run = Run::idle(plain(), &[]);
    run.now = 1_000_000;
    said(&mut run, Event::Open { seconds: 30 });
    run.now += 20_000;
    assert_eq!(
        said(&mut run, Event::Open { seconds: 300 }),
        [PairingEvent::PairingOpen { until_unix_seconds: 1_000 + 20 + 300 }]
    );
    assert!(read(&mut run, &reading(true, None, &[])).is_empty());
    // The time is up: nothing until a reading closes it.
    assert!(pass(&mut run, 300_000).is_empty());
    assert_eq!(read(&mut run, &reading(true, None, &[])), [closed(CloseReason::Timeout)]);
    // Opened again after it closed.
    assert_eq!(
        said(&mut run, Event::Open { seconds: 60 }),
        [PairingEvent::PairingOpen { until_unix_seconds: (run.now + 60_000) / 1000 }]
    );
}

#[test]
fn a_device_that_waits_is_said_once_with_the_seconds_the_host_gave() {
    let mut run = Run::idle(plain(), &[]);
    said(&mut run, Event::Open { seconds: 120 });
    read(&mut run, &reading(true, None, &[]));
    run.now += 5_000;
    let waiting = reading(false, Some(pending(KITCHEN, "Kitchen tablet", 55)), &[]);
    assert_eq!(read(&mut run, &waiting), [waits(KITCHEN, "Kitchen tablet", 55)]);
    // The readings after it say nothing more, however long it waits.
    for _ in 0..5 {
        assert!(pass(&mut run, 1_000).is_empty());
        assert!(
            read(&mut run, &reading(false, Some(pending(KITCHEN, "Kitchen tablet", 50)), &[])).is_empty()
        );
    }
}

#[test]
fn accepted_and_appeared_says_paired_then_closed() {
    let mut run = Run::idle(plain(), &[(VIEWER, "Guesthouse")]);
    said(&mut run, Event::Open { seconds: 120 });
    read(&mut run, &reading(false, Some(pending(KITCHEN, "Kitchen tablet", 50)), &[(VIEWER, "Guesthouse")]));
    assert!(said(&mut run, Event::Accept).is_empty());
    assert!(read(&mut run, &reading(false, None, &[(VIEWER, "Guesthouse")])).is_empty());
    let both = &[(VIEWER, "Guesthouse"), (KITCHEN, "Kitchen tablet")];
    assert_eq!(
        read(&mut run, &reading(false, None, both)),
        [
            PairingEvent::Paired { fingerprint: KITCHEN.into(), name: "Kitchen tablet".into() },
            closed(CloseReason::Paired),
        ]
    );
    // Nothing more once it is closed.
    assert!(read(&mut run, &reading(false, None, both)).is_empty());
}

#[test]
fn a_device_already_trusted_that_pairs_again_is_said_with_its_fingerprint() {
    // It is among the devices before and after, so only the pairing knows which one it was.
    let mut run = Run::idle(plain(), &[(KITCHEN, "Kitchen tablet")]);
    said(&mut run, Event::Open { seconds: 120 });
    read(&mut run, &reading(false, Some(pending(PHONE, "Phone", 50)), &[(KITCHEN, "Kitchen tablet")]));
    said(&mut run, Event::Accept);
    let events = read(&mut run, &reading(false, None, &[(KITCHEN, "Kitchen tablet"), (PHONE, "Phone")]));
    assert_eq!(
        events,
        [
            PairingEvent::Paired { fingerprint: PHONE.into(), name: "Phone".into() },
            closed(CloseReason::Paired)
        ]
    );
}

#[test]
fn declined_stopped_and_switched_off_say_why() {
    let mut run = Run::idle(plain(), &[]);
    said(&mut run, Event::Open { seconds: 120 });
    read(&mut run, &reading(false, Some(pending(KITCHEN, "Kitchen tablet", 50)), &[]));
    assert_eq!(said(&mut run, Event::Decline), [closed(CloseReason::Declined)]);

    said(&mut run, Event::Open { seconds: 120 });
    assert_eq!(said(&mut run, Event::Stop), [closed(CloseReason::Asked)]);

    said(&mut run, Event::Open { seconds: 120 });
    assert_eq!(said(&mut run, Event::SwitchOff), [closed(CloseReason::Asked)]);
    // Nothing open: nothing to say.
    assert!(said(&mut run, Event::Shutdown).is_empty());
}

#[test]
fn a_device_whose_time_ran_out_is_a_timeout() {
    let mut run = Run::idle(plain(), &[]);
    said(&mut run, Event::Open { seconds: 120 });
    read(&mut run, &reading(false, Some(pending(KITCHEN, "Kitchen tablet", 3)), &[]));
    run.now += 3_000;
    assert_eq!(read(&mut run, &reading(false, None, &[])), [closed(CloseReason::Timeout)]);
}

#[test]
fn another_device_after_one_was_accepted_is_said_too() {
    let mut run = Run::idle(plain(), &[]);
    said(&mut run, Event::Open { seconds: 120 });
    read(&mut run, &reading(false, Some(pending(KITCHEN, "Kitchen tablet", 50)), &[]));
    said(&mut run, Event::Accept);
    assert_eq!(
        read(&mut run, &reading(false, Some(pending(PHONE, "Phone", 40)), &[])),
        [waits(PHONE, "Phone", 40)]
    );
}

#[test]
fn a_device_found_waiting_at_the_first_reading_is_said_without_an_opening() {
    let mut run = Run::new(plain());
    assert_eq!(
        read(&mut run, &reading(false, Some(pending(KITCHEN, "Kitchen tablet", 30)), &[])),
        [waits(KITCHEN, "Kitchen tablet", 30)]
    );
}

#[test]
fn starting_again_with_pairing_open_says_it_closed_for_another_reason() {
    let mut run = Run::idle(plain(), &[]);
    said(&mut run, Event::Open { seconds: 120 });
    assert_eq!(said(&mut run, Event::Started { held: None }), [closed(CloseReason::Other)]);
    // Found left open at the next reading: closed by `pair off`, with nothing more to say.
    assert!(read(&mut run, &reading(true, None, &[])).is_empty());
}

#[test]
fn a_refused_step_says_nothing() {
    let mut run = Run::new(plain());
    assert!(said(&mut run, Event::Open { seconds: 60 }).is_empty());
    assert!(said(&mut run, Event::Accept).is_empty());
    let mut run = Run::idle(plain(), &[]);
    assert!(said(&mut run, Event::Open { seconds: 0 }).is_empty());
    assert!(said(&mut run, Event::Stop).is_empty());
}
