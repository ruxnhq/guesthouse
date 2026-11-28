//! The pairing flows, with a host that holds nothing host-wide so that the lines are the plain ones.

use super::*;
use crate::share::{DeviceInfo, Permissions};

pub(super) const VIEWER: &str = "1111111111111111111111111111111111111111111111111111111111111111";
pub(super) const KITCHEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub(super) const PHONE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
pub(super) const OLD: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

pub(super) fn plain() -> Policy {
    Policy { hold_host_wide: false, hide: None }
}

pub(super) fn pending(fingerprint: &str, name: &str, seconds: u64) -> Pending {
    Pending { name: name.into(), fingerprint: fingerprint.into(), code: "123 456".into(), seconds }
}

pub(super) fn device(fingerprint: &str, name: &str) -> DeviceInfo {
    DeviceInfo {
        fingerprint: fingerprint.into(),
        name: name.into(),
        connected: false,
        permissions: Permissions::default(),
    }
}

/// A status as the host would say it.
pub(super) fn reading(open: bool, waiting: Option<Pending>, devices: &[(&str, &str)]) -> Status {
    Status {
        pairing: open,
        pending: waiting,
        devices: devices.iter().map(|(fingerprint, name)| device(fingerprint, name)).collect(),
        ..Status::default()
    }
}

/// A run of events at a clock the test moves, with every command sent written down as a line.
pub(super) struct Run {
    pub pairing: Pairing,
    pub now: Millis,
    pub sent: Vec<String>,
    pub read_now: bool,
    pub refused: Option<String>,
}

impl Run {
    pub fn new(policy: Policy) -> Run {
        Run {
            pairing: Pairing::new(policy),
            now: 1_000_000,
            sent: Vec::new(),
            read_now: false,
            refused: None,
        }
    }

    /// A run that has read an idle host with `devices` on it.
    pub fn idle(policy: Policy, devices: &[(&str, &str)]) -> Run {
        let mut run = Run::new(policy);
        run.read(&reading(false, None, devices));
        run.sent.clear();
        run
    }

    /// Applies an event, and gives the lines it sends.
    pub fn event(&mut self, event: Event) -> Vec<String> {
        let before_waiting = self.pairing.view().pending.is_some();
        let step = self.pairing.step(self.now, event);
        let lines: Vec<String> = step.commands.iter().map(|words| words.join(" ")).collect();
        // `pair off` declines the device that waits, so it is never sent while one does.
        assert!(
            !(before_waiting && lines.iter().any(|line| line == "pair off")),
            "pair off sent while a device waits: {lines:?}"
        );
        self.pairing = step.pairing;
        self.read_now = step.read_now;
        self.refused = step.refused;
        self.sent.extend(lines.clone());
        lines
    }

    pub fn read(&mut self, status: &Status) -> Vec<String> {
        self.event(Event::Reading(status))
    }

    pub fn open(&mut self, seconds: u64) -> Vec<String> {
        self.event(Event::Open { seconds })
    }

    /// The clock moves on, and the time is told.
    pub fn pass(&mut self, ms: Millis) -> Vec<String> {
        self.now += ms;
        self.event(Event::Tick)
    }
}

#[test]
fn opening_sends_pair_on_and_sets_the_end_by_our_clock() {
    let mut run = Run::idle(plain(), &[]);
    let start = run.now;
    assert_eq!(run.open(120), ["pair on"]);
    let view = run.pairing.view();
    assert!(view.open);
    assert_eq!(view.until_ms, Some(start + 120_000));
    assert_eq!((view.pending, view.closed_reason, view.paired), (None, None, None));
    assert_eq!(run.pairing.poll_ms(), Some(POLL_MS));
}

#[test]
fn opening_is_not_done_before_the_host_has_been_read() {
    let mut run = Run::new(plain());
    assert!(run.open(60).is_empty());
    assert_eq!(run.refused.as_deref(), Some(NOT_READ_YET));
    assert!(!run.pairing.view().open);
}

#[test]
fn a_time_outside_one_to_3600_seconds_is_refused_and_nothing_changes() {
    let mut run = Run::idle(plain(), &[]);
    for seconds in [0, 3601, 1_000_000] {
        assert!(run.open(seconds).is_empty());
        assert_eq!(run.refused.as_deref(), Some("Pairing can stay open for 1 to 3600 seconds."));
        assert!(!run.pairing.view().open);
    }
    assert_eq!(run.open(1), ["pair on"]);
    assert!(run.refused.is_none());
}

