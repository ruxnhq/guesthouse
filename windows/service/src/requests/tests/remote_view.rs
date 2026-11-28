//! A machine's remote view: its switch and port, the firewall rule and the relay while it runs, and
//! what remoteman in the guest says, against the network double and a pretend remoteman.

use super::network::{FakeNet, net, problems, start_and_run, stop, wait_net};
use super::*;
use guesthouse_control::AdapterInfo;
use guesthouse_host_windows::network::{Proxy, rule_name};
use guesthouse_remote_view_core::share::{ShareInfo, ShareState};
use std::net::Ipv4Addr;

/// Guesthouse's own viewer's certificate, beside the machine's disk.
const VIEWER_CERTIFICATE: &[u8] = b"the viewer's certificate";
pub(super) const HOST: &str = "abababababababababababababababababababababababababababababababab";
pub(super) const PHONE: &str = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";
const RULE: &str = "Guesthouse remote view win 7792";

/// A Windows machine `name` that has been set up, with its viewer's identity beside its disk.
pub(super) fn windows_machine(setup: &Setup, name: &str) {
    ask(&setup.service, Request::Create { machine: new_machine(setup, name) });
    let disk = setup.service.machines().store.disk(name);
    let record = Record { installed: true, screen: Some(HOST.into()), ..Record::load(&disk).unwrap() };
    record.save(&disk).unwrap();
    let client = guesthouse_host_windows::screen::state_beside(&disk).join("client");
    fs::create_dir_all(&client).unwrap();
    fs::write(client.join("cert.der"), VIEWER_CERTIFICATE).unwrap();
}

pub(super) fn viewer() -> String {
    guesthouse_host_windows::screen::fingerprint(VIEWER_CERTIFICATE)
}

pub(super) fn share(service: &Arc<Service<Pretend>>, name: &str, on: Option<bool>) -> Answer {
    let request = Request::Share { name: name.into(), on, pairing: None, pairing_seconds: None };
    ask(service, request).pop().unwrap()
}

pub(super) fn info(answer: Answer) -> ShareInfo {
    match answer {
        Answer::Share { info } => info,
        other => panic!("{other:?}"),
    }
}

pub(super) fn shown(service: &Arc<Service<Pretend>>, name: &str) -> ShareInfo {
    info(ask(service, Request::RemoteView { name: name.into() }).pop().unwrap())
}

pub(super) fn refused(answer: Answer) -> String {
    match answer {
        Answer::Refused { reason } => reason,
        other => panic!("{other:?}"),
    }
}

fn listed(service: &Arc<Service<Pretend>>, name: &str) -> (bool, Option<u16>) {
    let machine = status(service).into_iter().find(|machine| machine.name == name).unwrap();
    (machine.remote_view, machine.remote_port)
}

fn switch_file(setup: &Setup, name: &str) -> Option<String> {
    fs::read_to_string(setup.service.machines().store.folder(name).join("remote-view.json")).ok()
}

/// What remoteman in the guest says to `status`: Guesthouse's own viewer, and `devices` besides.
fn says(service: &Arc<Service<Pretend>>, rest: &str) {
    let viewer = viewer();
    *service.work.remoteman.status.lock().unwrap() = format!(
        r#"{{"fingerprint":"{HOST}","pairing":false,"clipboard":true,"files":true,"devices":[{{"fingerprint":"{viewer}","name":"Guesthouse","connected":true}}{rest}]}}"#
    );
}

