//! Pairing a device through the shared driver against a pretend remoteman, which keeps its own
//! pairing, devices and switches as the commands change them.

use super::network::{start_and_run, stop};
use super::remote_view::{HOST, PHONE, info, refused, share, shown, viewer, wait_view, windows_machine};
use super::*;
use guesthouse_remote_view_core::policy::{HIDDEN_DEVICE, Policy};
use guesthouse_remote_view_core::share::{CloseReason, ShareInfo, ShareState};

const KITCHEN: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

/// A Windows machine running with its view open, and remoteman in it trusting Guesthouse's viewer
/// alone, with the host-wide clipboard and files on.
fn running(test: &str) -> Setup {
    let setup = setup(test, Pretend::default());
    let service = &setup.service;
    windows_machine(&setup, "win");
    {
        let mut model = service.work.remoteman.model.lock().unwrap();
        model.devices = vec![(viewer(), "Guesthouse".into(), true, true)];
        (model.clipboard, model.files) = (true, true);
    }
    share(service, "win", Some(true));
    start_and_run(service, "win");
    wait_view(service, "win", ShareState::Listening);
    setup
}

fn ask_info(service: &Arc<Service<Pretend>>, request: Request) -> ShareInfo {
    info(ask(service, request).pop().unwrap())
}

fn open(service: &Arc<Service<Pretend>>, seconds: Option<u32>) -> ShareInfo {
    ask_info(
        service,
        Request::Share { name: "win".into(), on: None, pairing: Some(true), pairing_seconds: seconds },
    )
}

/// A device comes while pairing is open: remoteman holds it and closes pairing itself.
fn arrive(service: &Arc<Service<Pretend>>, fingerprint: &str, name: &str, code: &str) {
    let mut model = service.work.remoteman.model.lock().unwrap();
    model.pending = Some((name.into(), fingerprint.into(), code.into()));
    model.pairing = false;
}

/// The commands sent since the `from`th, without the readings.
fn sent(service: &Arc<Service<Pretend>>, from: usize) -> Vec<String> {
    service.work.remoteman.asked()[from..].iter().filter(|line| *line != "status").cloned().collect()
}

fn asked_count(service: &Arc<Service<Pretend>>) -> usize {
    service.work.remoteman.asked().len()
}

/// Waits, without the app asking, for the service's own readings to have made `ready` true.
fn wait_until(service: &Arc<Service<Pretend>>, what: &str, ready: impl Fn(&ShareInfo) -> bool) -> ShareInfo {
    for _ in 0..100 {
        let now = service.sessions.with("win", |session| {
            session.shown("win", &Policy::windows(&viewer()), ShareState::Listening, Some(7792), None)
        });
        if ready(&now) {
            return now;
        }
        thread::sleep(Duration::from_millis(100));
    }
    panic!("never {what}: {:?}", service.work.remoteman.asked());
}

fn never_cut(service: &Arc<Service<Pretend>>) {
    for line in service.work.remoteman.asked() {
        assert!(!line.starts_with("sharing") && line != "drop" && !line.contains(&viewer()), "{line}");
    }
}

