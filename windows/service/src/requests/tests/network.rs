//! Port forwards and bridging, against a double for what `netsh` and Windows would do.

use super::*;
use guesthouse_control::{AdapterInfo, Forward};
use guesthouse_host_windows::hyper_v::Enabled;
use guesthouse_host_windows::network::{Protocol, Proxy, RULE_PREFIX, bridging_from};
use std::net::Ipv4Addr;

/// What the double's Windows holds, and what was asked of it, in order.
#[derive(Default)]
pub(super) struct NetState {
    pub proxies: Vec<Proxy>,
    pub rules: Vec<String>,
    /// Ports another program listens on.
    pub busy: Vec<u16>,
    pub ip_helper_down: bool,
    /// Ports whose forwards `netsh` refuses to make.
    pub refuse_proxy: Vec<u16>,
    /// The firewall's list says nothing this build can read, though the rules are there.
    pub hide_rules: bool,
    pub bridging: Bridging,
    pub calls: Vec<String>,
    /// UDP ports another program listens on.
    pub busy_udp: Vec<u16>,
    /// The ports a remote view's relay listens on now.
    pub views: Vec<u16>,
    /// A remote view's relay can't listen.
    pub refuse_view: bool,
    /// The machine's name is announced on the network already, by something else on this PC.
    pub name_taken: bool,
    /// How turning Hyper-V on ends: on, and waiting for a restart, unless it says.
    pub enable: Option<Enabled>,
    /// Turning Hyper-V on waits, part way, while this is set.
    pub enable_held: bool,
    /// Addresses where another card than the guest's answers, as this PC's own or another device's.
    pub elsewhere: Vec<Ipv4Addr>,
}

/// The card a pretend machine's record keeps, which answers at every address but those `elsewhere`.
pub(super) const GUEST_CARD: &str = "00-15-5D-00-00-01";

#[derive(Default)]
pub(super) struct FakeNet {
    pub state: Arc<Mutex<NetState>>,
}

impl FakeNet {
    pub fn with(change: impl FnOnce(&mut NetState)) -> Self {
        let net = FakeNet::default();
        change(&mut net.state.lock().unwrap());
        net
    }
}

impl Network for FakeNet {
    fn proxies(&self) -> Result<Vec<Proxy>, String> {
        Ok(self.state.lock().unwrap().proxies.clone())
    }

    fn add_proxy(&self, listen: u16, address: Ipv4Addr, connect: u16) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        state.calls.push(format!("add_proxy {listen} {address} {connect}"));
        if state.refuse_proxy.contains(&listen) {
            return Err("netsh said no".into());
        }
        state.proxies.push(Proxy {
            listen_address: Ipv4Addr::UNSPECIFIED,
            listen_port: listen,
            connect_address: address,
            connect_port: connect,
        });
        Ok(())
    }

    fn delete_proxy(&self, listen: u16) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        state.calls.push(format!("delete_proxy {listen}"));
        state.proxies.retain(|proxy| proxy.listen_port != listen);
        Ok(())
    }

    fn rules(&self) -> Result<Vec<String>, String> {
        let state = self.state.lock().unwrap();
        let shown = state.rules.iter().filter(|rule| rule.starts_with(RULE_PREFIX) && !state.hide_rules);
        Ok(shown.cloned().collect())
    }

    fn add_rule(&self, name: &str, port: u16, protocol: Protocol) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        state.calls.push(match protocol {
            Protocol::Tcp => format!("add_rule {name} {port}"),
            Protocol::Udp => format!("add_rule {name} {port} udp"),
        });
        state.rules.push(name.to_owned());
        Ok(())
    }

    fn delete_rule(&self, name: &str) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        state.calls.push(format!("delete_rule {name}"));
        let before = state.rules.len();
        state.rules.retain(|rule| rule != name);
        if state.rules.len() == before {
            Err("No rules match the specified criteria.".into())
        } else {
            Ok(())
        }
    }

    fn port_free(&self, port: u16) -> bool {
        let state = self.state.lock().unwrap();
        !state.busy.contains(&port) && !state.proxies.iter().any(|proxy| proxy.listen_port == port)
    }

    fn ip_helper_running(&self) -> bool {
        !self.state.lock().unwrap().ip_helper_down
    }

    fn card_at(&self, address: Ipv4Addr) -> Result<String, String> {
        let elsewhere = self.state.lock().unwrap().elsewhere.contains(&address);
        Ok(if elsewhere { "00-15-5D-99-99-99".into() } else { GUEST_CARD.into() })
    }

    fn bridging(&self) -> Bridging {
        self.state.lock().unwrap().bridging.clone()
    }

    /// Says 30% and waits while it's held, then 100%, and ends as `enable` says. Once on, Windows
    /// says it waits for a restart, as DISM's would.
    fn enable_hyper_v(&self, progress: &mut dyn FnMut(u8)) -> Enabled {
        self.state.lock().unwrap().calls.push("enable_hyper_v".into());
        progress(30);
        while self.state.lock().unwrap().enable_held {
            thread::sleep(Duration::from_millis(5));
        }
        progress(100);
        let mut state = self.state.lock().unwrap();
        let enabled = state.enable.clone().unwrap_or(Enabled::Done);
        if enabled == Enabled::Done {
            state.bridging = bridging_from(HyperV::RestartPending, Ok(Vec::new()));
        }
        enabled
    }
}