#[test]
fn the_clock_runs_out_and_pair_off_goes_only_after_a_reading_shows_nobody_came() {
    let mut run = Run::idle(plain(), &[(VIEWER, "Guesthouse")]);
    run.open(30);
    run.read(&reading(true, None, &[(VIEWER, "Guesthouse")]));
    assert!(run.pass(29_000).is_empty() && !run.read_now);
    // The time is up. Nothing is sent: a device may have come, and `pair off` would decline it.
    assert!(run.pass(1_000).is_empty());
    assert!(run.read_now);
    assert!(run.pairing.view().open);
    assert_eq!(run.read(&reading(true, None, &[(VIEWER, "Guesthouse")])), ["pair off"]);
    let view = run.pairing.view();
    assert!(!view.open);
    assert_eq!(view.closed_reason, Some(CloseReason::Timeout));
    assert_eq!(run.pairing.poll_ms(), None);
}

#[test]
fn a_device_that_came_as_the_time_ran_out_is_not_declined() {
    let mut run = Run::idle(plain(), &[]);
    run.open(10);
    run.read(&reading(true, None, &[]));
    run.pass(10_000);
    assert!(run.read_now);
    let waiting = reading(false, Some(pending(KITCHEN, "Kitchen tablet", 58)), &[]);
    assert!(run.read(&waiting).is_empty());
    assert_eq!(run.pairing.view().pending.unwrap().name, "Kitchen tablet");
    assert!(run.sent.iter().all(|line| line != "pair off"));
}

#[test]
fn a_device_let_in_as_the_time_ran_out_is_paired() {
    let mut run = Run::idle(plain(), &[]);
    run.open(10);
    run.pass(10_000);
    let lines = run.read(&reading(false, None, &[(KITCHEN, "Kitchen tablet")]));
    assert_eq!(
        lines,
        [format!("device {KITCHEN} allow clipboard off"), format!("device {KITCHEN} allow files off")]
    );
    assert_eq!(run.pairing.view().closed_reason, Some(CloseReason::Paired));
}

#[test]
fn when_the_time_is_up_and_the_host_closed_it_already_nothing_is_sent() {
    let mut run = Run::idle(plain(), &[]);
    run.open(10);
    run.read(&reading(true, None, &[]));
    run.pass(10_000);
    // Closed by the host, as a device's arrival does: looked at twice before it is believed.
    assert!(run.read(&reading(false, None, &[])).is_empty());
    assert!(run.pairing.view().open);
    assert!(run.read(&reading(false, None, &[])).is_empty());
    assert_eq!(run.pairing.view().closed_reason, Some(CloseReason::Timeout));
    assert!(run.sent.iter().all(|line| line != "pair off"));
}

#[test]
fn asking_again_while_open_sets_a_new_end_and_sends_nothing() {
    let mut run = Run::idle(plain(), &[]);
    run.open(30);
    run.pass(20_000);
    let now = run.now;
    assert!(run.open(300).is_empty());
    assert_eq!(run.pairing.view().until_ms, Some(now + 300_000));
    // And once the time was up, too.
    run.pass(300_000);
    assert!(run.read_now);
    assert!(run.open(60).is_empty());
    run.pass(30_000);
    assert!(!run.read_now);
    assert!(run.pairing.view().open);
}

#[test]
fn the_clock_stops_while_a_device_waits() {
    let mut run = Run::idle(plain(), &[]);
    run.open(30);
    run.read(&reading(true, None, &[]));
    run.pass(25_000);
    run.read(&reading(false, Some(pending(KITCHEN, "Kitchen tablet", 60)), &[]));
    // An hour goes by with the device still waiting: no limit of ours cuts in.
    for _ in 0..60 {
        assert!(run.pass(60_000).is_empty());
        assert!(!run.read_now);
        assert!(run.read(&reading(false, Some(pending(KITCHEN, "Kitchen tablet", 60)), &[])).is_empty());
    }
    assert!(run.pairing.view().pending.is_some());
    assert!(!run.pairing.view().open);
}

#[test]
fn a_device_waits_with_its_code_and_pairing_is_no_longer_open() {
    let mut run = Run::idle(plain(), &[]);
    run.open(120);
    run.read(&reading(true, None, &[]));
    run.pass(5_000);
    run.read(&reading(false, Some(pending(KITCHEN, "Kitchen tablet", 55)), &[]));
    let view = run.pairing.view();
    assert!(!view.open && view.until_ms.is_none());
    let waiting = view.pending.unwrap();
    assert_eq!((waiting.name.as_str(), waiting.code.as_str()), ("Kitchen tablet", "123 456"));
    assert_eq!(view.pending_until_ms, Some(run.now + 55_000));
    assert_eq!(view.closed_reason, None);
}