#[test]
fn a_device_pairs_waits_with_its_code_is_accepted_limited_and_pairing_closes_after_it() {
    let setup = running("pair-accept");
    let service = &setup.service;
    let before = asked_count(service);
    let opened = open(service, None);
    assert!(opened.pairing && opened.pairing_until_ms.is_some(), "{opened:?}");
    assert_eq!(sent(service, before), ["allow clipboard off", "allow files off", "pair on"]);

    // Read by the service once a second while it's open, with nobody asking.
    let reading = asked_count(service);
    thread::sleep(Duration::from_millis(2_500));
    assert!(asked_count(service) >= reading + 2, "{:?}", service.work.remoteman.asked());

    arrive(service, PHONE, "Zoë's phone", "123 456");
    let waiting = wait_until(service, "showed the device waiting", |info| info.pending.is_some());
    let pending = waiting.pending.unwrap();
    assert_eq!(
        (pending.name.as_str(), pending.code.as_str(), pending.fingerprint.as_str()),
        ("Zoë's phone", "123 456", PHONE)
    );
    assert!(!waiting.pairing);

    let before = asked_count(service);
    ask_info(service, Request::PairAccept { name: "win".into() });
    let paired = wait_until(service, "paired", |info| info.closed_reason.is_some());
    assert_eq!(paired.closed_reason, Some(CloseReason::Paired));
    assert_eq!(paired.paired.as_deref(), Some("Zoë's phone"));
    assert_eq!(
        sent(service, before),
        [
            "pair accept".to_owned(),
            format!("device {PHONE} allow clipboard off"),
            format!("device {PHONE} allow files off"),
            "allow clipboard on".to_owned(),
            "allow files on".to_owned()
        ]
    );
    let shown = shown(service, "win");
    assert!(
        service
            .work
            .logged
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .ends_with(": 2 devices trusted, 1 of them Guesthouse's own viewer."),
        "{:?}",
        service.work.logged.lock().unwrap()
    );
    assert_eq!(shown.devices.len(), 1, "Guesthouse's own viewer is no device: {:?}", shown.devices);
    assert_eq!(shown.devices[0].fingerprint, PHONE);
    assert!(!shown.devices[0].permissions.clipboard && !shown.devices[0].permissions.files);
    assert_eq!(shown.fingerprint.as_deref(), Some(HOST));

    // One device used the opening: nothing more is sent, and the service stops reading by itself.
    thread::sleep(Duration::from_millis(1_500));
    let settled = asked_count(service);
    thread::sleep(Duration::from_millis(2_500));
    assert_eq!(asked_count(service), settled, "{:?}", service.work.remoteman.asked());
    never_cut(service);
    stop(service, "win");
}

#[test]
fn a_device_declined_or_left_to_run_out_or_stopped_gives_the_switches_back() {
    let setup = running("pair-decline");
    let service = &setup.service;
    open(service, None);
    arrive(service, KITCHEN, "Kitchen tablet", "654 321");
    wait_until(service, "showed the device waiting", |info| info.pending.is_some());
    let before = asked_count(service);
    ask_info(service, Request::PairDecline { name: "win".into() });
    let declined = wait_until(service, "declined", |info| info.closed_reason.is_some());
    assert_eq!(declined.closed_reason, Some(CloseReason::Declined));
    assert_eq!(sent(service, before), ["pair decline", "allow clipboard on", "allow files on"]);

    // Run out: pairing for a second, and nobody comes.
    let before = asked_count(service);
    open(service, Some(1));
    let ran_out = wait_until(service, "ran out", |info| info.closed_reason == Some(CloseReason::Timeout));
    assert!(!ran_out.pairing);
    assert_eq!(
        sent(service, before),
        [
            "allow clipboard off",
            "allow files off",
            "pair on",
            "pair off",
            "allow clipboard on",
            "allow files on"
        ]
    );

    // Stopped by the app.
    let before = asked_count(service);
    open(service, None);
    let closed = ask_info(service, Request::PairClose { name: "win".into() });
    assert_eq!((closed.pairing, closed.closed_reason), (false, Some(CloseReason::Asked)));
    assert_eq!(
        sent(service, before),
        [
            "allow clipboard off",
            "allow files off",
            "pair on",
            "pair off",
            "allow clipboard on",
            "allow files on"
        ]
    );
    assert!(matches!(
        ask(service, Request::PairClose { name: "win".into() }).as_slice(),
        [Answer::Refused { .. }]
    ));
    let model = service.work.remoteman.model.lock().unwrap();
    assert!(model.clipboard && model.files && !model.pairing);
    drop(model);
    never_cut(service);
    stop(service, "win");
}