fn forward(host_port: u16, guest_port: u16) -> Forward {
    Forward { host_port, guest_port }
}

fn with_forwards(forwards: &[Forward]) -> MachineSettings {
    MachineSettings { forwards: forwards.to_vec(), ..hardware(8192, Card::Default) }
}

pub(super) fn net(service: &Arc<Service<Pretend>>) -> std::sync::MutexGuard<'_, NetState> {
    service.work.net.state.lock().unwrap()
}

/// Waits for the double's Windows to be as `ready` says, as the forwards are made on a thread of
/// their own.
pub(super) fn wait_net(service: &Arc<Service<Pretend>>, ready: impl Fn(&NetState) -> bool) {
    for _ in 0..400 {
        if ready(&net(service)) {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("Windows never came to be as expected: {:?}", net(service).calls);
}

pub(super) fn log(service: &Arc<Service<Pretend>>, name: &str) -> Vec<Event> {
    service.machines().watch(name).unwrap().0
}

pub(super) fn problems(service: &Arc<Service<Pretend>>, name: &str) -> Vec<String> {
    log(service, name)
        .into_iter()
        .filter_map(|event| match event {
            Event::Problem(text) => Some(text),
            _ => None,
        })
        .collect()
}

pub(super) fn start_and_run(service: &Arc<Service<Pretend>>, name: &str) {
    assert_eq!(ask(service, Request::Start { name: name.into() }), [Answer::Ok]);
    wait_for(service, name, State::Running);
}

pub(super) fn stop(service: &Arc<Service<Pretend>>, name: &str) {
    ask(service, Request::Stop { name: name.into() });
    wait_for(service, name, State::Off);
}

#[test]
fn forwards_are_made_once_the_guest_says_its_address_and_gone_before_the_machine_counts_as_stopped() {
    let setup = setup("forwards", Pretend::default());
    let service = &setup.service;
    ask(service, Request::Create { machine: new_machine(&setup, "win") });
    let wanted = with_forwards(&[forward(8080, 80), forward(2222, 22)]);
    assert_eq!(change(service, wanted), [Answer::Ok]);
    assert_eq!(status(service)[0].forwards, [forward(8080, 80), forward(2222, 22)]);

    start_and_run(service, "win");
    wait_net(service, |net| net.proxies.len() == 2);
    {
        let net = net(service);
        assert_eq!(net.rules, ["Guesthouse win 8080", "Guesthouse win 2222"]);
        let guest = Ipv4Addr::new(172, 29, 1, 2);
        assert_eq!(net.proxies[0].connect_address, guest);
        assert_eq!((net.proxies[0].listen_port, net.proxies[0].connect_port), (8080, 80));
        assert_eq!((net.proxies[1].listen_port, net.proxies[1].connect_port), (2222, 22));
        // A rule is made before its entry, so a rule is there whenever an entry is.
        assert_eq!(
            net.calls,
            [
                "add_rule Guesthouse win 8080 8080",
                "add_proxy 8080 172.29.1.2 80",
                "add_rule Guesthouse win 2222 2222",
                "add_proxy 2222 172.29.1.2 22"
            ]
        );
    }

    stop(service, "win");
    {
        // Nothing is left by the time the machine is stopped, and each entry went before its rule.
        let net = net(service);
        assert!(net.proxies.is_empty() && net.rules.is_empty());
        assert_eq!(
            net.calls[4..],
            [
                "delete_proxy 8080",
                "delete_rule Guesthouse win 8080",
                "delete_proxy 2222",
                "delete_rule Guesthouse win 2222"
            ]
        );
    }
    assert!(problems(service, "win").is_empty());
}

#[test]
fn forwards_follow_the_guests_address_when_it_changes() {
    let setup = setup("forwards-move", Pretend::default());
    let service = &setup.service;
    ask(service, Request::Create { machine: new_machine(&setup, "win") });
    change(service, with_forwards(&[forward(8080, 80)]));
    start_and_run(service, "win");
    wait_net(service, |net| net.proxies.len() == 1);

    service.machines().tell("win", Event::Report("address: 172.29.1.9".into()));
    wait_net(service, |net| {
        net.proxies.len() == 1 && net.proxies[0].connect_address == Ipv4Addr::new(172, 29, 1, 9)
    });
    assert_eq!(net(service).rules, ["Guesthouse win 8080"], "one rule, as one forward");
    stop(service, "win");
    assert!(net(service).proxies.is_empty());
}

#[test]
fn a_forward_leads_only_to_an_address_where_the_machines_own_card_answers() {
    let device = Ipv4Addr::new(10, 0, 0, 5);
    let work =
        Pretend { net: FakeNet::with(|net| net.elsewhere = vec![THIS_PC, device]), ..Pretend::default() };
    let setup = setup("forwards-card", work);
    let service = &setup.service;
    ask(service, Request::Create { machine: new_machine(&setup, "win") });
    change(service, with_forwards(&[forward(8445, 445)]));
    start_and_run(service, "win");
    wait_net(service, |net| net.proxies.len() == 1);

    // This PC's own address, then another device's: neither is the guest's, whatever it says.
    for said in [THIS_PC, device] {
        service.machines().tell("win", Event::Report(format!("address: {said}")));
        wait_net(service, |net| net.proxies.is_empty() && net.rules.is_empty());
        thread::sleep(Duration::from_millis(300));
        assert!(net(service).proxies.is_empty(), "nothing leads to {said}");
        let said_why = problems(service, "win").iter().any(|problem| {
            problem.starts_with("No port forwards are made") && problem.contains(&said.to_string())
        });
        assert!(said_why, "{:?}", problems(service, "win"));
    }
    assert!(
        !net(service).calls.iter().any(|call| call.contains("192.168.1.20") || call.contains("10.0.0.5"))
    );

    service.machines().tell("win", Event::Report("address: 172.29.1.9".into()));
    wait_net(service, |net| {
        net.proxies.len() == 1 && net.proxies[0].connect_address == Ipv4Addr::new(172, 29, 1, 9)
    });
    stop(service, "win");
}

#[test]
fn the_last_runs_address_is_not_forwarded_to_until_the_guest_says_it_again() {
    let setup = setup("forwards-stale", Pretend::default());
    let service = &setup.service;
    ask(service, Request::Create { machine: new_machine(&setup, "win") });
    change(service, with_forwards(&[forward(8080, 80)]));
    start_and_run(service, "win");
    wait_net(service, |net| net.proxies.len() == 1);
    stop(service, "win");
    assert_eq!(status(service)[0].address.as_deref(), Some("172.29.1.2"), "kept from the run");

    // A guest that's slow to say its address leaves the old one, which may be anyone's now.
    service.work.silent.store(true, Ordering::SeqCst);
    start_and_run(service, "win");
    thread::sleep(Duration::from_millis(400));
    assert!(net(service).proxies.is_empty());
    // And when it says the very same address, which a guest on the same network often does, that is
    // news too.
    service.machines().tell("win", Event::Guest("guesthouse: address: 172.29.1.2".into()));
    wait_net(service, |net| net.proxies.len() == 1);
    stop(service, "win");
}

#[test]
fn a_forward_that_cant_be_made_is_a_line_in_the_log_and_the_machine_still_runs() {
    let work = Pretend {
        net: FakeNet::with(|net| {
            net.busy = vec![81];
            // Someone else's forward, which has no rule of Guesthouse's.
            net.proxies.push(Proxy {
                listen_address: Ipv4Addr::UNSPECIFIED,
                listen_port: 82,
                connect_address: Ipv4Addr::new(10, 0, 0, 5),
                connect_port: 22,
            });
            net.refuse_proxy = vec![83];
            net.ip_helper_down = true;
        }),
        ..Pretend::default()
    };
    let setup = setup("forwards-refused", work);
    let service = &setup.service;
    ask(service, Request::Create { machine: new_machine(&setup, "win") });
    let wanted = with_forwards(&[forward(80, 80), forward(81, 81), forward(82, 82), forward(83, 83)]);
    assert_eq!(change(service, wanted), [Answer::Ok]);
    start_and_run(service, "win");
    wait_net(service, |net| net.calls.len() >= 5);
    thread::sleep(Duration::from_millis(100));
    {
        let net = net(service);
        assert_eq!(
            net.rules,
            ["Guesthouse win 80"],
            "the one that could be made; 83's rule went with its entry"
        );
        assert!(net.proxies.iter().any(|proxy| proxy.listen_port == 80));
        assert!(net.proxies.iter().any(|proxy| proxy.listen_port == 82), "someone else's is untouched");
    }
    let said = problems(service, "win").join("\n");
    assert!(said.contains("iphlpsvc"), "{said}");
    assert!(said.contains("Port 81") && said.contains("another program"), "{said}");
    assert!(said.contains("Port 82") && said.contains("something else forwards"), "{said}");
    assert!(said.contains("Port 83") && said.contains("couldn't be made"), "{said}");
    assert_eq!(service.machines().lock()["win"].state, State::Running);

    stop(service, "win");
    let net = net(service);
    assert_eq!(net.proxies.len(), 1, "only someone else's is left");
    assert_eq!(net.proxies[0].listen_port, 82);
    assert!(net.rules.is_empty());
}

#[test]
fn forwards_left_behind_are_taken_away_when_the_service_starts_and_others_are_not_touched() {
    let leftover = |port, connect| Proxy {
        listen_address: Ipv4Addr::UNSPECIFIED,
        listen_port: port,
        connect_address: Ipv4Addr::new(172, 29, 1, connect),
        connect_port: 80,
    };
    let work = Pretend {
        net: FakeNet::with(|net| {
            net.proxies = vec![leftover(8080, 2), leftover(9000, 7)];
            net.rules = vec!["Guesthouse win 8080".into(), "Remote Desktop (TCP-In)".into()];
        }),
        ..Pretend::default()
    };
    let setup = setup("forwards-leftover", work);
    // It's done on a thread of its own, so that the service is running while it is.
    wait_net(&setup.service, |net| net.rules.len() == 1);
    let net = net(&setup.service);
    assert_eq!(net.proxies, [leftover(9000, 7)], "a forward of someone else's, as WSL's setups make");
    assert_eq!(net.rules, ["Remote Desktop (TCP-In)"]);
}

#[test]
fn a_stored_machines_forwards_are_found_even_when_the_rules_list_names_none() {
    let setup_dir = std::env::temp_dir().join(format!("gh-requests-forwards-stored-{}", std::process::id()));
    let _ = fs::remove_dir_all(&setup_dir);
    let store = Store::new(setup_dir.join("machines"));
    fs::create_dir_all(store.root()).unwrap();
    store.make("win").unwrap();
    let mut settings = Settings::basic(8192, 1, Card::Default);
    settings.address = Some("172.29.1.2".into());
    settings.forwards = vec![forward(8080, 80)];
    store.save("win", &settings).unwrap();
    let ours = Proxy {
        listen_address: Ipv4Addr::UNSPECIFIED,
        listen_port: 8080,
        connect_address: Ipv4Addr::new(172, 29, 1, 2),
        connect_port: 80,
    };
    // Someone else's entry on the same port that leads elsewhere stays.
    let other = Proxy { connect_address: Ipv4Addr::new(10, 0, 0, 1), listen_port: 9090, ..ours };
    let work = Pretend {
        net: FakeNet::with(|net| {
            net.proxies = vec![ours, other];
            net.rules = vec!["Guesthouse win 8080".into()];
            net.hide_rules = true;
        }),
        ..Pretend::default()
    };
    let service = Service::new(store, work);
    wait_net(&service, |net| net.rules.is_empty());
    assert_eq!(net(&service).proxies, [other]);
    let _ = fs::remove_dir_all(&setup_dir);
}

#[test]
fn forwards_are_kept_to_the_services_rules() {
    let setup = setup("forwards-rules", Pretend::default());
    let service = &setup.service;
    ask(service, Request::Create { machine: new_machine(&setup, "win") });
    ask(service, Request::Create { machine: new_machine(&setup, "ubuntu") });
    let refused = |forwards: &[Forward]| match change(service, with_forwards(forwards)).as_slice() {
        [Answer::Refused { reason }] => reason.clone(),
        other => panic!("{other:?}"),
    };
    assert!(refused(&[forward(0, 80)]).contains("1 to 65535"));
    assert!(refused(&[forward(80, 0)]).contains("1 to 65535"));
    assert!(refused(&[forward(80, 80), forward(80, 81)]).contains("twice"));
    let many: Vec<Forward> = (1..=17).map(|port| forward(port, port)).collect();
    assert!(refused(&many).contains("16"));
    assert_eq!(change(service, with_forwards(&many[..16])), [Answer::Ok]);

    // No port of this PC leads to two machines, whichever machine asks.
    let other =
        ask(service, Request::Change { name: "ubuntu".into(), settings: with_forwards(&[forward(9, 9)]) });
    assert!(
        matches!(other.as_slice(), [Answer::Refused { reason }] if reason.contains("port 9 is forwarded to win"))
    );
    let free =
        ask(service, Request::Change { name: "ubuntu".into(), settings: with_forwards(&[forward(2000, 9)]) });
    assert_eq!(free, [Answer::Ok]);
    assert!(
        service
            .machines()
            .store
            .load()
            .iter()
            .any(|(name, settings)| name == "win" && settings.forwards.len() == 16)
    );
}

fn ethernet_and_wifi() -> Bridging {
    Bridging {
        available: true,
        reason: None,
        adapters: vec![
            AdapterInfo { name: "Ethernet".into(), description: "Realtek".into(), wifi: false },
            AdapterInfo { name: "Wi-Fi".into(), description: "Intel".into(), wifi: true },
        ],
        hyper_v: HyperV::On,
    }
}

fn bridged(adapter: &str) -> MachineSettings {
    MachineSettings {
        network_mode: NetworkMode::Bridged { adapter: adapter.into() },
        ..hardware(8192, Card::Default)
    }
}

#[test]
fn bridging_is_offered_in_the_status_and_a_machine_is_bridged_onto_an_adapter_it_lists() {
    let work = Pretend { net: FakeNet::with(|net| net.bridging = ethernet_and_wifi()), ..Pretend::default() };
    let setup = setup("bridged", work);
    let service = &setup.service;
    ask(service, Request::Create { machine: new_machine(&setup, "win") });
    match ask(service, Request::Status).pop() {
        Some(Answer::Status { bridging, .. }) => assert_eq!(bridging, ethernet_and_wifi()),
        other => panic!("{other:?}"),
    }
    assert_eq!(status(service)[0].network_mode, NetworkMode::Shared);
    // Run once, so there's an address of the shared network to forget.
    start_and_run(service, "win");
    stop(service, "win");
    assert_eq!(status(service)[0].address.as_deref(), Some("172.29.1.2"));

    let refused = ask(service, Request::Change { name: "win".into(), settings: bridged("Ethernet 9") });
    assert!(matches!(refused.as_slice(), [Answer::Refused { reason }] if reason.contains("Ethernet 9")));
    assert_eq!(
        ask(service, Request::Change { name: "win".into(), settings: bridged("Ethernet") }),
        [Answer::Ok]
    );
    assert_eq!(status(service)[0].network_mode, NetworkMode::Bridged { adapter: "Ethernet".into() });
    assert_eq!(
        service.machines().store.load()[0].1.network_mode,
        NetworkMode::Bridged { adapter: "Ethernet".into() }
    );

    // The address of the shared network leads nowhere on the router's.
    assert_eq!(status(service)[0].address, None);

    // It boots bridged, with no forwards made, as it has an address of its own on the network.
    let both = MachineSettings { forwards: vec![forward(8080, 80)], ..bridged("Ethernet") };
    assert_eq!(ask(service, Request::Change { name: "win".into(), settings: both }), [Answer::Ok]);
    start_and_run(service, "win");
    assert_eq!(service.work.booted.lock().unwrap()[1].bridge.as_deref(), Some("Ethernet"));
    thread::sleep(Duration::from_millis(300));
    assert!(net(service).calls.is_empty(), "{:?}", net(service).calls);
    assert_eq!(
        status(service)[0].address.as_deref(),
        Some("172.29.1.2"),
        "this run's, on the router's network"
    );
    assert!(
        log(service, "win").iter().any(
            |event| matches!(event, Event::Note(text) if text.contains("aren't made while it's bridged"))
        )
    );
    stop(service, "win");

    // Back on the shared network it boots with no adapter.
    assert_eq!(change(service, hardware(8192, Card::Default)), [Answer::Ok]);
    start_and_run(service, "win");
    assert_eq!(service.work.booted.lock().unwrap()[2].bridge, None);
    stop(service, "win");
}

#[test]
fn bridging_is_refused_where_hyper_v_is_missing_and_a_vanished_adapter_doesnt_stop_other_changes() {
    let work = Pretend {
        net: FakeNet::with(|net| {
            net.bridging = Bridging {
                available: false,
                reason: Some("Bridging needs Hyper-V.".into()),
                adapters: Vec::new(),
                hyper_v: HyperV::Off,
            }
        }),
        ..Pretend::default()
    };
    let setup = setup("bridged-missing", work);
    let service = &setup.service;
    ask(service, Request::Create { machine: new_machine(&setup, "win") });
    let refused = ask(service, Request::Change { name: "win".into(), settings: bridged("Ethernet") });
    assert!(
        matches!(refused.as_slice(), [Answer::Refused { reason }] if reason == "Bridging needs Hyper-V.")
    );

    // A machine already bridged, whose adapter has since gone, still has its memory changed.
    {
        let mut entries = service.machines().lock();
        let entry = entries.get_mut("win").unwrap();
        entry.settings.network_mode = NetworkMode::Bridged { adapter: "Gone".into() };
    }
    let more = MachineSettings { memory_mb: 16384, ..bridged("Gone") };
    assert_eq!(ask(service, Request::Change { name: "win".into(), settings: more }), [Answer::Ok]);
    assert_eq!(status(service)[0].memory_mb, 16384);
}

#[test]
fn forwards_are_taken_away_of_a_machine_the_service_ends_as_it_stops() {
    let setup = setup("forwards-stop-all", Pretend::default());
    let service = &setup.service;
    ask(service, Request::Create { machine: new_machine(&setup, "win") });
    change(service, with_forwards(&[forward(8080, 80)]));
    start_and_run(service, "win");
    wait_net(service, |net| net.proxies.len() == 1);
    service.stop_all(Duration::from_secs(5));
    let net = net(service);
    assert!(net.proxies.is_empty() && net.rules.is_empty());
}