#[test]
fn accepting_sends_accept_and_then_limits_the_device_when_it_appears() {
    let mut run = Run::idle(plain(), &[(VIEWER, "Guesthouse")]);
    run.open(120);
    run.read(&reading(true, None, &[(VIEWER, "Guesthouse")]));
    run.read(&reading(false, Some(pending(KITCHEN, "Kitchen tablet", 50)), &[(VIEWER, "Guesthouse")]));
    run.sent.clear();
    assert_eq!(run.event(Event::Accept), [format!("pair accept {KITCHEN}")]);
    // The device is shown until it is among the devices, so the card does not flicker.
    assert!(run.pairing.view().pending.is_some());
    assert!(run.pairing.busy());
    // Pressed twice: nothing more is sent.
    assert!(run.event(Event::Accept).is_empty() && run.refused.is_none());
    // The host has not added it yet.
    run.pass(1_000);
    assert!(run.read(&reading(false, None, &[(VIEWER, "Guesthouse")])).is_empty());
    run.pass(1_000);
    let both = &[(VIEWER, "Guesthouse"), (KITCHEN, "Kitchen tablet")];
    assert_eq!(
        run.read(&reading(false, None, both)),
        [format!("device {KITCHEN} allow clipboard off"), format!("device {KITCHEN} allow files off")]
    );
    let view = run.pairing.view();
    assert_eq!(
        (view.closed_reason, view.paired.as_deref()),
        (Some(CloseReason::Paired), Some("Kitchen tablet"))
    );
    assert!(view.pending.is_none() && !view.open);
    assert_eq!(run.pairing.poll_ms(), None);
}

#[test]
fn a_device_accepted_that_never_appears_closes_once_the_grace_is_over() {
    let mut run = Run::idle(plain(), &[]);
    run.open(120);
    run.read(&reading(true, None, &[]));
    run.read(&reading(false, Some(pending(KITCHEN, "Kitchen tablet", 3)), &[]));
    run.event(Event::Accept);
    run.pass(9_000);
    assert!(run.read(&reading(false, None, &[])).is_empty());
    assert!(run.pairing.busy());
    run.pass(2_000);
    assert!(run.read(&reading(false, None, &[])).is_empty());
    assert!(!run.pairing.busy());
    // Its own time had run out as it was accepted.
    assert_eq!(run.pairing.view().closed_reason, Some(CloseReason::Timeout));
}

#[test]
fn declining_turns_the_device_away_and_that_pairing_is_used_up() {
    let mut run = Run::idle(plain(), &[]);
    run.open(120);
    run.read(&reading(true, None, &[]));
    run.read(&reading(false, Some(pending(KITCHEN, "Kitchen tablet", 50)), &[]));
    assert_eq!(run.event(Event::Decline), [format!("pair decline {KITCHEN}")]);
    let view = run.pairing.view();
    assert_eq!(view.closed_reason, Some(CloseReason::Declined));
    assert!(!view.open && view.pending.is_none());
    // A second press has nobody to answer.
    assert!(run.event(Event::Decline).is_empty());
    assert_eq!(run.refused.as_deref(), Some(NOBODY_WAITING));
    // Pairing is not opened again by itself: the host still saying it is open is closed.
    assert_eq!(run.read(&reading(true, None, &[])), ["pair off"]);
    assert_eq!(run.pairing.view().closed_reason, Some(CloseReason::Declined));
}

#[test]
fn a_device_the_host_turned_away_is_a_timeout_when_its_time_was_up_and_declined_when_not() {
    let table = [
        // (seconds the host said at the last look, ms until it was gone, reason)
        (60, 59_000, CloseReason::Timeout),
        (3, 1_500, CloseReason::Timeout),
        (60, 1_000, CloseReason::Declined),
        (60, 30_000, CloseReason::Declined),
        (20, 10_000, CloseReason::Declined),
        (20, 18_000, CloseReason::Timeout),
    ];
    for (seconds, after, want) in table {
        let mut run = Run::idle(plain(), &[]);
        run.open(120);
        run.read(&reading(true, None, &[]));
        run.read(&reading(false, Some(pending(KITCHEN, "Tab", seconds)), &[]));
        run.pass(after);
        assert!(run.read(&reading(false, None, &[])).is_empty());
        assert_eq!(run.pairing.view().closed_reason, Some(want), "{seconds} {after}");
        assert!(!run.pairing.busy());
    }
}