#[test]
fn devices_are_changed_by_their_fingerprint_and_guesthouses_own_viewer_never() {
    let setup = running("pair-devices");
    let service = &setup.service;
    service.work.remoteman.model.lock().unwrap().devices.push((PHONE.into(), "Phone".into(), false, false));
    let before = asked_count(service);
    let viewer = viewer();
    for request in [
        Request::DeviceRevoke { name: "win".into(), fingerprint: viewer.clone() },
        Request::DeviceAllow {
            name: "win".into(),
            fingerprint: viewer.to_uppercase(),
            what: "files".into(),
            on: false,
        },
    ] {
        assert_eq!(refused(ask(service, request).pop().unwrap()), HIDDEN_DEVICE);
    }
    let root =
        Request::DeviceAllow { name: "win".into(), fingerprint: PHONE.into(), what: "root".into(), on: true };
    assert!(!refused(ask(service, root).pop().unwrap()).is_empty());
    assert_eq!(asked_count(service), before, "nothing was sent");

    let allowed = ask_info(
        service,
        Request::DeviceAllow {
            name: "win".into(),
            fingerprint: PHONE.into(),
            what: "clipboard".into(),
            on: true,
        },
    );
    assert!(allowed.devices[0].permissions.clipboard);
    ask_info(
        service,
        Request::DeviceAllow {
            name: "win".into(),
            fingerprint: PHONE.into(),
            what: "input".into(),
            on: false,
        },
    );
    let revoked = ask_info(service, Request::DeviceRevoke { name: "win".into(), fingerprint: PHONE.into() });
    assert!(revoked.devices.is_empty());
    assert_eq!(
        sent(service, before),
        [
            format!("device {PHONE} allow clipboard on"),
            format!("device {PHONE} view-only on"),
            format!("revoke {PHONE}")
        ]
    );
    never_cut(service);
    stop(service, "win");
}

#[test]
fn switching_off_or_stopping_while_pairing_closes_it_and_a_pairing_found_open_is_closed() {
    let setup = running("pair-off");
    let service = &setup.service;
    open(service, None);
    let before = asked_count(service);
    assert_eq!(info(share(service, "win", Some(false))).state, ShareState::Off);
    assert_eq!(sent(service, before), ["pair off", "allow clipboard on", "allow files on"]);
    assert!(matches!(
        ask(
            service,
            Request::Share { name: "win".into(), on: None, pairing: Some(true), pairing_seconds: None }
        )
        .as_slice(),
        [Answer::Refused { .. }]
    ));

    // Stopped while pairing: what was read goes with the run.
    share(service, "win", Some(true));
    open(service, None);
    stop(service, "win");
    assert!(service.sessions.with("win", |session| session.pairing.is_none()));
    assert!(matches!(
        ask(service, Request::PairAccept { name: "win".into() }).as_slice(),
        [Answer::Refused { reason }] if reason == crate::requests::pairing::NOT_RUNNING
    ));

    // remoteman still has pairing open at the next start, as after a service that stopped without
    // closing it: the service closes it at its first reading, without the app asking.
    service.work.remoteman.model.lock().unwrap().pairing = true;
    let before = asked_count(service);
    start_and_run(service, "win");
    for _ in 0..100 {
        if !service.work.remoteman.model.lock().unwrap().pairing {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(sent(service, before), ["pair off"]);
    never_cut(service);
    stop(service, "win");
}

#[test]
fn a_pairing_time_outside_the_limits_is_refused() {
    let setup = running("pair-limits");
    let service = &setup.service;
    for seconds in [0, 3601] {
        assert!(refused(open_raw(service, seconds)).contains("1 to 3600"));
    }
    stop(service, "win");
}

fn open_raw(service: &Arc<Service<Pretend>>, seconds: u32) -> Answer {
    ask(
        service,
        Request::Share { name: "win".into(), on: None, pairing: Some(true), pairing_seconds: Some(seconds) },
    )
    .pop()
    .unwrap()
}

#[test]
fn no_name_or_code_reaches_a_log() {
    let setup = running("pair-logs");
    let service = &setup.service;
    open(service, None);
    arrive(service, PHONE, "Zoë's phone", "123 456");
    wait_until(service, "showed the device waiting", |info| info.pending.is_some());
    ask_info(service, Request::PairAccept { name: "win".into() });
    wait_until(service, "paired", |info| info.closed_reason.is_some());
    stop(service, "win");
    let events = fs::read_to_string(service.machines().store.events("win")).unwrap();
    let logged = service.work.logged.lock().unwrap().join("\n");
    let history: Vec<String> =
        super::network::log(service, "win").iter().map(|event| format!("{event:?}")).collect();
    for text in [events, logged, history.join("\n")] {
        assert!(!text.contains("Zo") && !text.contains("123 456") && !text.contains(PHONE), "{text}");
        assert!(!text.contains("guesthouse: ctl"), "{text}");
    }
}