/// Waits for the keeper to have left the machine's view as `state`.
pub(super) fn wait_view(service: &Arc<Service<Pretend>>, name: &str, state: ShareState) {
    for _ in 0..400 {
        if service.machines().lock()[name].view.state == state {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("{name}'s view never became {state:?}: {:?}", service.machines().lock()[name].view);
}

/// Waits for `ready`, which looks only at what the service keeps and asks the guest nothing. A step
/// of pairing keeps each reading as it takes it, but its pairing, which is what shows a device
/// waiting, and the service's line in its log, only as it ends, so a view that listens says nothing
/// of where the keeper's first step is.
fn wait_until(service: &Arc<Service<Pretend>>, what: &str, ready: impl Fn() -> bool) {
    for _ in 0..400 {
        if ready() {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("never {what}: {:?}", service.work.remoteman.asked());
}

#[test]
fn the_switch_is_kept_in_its_own_file_with_a_port_that_stays_through_off_and_on() {
    let setup = setup("view-switch", Pretend::default());
    let service = &setup.service;
    windows_machine(&setup, "win");
    windows_machine(&setup, "other");
    assert_eq!(listed(service, "win"), (false, None));
    assert_eq!(
        info(share(service, "win", None)),
        ShareInfo { fingerprint: Some(HOST.into()), ..ShareInfo::off("win") }
    );

    let on = info(share(service, "win", Some(true)));
    assert_eq!((on.state, on.port), (ShareState::Starting, Some(7792)), "on, and shared once it runs");
    assert_eq!(listed(service, "win"), (true, Some(7792)));
    assert!(switch_file(&setup, "win").unwrap().contains("\"remote_port\": 7792"));
    assert!(
        !fs::read_to_string(service.machines().store.folder("win").join("machine.json"))
            .unwrap()
            .contains("7792")
    );

    // Another machine gets a port of its own, past one another program has.
    service.work.net.state.lock().unwrap().busy_udp = vec![7793];
    assert_eq!(info(share(service, "other", Some(true))).port, Some(7794));

    let off = info(share(service, "win", Some(false)));
    assert_eq!((off.state, off.port), (ShareState::Off, Some(7792)));
    assert_eq!(listed(service, "win"), (false, Some(7792)), "the port stays, so a device's address does");
    assert_eq!(info(share(service, "win", Some(true))).port, Some(7792));

    // A service that starts again reads it from the file.
    let again = Service::new(Store::new(setup.root.join("machines")), Pretend::default());
    assert_eq!(listed(&again, "win"), (true, Some(7792)));
    assert_eq!(listed(&again, "other"), (true, Some(7794)));
}

#[test]
fn it_is_refused_for_an_ubuntu_guest_one_without_a_network_and_one_not_set_up_and_always_turns_off() {
    let setup = setup("view-refused", Pretend::default());
    let service = &setup.service;
    windows_machine(&setup, "win");
    let disk = service.machines().store.disk("win");

    Record { installed: false, ..Record::default() }.save(&disk).unwrap();
    assert!(refused(share(service, "win", Some(true))).contains("isn't set up yet"));

    Record { guest: Guest::Linux, installed: true, ..Record::default() }.save(&disk).unwrap();
    assert_eq!(refused(share(service, "win", Some(true))), crate::requests::remote_view::NOT_WINDOWS);

    Record { installed: true, ..Record::default() }.save(&disk).unwrap();
    change(service, MachineSettings { network: false, ..hardware(8192, Card::Default) });
    assert!(refused(share(service, "win", Some(true))).contains("has no network"));
    assert_eq!(listed(service, "win"), (false, None), "nothing was kept");
    assert!(switch_file(&setup, "win").is_none());

    // Off is never refused, as a machine turned on before its network went off must be turnable off.
    assert_eq!(info(share(service, "win", Some(false))).state, ShareState::Off);
    let pairing = Request::Share { name: "win".into(), on: None, pairing: Some(true), pairing_seconds: None };
    assert_eq!(refused(ask(service, pairing).pop().unwrap()), crate::requests::pairing::NOT_ON);
    assert!(matches!(share(service, "nothing", Some(true)), Answer::Refused { .. }));
}

#[test]
fn a_running_machines_rule_is_made_before_its_relay_and_both_go_before_it_counts_as_stopped() {
    let setup = setup("view-running", Pretend::default());
    let service = &setup.service;
    windows_machine(&setup, "win");
    share(service, "win", Some(true));
    start_and_run(service, "win");
    wait_view(service, "win", ShareState::Listening);
    assert_eq!(
        net(service).calls,
        [format!("add_rule {RULE} 7792 udp"), "open_view 7792 172.29.1.2:7791".to_owned()]
    );
    assert_eq!(shown(service, "win").state, ShareState::Listening);

    stop(service, "win");
    {
        let net = net(service);
        assert_eq!(net.calls[2..], ["close_view 7792".to_owned(), format!("delete_rule {RULE}")]);
        assert!(net.rules.is_empty() && net.views.is_empty());
    }
    assert_eq!(shown(service, "win").state, ShareState::Starting, "on, for its next start");
    assert!(problems(service, "win").is_empty());
}

#[test]
fn switching_it_on_and_off_while_the_machine_runs_opens_and_closes_it_and_never_cuts_the_guests_viewer() {
    let setup = setup("view-live", Pretend::default());
    let service = &setup.service;
    windows_machine(&setup, "win");
    says(service, "");
    start_and_run(service, "win");
    thread::sleep(Duration::from_millis(300));
    assert!(net(service).calls.is_empty(), "nothing while it's off");

    share(service, "win", Some(true));
    wait_view(service, "win", ShareState::Listening);
    assert_eq!(net(service).views, [7792]);

    let off = info(share(service, "win", Some(false)));
    assert_eq!(off.state, ShareState::Off);
    wait_net(service, |net| net.views.is_empty() && net.rules.is_empty());
    assert_eq!(net(service).calls[2..], ["close_view 7792".to_owned(), format!("delete_rule {RULE}")]);

    let again = info(share(service, "win", Some(true)));
    assert_ne!(again.state, ShareState::Off, "on, before its keeper has looked, is starting");
    wait_view(service, "win", ShareState::Listening);
    stop(service, "win");
    for asked in service.work.remoteman.asked() {
        assert!(!asked.starts_with("sharing") && asked != "drop", "{asked}");
    }
}

#[test]
fn a_bridged_machines_relay_leads_to_the_address_its_guest_has_on_this_pcs_network() {
    let fake = FakeNet::with(|net| {
        net.bridging = Bridging {
            available: true,
            reason: None,
            adapters: vec![AdapterInfo {
                name: "Ethernet".into(),
                description: "Ethernet".into(),
                wifi: false,
            }],
            hyper_v: HyperV::On,
        }
    });
    let setup = setup("view-bridged", Pretend { net: fake, ..Pretend::default() });
    let service = &setup.service;
    windows_machine(&setup, "win");
    assert_eq!(
        change(
            service,
            MachineSettings {
                network_mode: NetworkMode::Bridged { adapter: "Ethernet".into() },
                ..hardware(8192, Card::Default)
            },
        ),
        [Answer::Ok]
    );
    share(service, "win", Some(true));
    service.work.silent.store(true, Ordering::SeqCst);
    start_and_run(service, "win");
    service.machines().tell("win", Event::Guest("guesthouse: address: 192.168.1.40".into()));
    wait_view(service, "win", ShareState::Listening);
    assert_eq!(net(service).calls[1], "open_view 7792 192.168.1.40:7791");
    assert_eq!(
        service.work.booted.lock().unwrap()[0].host_addresses,
        Some(vec![THIS_PC]),
        "the guest lets in this PC"
    );

    // This PC takes another address on that network while the machine runs: the guest won't let it
    // in until its next start, which the view says, and back on the old one it opens again.
    *service.work.source.lock().unwrap() = Some(Ipv4Addr::new(192, 168, 1, 77));
    wait_view(service, "win", ShareState::Failed);
    assert!(net(service).views.is_empty());
    let shown = shown(service, "win");
    assert!(shown.reason.unwrap().contains("192.168.1.77"));
    *service.work.source.lock().unwrap() = Some(THIS_PC);
    wait_view(service, "win", ShareState::Listening);
    stop(service, "win");
}

#[test]
fn the_relay_follows_the_guests_address_and_waits_for_this_runs() {
    let setup = setup("view-address", Pretend::default());
    let service = &setup.service;
    windows_machine(&setup, "win");
    share(service, "win", Some(true));
    start_and_run(service, "win");
    wait_view(service, "win", ShareState::Listening);
    service.machines().tell("win", Event::Report("address: 172.29.1.9".into()));
    wait_net(service, |net| net.calls.len() == 6);
    assert_eq!(
        net(service).calls[2..],
        [
            "close_view 7792".to_owned(),
            format!("delete_rule {RULE}"),
            format!("add_rule {RULE} 7792 udp"),
            "open_view 7792 172.29.1.9:7791".to_owned()
        ]
    );
    stop(service, "win");

    // The last run's address may be anyone's by now.
    service.work.silent.store(true, Ordering::SeqCst);
    start_and_run(service, "win");
    thread::sleep(Duration::from_millis(300));
    assert!(net(service).views.is_empty());
    assert_eq!(shown(service, "win").state, ShareState::Starting);
    service.machines().tell("win", Event::Guest("guesthouse: address: 172.29.1.9".into()));
    wait_view(service, "win", ShareState::Listening);
    stop(service, "win");
}

#[test]
fn the_relay_leads_only_to_an_address_where_the_machines_own_card_answers() {
    let work = Pretend { net: FakeNet::with(|net| net.elsewhere = vec![THIS_PC]), ..Pretend::default() };
    let setup = setup("view-card", work);
    let service = &setup.service;
    windows_machine(&setup, "win");
    share(service, "win", Some(true));
    start_and_run(service, "win");
    wait_view(service, "win", ShareState::Listening);
    service.machines().tell("win", Event::Report(format!("address: {THIS_PC}")));
    wait_view(service, "win", ShareState::Failed);
    assert!(net(service).views.is_empty());
    assert!(!net(service).calls.iter().any(|call| call.contains("192.168.1.20")), "{:?}", net(service).calls);
    let reason = shown(service, "win").reason.unwrap_or_default();
    assert!(reason.contains("not the machine's"), "{reason}");
    stop(service, "win");
}

#[test]
fn a_relay_that_cant_listen_fails_once_and_takes_its_rule_away() {
    let setup = setup(
        "view-fails",
        Pretend { net: FakeNet::with(|net| net.refuse_view = true), ..Pretend::default() },
    );
    let service = &setup.service;
    windows_machine(&setup, "win");
    share(service, "win", Some(true));
    start_and_run(service, "win");
    wait_view(service, "win", ShareState::Failed);
    thread::sleep(Duration::from_millis(300));
    assert_eq!(
        net(service).calls,
        [
            format!("add_rule {RULE} 7792 udp"),
            "open_view 7792 172.29.1.2:7791".to_owned(),
            format!("delete_rule {RULE}")
        ],
        "tried once"
    );
    let failed = shown(service, "win");
    assert_eq!(failed.state, ShareState::Failed);
    assert!(failed.reason.unwrap().contains("the port is taken"));
    assert_eq!(problems(service, "win").len(), 1);
    stop(service, "win");
}

#[test]
fn a_machine_whose_network_went_off_says_why_it_isnt_shared() {
    let setup = setup("view-no-network", Pretend::default());
    let service = &setup.service;
    windows_machine(&setup, "win");
    share(service, "win", Some(true));
    change(service, MachineSettings { network: false, ..hardware(8192, Card::Default) });
    start_and_run(service, "win");
    wait_view(service, "win", ShareState::Failed);
    assert!(shown(service, "win").reason.unwrap().contains("has no network"));
    assert!(net(service).calls.is_empty());
    stop(service, "win");
}

#[test]
fn rules_left_behind_are_swept_at_start_and_neither_sweep_touches_the_others() {
    let entry = |port| Proxy {
        listen_address: Ipv4Addr::UNSPECIFIED,
        listen_port: port,
        connect_address: Ipv4Addr::new(172, 29, 1, 2),
        connect_port: 80,
    };
    let work = Pretend {
        net: FakeNet::with(|net| {
            // Someone else's forward on the remote view's port, and a forward of Guesthouse's.
            net.proxies = vec![entry(7792), entry(8080)];
            net.rules = vec![RULE.into(), rule_name("win", 8080), "Remote Desktop (TCP-In)".into()];
        }),
        ..Pretend::default()
    };
    let setup = setup("view-leftover", work);
    wait_net(&setup.service, |net| net.rules.len() == 1);
    let net = net(&setup.service);
    assert_eq!(net.rules, ["Remote Desktop (TCP-In)"]);
    assert_eq!(net.proxies, [entry(7792)], "the remote view's sweep takes no portproxy entry");
    assert!(!net.calls.contains(&"delete_proxy 7792".to_owned()));
}

#[test]
fn what_remoteman_says_is_read_as_it_starts_and_when_asked_with_guesthouses_own_viewer_kept_out_of_sight() {
    let setup = setup("view-status", Pretend::default());
    let service = &setup.service;
    windows_machine(&setup, "win");
    share(service, "win", Some(true));
    says(service, "");
    start_and_run(service, "win");
    wait_view(service, "win", ShareState::Listening);
    // Read once by the service as the guest first answers, which closes a pairing found open, and
    // then only when the app asks, while nothing is pairing. The service's line goes in its log as
    // that first step ends.
    wait_until(service, "the guest answered", || !service.work.logged.lock().unwrap().is_empty());

    let idle = shown(service, "win");
    assert_eq!((idle.state, idle.port, idle.reason.as_deref()), (ShareState::Listening, Some(7792), None));
    assert_eq!(idle.fingerprint.as_deref(), Some(HOST));
    assert!(idle.devices.is_empty(), "{:?}", idle.devices);
    assert_eq!((idle.viewer, idle.pending), (None, None), "the window onto it is Guesthouse's own");
    assert_eq!(service.work.remoteman.asked(), ["status"], "a reading writes one request");
    let logged = service.work.logged.lock().unwrap().clone();
    assert_eq!(logged.len(), 1, "{logged:?}");
    assert!(logged[0].starts_with("win: remoteman in the guest answered over the serial port, in "));
    assert!(
        logged[0].ends_with(" ms: 1 devices trusted, 1 of them Guesthouse's own viewer."),
        "{}",
        logged[0]
    );
    // Asked again at once, the reading is still new.
    shown(service, "win");
    assert_eq!(service.work.remoteman.asked(), ["status"]);

    // A device that has the screen now, which Guesthouse's own window had.
    let viewer = viewer();
    *service.work.remoteman.status.lock().unwrap() = format!(
        r#"{{"fingerprint":"{HOST}","pairing":false,"client":{{"name":"Zoë's phone","placement":"only"}},"devices":[{{"fingerprint":"{viewer}","name":"Guesthouse"}},{{"fingerprint":"{PHONE}","name":"Zoë's phone","connected":true}}]}}"#
    );
    thread::sleep(Duration::from_millis(2_100));
    let viewing = shown(service, "win");
    assert_eq!(viewing.viewer.as_deref(), Some("Zoë's phone"));
    assert_eq!(viewing.viewing_only(), Some("Zoë's phone"));
    assert_eq!(viewing.devices.len(), 1);
    assert_eq!(viewing.devices[0].fingerprint, PHONE);
    stop(service, "win");
}

#[test]
fn a_device_found_waiting_is_shown_and_nothing_it_said_reaches_a_log() {
    let setup = setup("view-pending", Pretend::default());
    let service = &setup.service;
    windows_machine(&setup, "win");
    share(service, "win", Some(true));
    says(service, "");
    let pending = format!(
        r#""pending":{{"name":"Zoë's phone","fingerprint":"{PHONE}","code":"123 456","seconds":50}},"devices""#
    );
    let status = service.work.remoteman.status.lock().unwrap().replace(r#""devices""#, &pending);
    *service.work.remoteman.status.lock().unwrap() = status;
    start_and_run(service, "win");
    wait_view(service, "win", ShareState::Listening);
    wait_until(service, "a device shown waiting", || {
        service.sessions.with("win", |session| {
            session.pairing.as_ref().is_some_and(|pairing| pairing.view().pending.is_some())
        })
    });

    let shown = shown(service, "win");
    let waiting =
        shown.pending.clone().unwrap_or_else(|| panic!("{shown:?} {:?}", service.work.remoteman.asked()));
    assert_eq!((waiting.name.as_str(), waiting.code.as_str()), ("Zoë's phone", "123 456"));
    assert_eq!(
        service.work.remoteman.asked(),
        ["status", "allow clipboard off", "allow files off", "status"],
        "met with the host-wide switches held off, as on Linux"
    );

    // Switched off, the device is turned away and the switches come back, and nothing cuts
    // Guesthouse's own window. The guest has no device waiting once it's declined.
    says(service, "");
    assert_eq!(info(share(service, "win", Some(false))).state, ShareState::Off);
    let asked = service.work.remoteman.asked();
    let decline = format!("pair decline {PHONE}");
    assert_eq!(asked[4..], [decline.as_str(), "allow clipboard on", "allow files on", "status"]);
    for line in &asked {
        assert!(!line.starts_with("sharing") && line != "drop" && !line.contains(&viewer()), "{line}");
    }
    stop(service, "win");

    for line in service.work.logged.lock().unwrap().iter() {
        assert!(!line.contains("Zo") && !line.contains("123 456") && !line.contains(PHONE), "{line}");
    }
    let events = fs::read_to_string(service.machines().store.events("win")).unwrap();
    for history in super::network::log(service, "win") {
        let text = format!("{history:?}");
        assert!(!text.contains("Zo") && !text.contains("123 456") && !text.contains(PHONE), "{text}");
    }
    assert!(!events.contains("Zo") && !events.contains("123 456") && !events.contains(PHONE), "{events}");
    assert!(!events.contains("guesthouse: ctl"));
}

#[test]
fn a_guest_that_doesnt_answer_is_said_and_the_app_is_answered_anyway() {
    let setup = setup("view-silent", Pretend::default());
    let service = &setup.service;
    windows_machine(&setup, "win");
    share(service, "win", Some(true));
    start_and_run(service, "win");
    wait_view(service, "win", ShareState::Listening);
    // A guest whose remoteman isn't up: what ctl says isn't a status.
    *service.work.remoteman.status.lock().unwrap() = "Error: no host is answering".into();
    thread::sleep(Duration::from_millis(2_100));
    let shown = shown(service, "win");
    assert_eq!(shown.state, ShareState::Listening);
    assert!(shown.reason.is_some());
    stop(service, "win");
    // Once stopped, nothing more is asked.
    let before = service.work.remoteman.asked().len();
    assert_eq!(info(share(service, "win", None)).state, ShareState::Starting);
    assert_eq!(service.work.remoteman.asked().len(), before);
}

#[test]
fn a_machine_whose_name_cant_be_announced_still_listens_and_says_why() {
    let setup = setup(
        "view-unlisted",
        Pretend { net: FakeNet::with(|net| net.name_taken = true), ..Pretend::default() },
    );
    let service = &setup.service;
    windows_machine(&setup, "win");
    share(service, "win", Some(true));
    start_and_run(service, "win");
    wait_view(service, "win", ShareState::Listening);
    let shown = shown(service, "win");
    assert_eq!(shown.state, ShareState::Listening);
    let reason = shown.reason.unwrap();
    assert!(reason.contains("the name is taken") && reason.contains("still reach it"), "{reason}");
    assert!(service.work.logged.lock().unwrap().iter().any(|line| line.contains("the name is taken")));
    assert!(problems(service, "win").is_empty(), "a note, not a problem");
    stop(service, "win");
    assert!(net(service).views.is_empty());
}