#[test]
fn a_new_fingerprint_seen_without_having_seen_it_wait_is_paired() {
    let mut run = Run::idle(plain(), &[(OLD, "Old phone")]);
    run.open(120);
    run.read(&reading(true, None, &[(OLD, "Old phone")]));
    let lines = run.read(&reading(false, None, &[(OLD, "Old phone"), (PHONE, "New phone")]));
    assert_eq!(
        lines,
        [format!("device {PHONE} allow clipboard off"), format!("device {PHONE} allow files off")]
    );
    assert_eq!(run.pairing.view().paired.as_deref(), Some("New phone"));
}

#[test]
fn a_reading_between_a_devices_arrival_and_its_hello_is_not_taken_for_a_close() {
    let mut run = Run::idle(plain(), &[]);
    run.open(120);
    run.read(&reading(true, None, &[]));
    // The host turns pairing off as the device connects, and has its name a moment later.
    assert!(run.read(&reading(false, None, &[])).is_empty());
    assert!(run.pairing.view().open);
    assert!(run.read(&reading(true, None, &[])).is_empty());
    assert!(run.read(&reading(false, None, &[])).is_empty());
    assert!(run.read(&reading(false, Some(pending(KITCHEN, "Tab", 60)), &[])).is_empty());
    assert!(run.pairing.view().pending.is_some());
    assert!(run.sent.iter().all(|line| line != "pair off"));
}

#[test]
fn pairing_that_the_host_closed_is_other_and_is_not_closed_again() {
    let mut run = Run::idle(plain(), &[]);
    run.open(120);
    run.read(&reading(true, None, &[]));
    run.read(&reading(false, None, &[]));
    assert!(run.read(&reading(false, None, &[])).is_empty());
    assert_eq!(run.pairing.view().closed_reason, Some(CloseReason::Other));
    assert_eq!(run.sent, ["pair on"]);
}

#[test]
fn sharing_switched_off_in_the_host_closes_it_at_once() {
    let mut run = Run::idle(plain(), &[]);
    run.open(120);
    run.read(&reading(true, None, &[]));
    let paused = Status { paused: true, ..reading(false, None, &[]) };
    assert!(run.read(&paused).is_empty());
    assert_eq!(run.pairing.view().closed_reason, Some(CloseReason::Other));
}

#[test]
fn a_reading_before_the_command_lands_is_ignored_for_a_moment() {
    let mut run = Run::idle(plain(), &[]);
    run.open(120);
    for _ in 0..3 {
        run.pass(1_000);
        assert!(run.read(&reading(false, None, &[])).is_empty());
        assert!(run.pairing.view().open);
    }
    // Five seconds on, with it never seen open, it did not open.
    run.pass(3_000);
    run.read(&reading(false, None, &[]));
    run.read(&reading(false, None, &[]));
    assert_eq!(run.pairing.view().closed_reason, Some(CloseReason::Other));
}

#[test]
fn stopping_closes_it_and_says_asked() {
    let mut run = Run::idle(plain(), &[]);
    run.open(120);
    assert_eq!(run.event(Event::Stop), ["pair off"]);
    assert_eq!(run.pairing.view().closed_reason, Some(CloseReason::Asked));
    assert!(run.event(Event::Stop).is_empty());
    assert_eq!(run.refused.as_deref(), Some(NOT_OPEN));
}

#[test]
fn stopping_while_a_device_waits_declines_it_and_never_sends_pair_off() {
    let mut run = Run::idle(plain(), &[]);
    run.open(120);
    run.read(&reading(true, None, &[]));
    run.read(&reading(false, Some(pending(KITCHEN, "Tab", 40)), &[]));
    assert_eq!(run.event(Event::Stop), [format!("pair decline {KITCHEN}")]);
    assert_eq!(run.pairing.view().closed_reason, Some(CloseReason::Asked));
}

#[test]
fn answering_with_nobody_waiting_is_refused() {
    let mut run = Run::idle(plain(), &[]);
    for event in [Event::Accept, Event::Decline] {
        assert!(run.event(event).is_empty());
        assert_eq!(run.refused.as_deref(), Some(NOBODY_WAITING));
    }
    run.open(60);
    assert!(run.event(Event::Accept).is_empty());
    assert_eq!(run.refused.as_deref(), Some(NOBODY_WAITING));
}

#[test]
fn opening_while_a_device_waits_is_refused() {
    let mut run = Run::idle(plain(), &[]);
    run.open(120);
    run.read(&reading(true, None, &[]));
    run.read(&reading(false, Some(pending(KITCHEN, "Tab", 40)), &[]));
    assert!(run.open(60).is_empty());
    assert_eq!(run.refused.as_deref(), Some(DEVICE_WAITING));
    run.event(Event::Accept);
    assert!(run.open(60).is_empty());
    assert_eq!(run.refused.as_deref(), Some(DEVICE_WAITING));
}

#[test]
fn the_switch_going_off_or_quitting_leaves_nothing_open_or_waiting() {
    for event in [Event::SwitchOff, Event::Shutdown] {
        let mut run = Run::idle(plain(), &[]);
        assert!(run.event(event).is_empty(), "closed: nothing to send");

        run.open(120);
        assert_eq!(run.event(event), ["pair off"]);
        assert!(!run.pairing.busy());

        run.open(120);
        run.read(&reading(true, None, &[]));
        run.read(&reading(false, Some(pending(KITCHEN, "Tab", 40)), &[]));
        assert_eq!(run.event(event), [format!("pair decline {KITCHEN}")]);
        assert!(!run.pairing.busy());
    }
    let mut unread = Run::new(plain());
    assert!(unread.event(Event::Shutdown).is_empty());
}

#[test]
fn opening_again_forgets_how_the_last_one_ended() {
    let mut run = Run::idle(plain(), &[]);
    run.open(120);
    run.read(&reading(true, None, &[]));
    run.read(&reading(false, None, &[(KITCHEN, "Tab")]));
    assert_eq!(run.pairing.view().paired.as_deref(), Some("Tab"));
    run.open(120);
    let view = run.pairing.view();
    assert_eq!((view.closed_reason, view.paired), (None, None));
}

#[test]
fn the_clock_never_runs_backwards() {
    let mut run = Run::idle(plain(), &[]);
    run.open(30);
    let until = run.pairing.view().until_ms;
    run.now -= 5_000;
    assert!(run.event(Event::Tick).is_empty());
    assert!(run.pairing.view().until_ms >= until);
    assert!(!run.read_now);
}

#[test]
fn nothing_needs_polling_until_pairing_is_open() {
    let mut run = Run::new(plain());
    assert_eq!(run.pairing.poll_ms(), None);
    run.read(&reading(false, None, &[]));
    assert_eq!(run.pairing.poll_ms(), None);
    assert!(!run.pairing.busy());
}

#[test]
fn owing_more_adds_to_what_is_owed_and_nothing_adds_nothing() {
    let mut pairing = Pairing::new(plain());
    pairing.owe(None);
    assert_eq!(pairing.held(), None);
    pairing.owe(Some(HostWide { clipboard: true, files: false }));
    pairing.owe(Some(HostWide { clipboard: false, files: true }));
    assert_eq!(pairing.held(), Some(HostWide { clipboard: true, files: true }));
}

#[test]
fn an_answer_names_the_device_shown_last() {
    let mut run = Run::idle(plain(), &[]);
    run.open(120);
    run.read(&reading(true, None, &[]));
    run.read(&reading(false, Some(pending(KITCHEN, "Kitchen tablet", 50)), &[]));
    // Another device took the place before the person answered: the person is shown that one.
    run.read(&reading(false, Some(pending(PHONE, "Phone", 58)), &[]));
    assert_eq!(run.pairing.view().pending.unwrap().fingerprint, PHONE);
    assert_eq!(run.event(Event::Accept), [format!("pair accept {PHONE}")]);
    // The guest found yet another one waiting and let nobody in: that one is shown, not trusted.
    run.read(&reading(false, Some(pending(OLD, "Old phone", 59)), &[]));
    let view = run.pairing.view();
    assert_eq!(view.pending.unwrap().fingerprint, OLD);
    assert!(view.paired.is_none() && run.pairing.busy());
}

#[test]
fn a_device_with_no_fingerprint_is_never_answered() {
    let mut run = Run::idle(plain(), &[]);
    run.open(120);
    run.read(&reading(true, None, &[]));
    run.read(&reading(false, Some(pending("Tablet", "Tab", 50)), &[]));
    for event in [Event::Accept, Event::Decline] {
        assert!(run.event(event).is_empty());
        assert_eq!(run.refused.as_deref(), Some(ctl::NOT_A_FINGERPRINT));
    }
    assert!(run.event(Event::Shutdown).is_empty());
}
