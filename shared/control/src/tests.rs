use super::*;
use serde_json::json;

#[test]
fn requests_are_tagged_by_what_they_ask() {
    assert_eq!(serde_json::to_value(Request::Status).unwrap(), json!({"ask": "status"}));
    assert_eq!(
        serde_json::to_value(Request::Start { name: "win".into() }).unwrap(),
        json!({"ask": "start", "name": "win"})
    );
    let new = NewMachine {
        name: "u".into(),
        image: PathBuf::from("u.iso"),
        memory_mb: 8192,
        processors: 4,
        size_gb: 128,
        card: Card::Named("NVIDIA GeForce RTX 5090".into()),
        edition: String::new(),
        user: String::new(),
        folder: None,
        tpm: true,
        network: true,
        shares: Vec::new(),
    };
    let value = serde_json::to_value(Request::Create { machine: new }).unwrap();
    assert_eq!(value["machine"]["card"], json!({"card": "named", "name": "NVIDIA GeForce RTX 5090"}));
    let older: Request = serde_json::from_value(json!({"ask": "change", "name": "win", "settings": {
        "memory_mb": 16384, "processors": 8, "card": {"card": "none"}, "nvapi_wrapper": true
    }}))
    .unwrap();
    let settings = MachineSettings {
        memory_mb: 16384,
        processors: 8,
        card: Card::None,
        nvapi_wrapper: true,
        tpm: true,
        network: true,
        shares: Vec::new(),
        network_mode: NetworkMode::Shared,
        forwards: Vec::new(),
        nested_virtualization: false,
    };
    assert_eq!(
        older,
        Request::Change { name: "win".into(), settings },
        "an older app's change still reads, with a TPM, a network and no share"
    );
    let made: NewMachine = serde_json::from_value(json!({
        "name": "u", "image": "u.iso", "memory_mb": 8192, "processors": 4, "size_gb": 128,
        "card": {"card": "default"}
    }))
    .unwrap();
    assert!(made.tpm && made.network && made.shares.is_empty() && made.user.is_empty());
    assert_eq!(made.folder, None, "an older app's machine is kept where machines always were");
}

#[test]
fn remote_view_is_asked_for_with_the_other_hosts_words_and_an_older_app_still_reads() {
    let share: Request =
        serde_json::from_value(json!({"ask": "share", "name": "win", "on": true})).unwrap();
    assert_eq!(
        share,
        Request::Share { name: "win".into(), on: Some(true), pairing: None, pairing_seconds: None }
    );
    let asked: Request = serde_json::from_value(json!({"ask": "share", "name": "win"})).unwrap();
    assert_eq!(
        asked,
        Request::Share { name: "win".into(), on: None, pairing: None, pairing_seconds: None }
    );
    assert_eq!(
        serde_json::to_value(Request::Share {
            name: "win".into(),
            on: None,
            pairing: Some(true),
            pairing_seconds: Some(120)
        })
        .unwrap(),
        json!({"ask": "share", "name": "win", "on": null, "pairing": true, "pairing_seconds": 120})
    );
    assert_eq!(
        serde_json::to_value(Request::RemoteView { name: "win".into() }).unwrap(),
        json!({"ask": "remote_view", "name": "win"})
    );
    let info = ShareInfo { port: Some(7792), ..ShareInfo::off("win") };
    let answer = serde_json::to_value(Answer::Share { info: info.clone() }).unwrap();
    assert_eq!(
        (&answer["answer"], &answer["info"]["port"], &answer["info"]["state"]),
        (&json!("share"), &json!(7792), &json!("off"))
    );
    assert_eq!(serde_json::from_value::<Answer>(answer).unwrap(), Answer::Share { info });
}

#[test]
fn pairing_and_devices_are_asked_for_with_the_other_hosts_words() {
    for (request, json) in [
        (Request::PairClose { name: "win".into() }, json!({"ask": "pair_close", "name": "win"})),
        (Request::PairAccept { name: "win".into() }, json!({"ask": "pair_accept", "name": "win"})),
        (Request::PairDecline { name: "win".into() }, json!({"ask": "pair_decline", "name": "win"})),
        (
            Request::DeviceAllow {
                name: "win".into(),
                fingerprint: "ab".repeat(32),
                what: "clipboard".into(),
                on: true,
            },
            json!({"ask": "device_allow", "name": "win", "fingerprint": "ab".repeat(32), "what": "clipboard", "on": true}),
        ),
        (
            Request::DeviceRevoke { name: "win".into(), fingerprint: "ab".repeat(32) },
            json!({"ask": "device_revoke", "name": "win", "fingerprint": "ab".repeat(32)}),
        ),
    ] {
        assert_eq!(serde_json::to_value(&request).unwrap(), json);
        assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);
    }
    assert!(serde_json::from_value::<Request>(json!({"ask": "device_drop", "name": "win"})).is_err());
}

#[test]
fn a_machines_remote_view_is_listed_and_an_older_service_says_off() {
    let older = json!({
        "name": "win", "system": "windows", "state": "running", "set_up": true, "memory_mb": 8192,
        "processors": 4, "card": null, "address": "172.29.1.2"
    });
    let machine: Machine = serde_json::from_value(older.clone()).unwrap();
    assert_eq!((machine.remote_view, machine.remote_port), (false, None));
    let on = Machine { remote_view: true, remote_port: Some(7793), ..machine };
    let listed = serde_json::to_value(&on).unwrap();
    assert_eq!((&listed["remote_view"], &listed["remote_port"]), (&json!(true), &json!(7793)));
    // An app from before these, which reads a machine with only the fields it knows, still reads
    // the listing.
    #[derive(Deserialize)]
    struct Older {
        name: String,
        state: State,
        address: Option<String>,
    }
    let read: Older = serde_json::from_value(listed).unwrap();
    assert_eq!(
        (read.name.as_str(), read.state, read.address.as_deref()),
        ("win", State::Running, Some("172.29.1.2"))
    );
}

#[test]
fn a_machines_place_is_asked_for_and_moved_to_and_an_older_service_says_none() {
    let placed: NewMachine = serde_json::from_value(json!({
        "name": "u", "image": "u.iso", "memory_mb": 8192, "processors": 4, "size_gb": 128,
        "card": {"card": "default"}, "folder": "D:\\vms"
    }))
    .unwrap();
    assert_eq!(placed.folder, Some(PathBuf::from("D:\\vms")));
    let moved = Request::Move { name: "u".into(), folder: PathBuf::from("E:\\vms") };
    assert_eq!(
        serde_json::to_value(&moved).unwrap(),
        json!({"ask": "move", "name": "u", "folder": "E:\\vms"})
    );
    assert_eq!(serde_json::from_value::<Request>(serde_json::to_value(&moved).unwrap()).unwrap(), moved);
    let older: Machine = serde_json::from_value(json!({
        "name": "u", "system": "windows", "state": "off", "set_up": true, "memory_mb": 8192,
        "processors": 4, "card": null, "address": null
    }))
    .unwrap();
    assert!(older.folder.as_os_str().is_empty() && older.problem.is_none());
}

#[test]
fn a_machine_from_a_service_without_its_chosen_card_reads_as_default() {
    let machine: Machine = serde_json::from_value(json!({
        "name": "win", "system": "windows", "state": "off", "set_up": true,
        "memory_mb": 8192, "processors": 4, "card": "NVIDIA GeForce RTX 5090", "address": null
    }))
    .unwrap();
    assert_eq!(machine.chosen_card, Card::Default);
}

#[test]
fn a_change_carries_the_settings_and_an_older_status_reads_with_defaults() {
    let settings = MachineSettings {
        memory_mb: 16384,
        processors: 8,
        card: Card::Default,
        nvapi_wrapper: false,
        tpm: true,
        network: true,
        shares: Vec::new(),
        network_mode: NetworkMode::Shared,
        forwards: Vec::new(),
        nested_virtualization: true,
    };
    let value = serde_json::to_value(Request::Change { name: "win".into(), settings }).unwrap();
    assert_eq!(value["ask"], json!("change"));
    assert_eq!(value["settings"]["nvapi_wrapper"], json!(false));
    assert_eq!(value["settings"]["nested_virtualization"], json!(true));
    let older = json!({"name": "win", "system": "windows", "state": "off", "set_up": true,
        "memory_mb": 8192, "processors": 4, "card": null, "address": null});
    let machine: Machine = serde_json::from_value(older).unwrap();
    assert_eq!((machine.chosen_card, machine.nvapi_wrapper), (Card::Default, true));
    assert!(!machine.nested_virtualization, "a service from before nesting says none, which is off");
    assert_eq!(machine.ssh_port, None, "and one from before the SSH port says none");
}

#[test]
fn the_ssh_line_names_the_port_and_the_loopback() {
    assert_eq!(ssh_line(2201, "guesthouse"), "ssh -p 2201 guesthouse@127.0.0.1");
}

#[test]
fn a_request_for_a_reports_files_names_a_machine_or_none() {
    let named = Request::ReportFiles { machine: Some("win".into()) };
    assert_eq!(serde_json::to_value(&named).unwrap(), json!({"ask": "report_files", "machine": "win"}));
    let none: Request = serde_json::from_value(json!({"ask": "report_files", "machine": null})).unwrap();
    assert_eq!(none, Request::ReportFiles { machine: None });
}

#[test]
fn a_reports_files_come_as_parts_and_what_is_missing() {
    let part = Answer::ReportPart { name: "service.log".into(), text: "one\n".into() };
    assert_eq!(
        serde_json::to_value(&part).unwrap(),
        json!({"answer": "report_part", "name": "service.log", "text": "one\n"})
    );
    let missing = Answer::ReportMissing { name: "events.log".into(), reason: "there is none".into() };
    assert_eq!(
        serde_json::to_value(&missing).unwrap(),
        json!({"answer": "report_missing", "name": "events.log", "reason": "there is none"})
    );
    let back: Answer = serde_json::from_value(serde_json::to_value(&part).unwrap()).unwrap();
    assert_eq!(back, part);
}

#[test]
fn a_piece_of_the_worst_text_still_fits_a_line_the_app_reads() {
    for text in ["\u{1}".repeat(40_000), "\n".repeat(40_000), "\"".repeat(40_000), "画".repeat(40_000)] {
        for piece in report::pieces(&text) {
            let mut buffer = Vec::new();
            let answer = Answer::ReportPart { name: "a-long-file-name.txt".into(), text: piece.into() };
            write_line(&mut buffer, &answer).unwrap();
            assert!(buffer.len() < REQUEST_LIMIT, "{} bytes", buffer.len());
            let mut reader = io::BufReader::new(&buffer[..]);
            assert_eq!(read_line::<Answer>(&mut reader).unwrap(), Some(answer));
        }
    }
}

#[test]
fn a_networks_mode_and_forwards_read_and_write_as_the_apps_agree() {
    assert_eq!(serde_json::to_value(NetworkMode::Shared).unwrap(), json!({"mode": "shared"}));
    assert_eq!(
        serde_json::to_value(NetworkMode::Bridged { adapter: "Ethernet".into() }).unwrap(),
        json!({"mode": "bridged", "adapter": "Ethernet"})
    );
    let settings: MachineSettings = serde_json::from_value(json!({
        "memory_mb": 8192, "processors": 4, "card": {"card": "none"}, "nvapi_wrapper": true,
        "network_mode": {"mode": "bridged", "adapter": "Wi-Fi"},
        "forwards": [{"host_port": 8080, "guest_port": 80}]
    }))
    .unwrap();
    assert_eq!(settings.network_mode, NetworkMode::Bridged { adapter: "Wi-Fi".into() });
    assert_eq!(settings.forwards, [Forward { host_port: 8080, guest_port: 80 }]);
    // A service from before bridging says nothing of it, and a status still reads.
    let status: Answer = serde_json::from_value(
        json!({"answer": "status", "version": "0.9.0", "machines": [], "cards": []}),
    )
    .unwrap();
    assert!(matches!(status, Answer::Status { bridging, .. } if !bridging.available));
}

#[test]
fn hyper_v_is_said_as_a_case_each_and_one_this_app_doesnt_know_reads_as_unsaid() {
    for (hyper_v, line) in [
        (HyperV::On, json!({"state": "on"})),
        (HyperV::Off, json!({"state": "off"})),
        (HyperV::NotInEdition, json!({"state": "not_in_edition"})),
        (HyperV::RestartPending, json!({"state": "restart_pending"})),
        (HyperV::NoVirtualization, json!({"state": "no_virtualization"})),
        (HyperV::TurningOn { percent: Some(40) }, json!({"state": "turning_on", "percent": 40})),
        (
            HyperV::Failed { reason: "No.".into(), log: Some(r"C:\Windows\Logs\DISM\dism.log".into()) },
            json!({"state": "failed", "reason": "No.", "log": r"C:\Windows\Logs\DISM\dism.log"}),
        ),
        (HyperV::Unknown { reason: "Why.".into() }, json!({"state": "unknown", "reason": "Why."})),
    ] {
        assert_eq!(serde_json::to_value(&hyper_v).unwrap(), line);
        let bridging = json!({"available": false, "hyper_v": line});
        assert_eq!(serde_json::from_value::<Bridging>(bridging).unwrap().hyper_v, hyper_v);
    }
    assert_eq!(serde_json::to_value(Request::EnableHyperV).unwrap(), json!({"ask": "enable_hyper_v"}));
    // Before this, and a case from a newer service: the rest still reads.
    let before: Bridging = serde_json::from_value(json!({"available": false, "reason": "Off."})).unwrap();
    assert_eq!((before.hyper_v, before.reason.as_deref()), (HyperV::Unsaid, Some("Off.")));
    let newer: Bridging = serde_json::from_value(json!({"available": true, "adapters": [],
        "hyper_v": {"state": "sideways", "why": 3}}))
    .unwrap();
    assert!(newer.available && newer.hyper_v == HyperV::Unsaid);
    let garbled: Bridging = serde_json::from_value(json!({"available": false, "hyper_v": 7})).unwrap();
    assert_eq!(garbled.hyper_v, HyperV::Unsaid);
}

#[test]
fn forwards_are_kept_to_their_rules() {
    let forward = |host_port, guest_port| Forward { host_port, guest_port };
    assert_eq!(forwards_refusal(&[]), None);
    assert_eq!(forwards_refusal(&[forward(8080, 80), forward(2222, 22), forward(1, 65535)]), None);
    // Two of this PC's ports may lead to one guest port, but not one of this PC's to two.
    assert_eq!(forwards_refusal(&[forward(8080, 80), forward(8081, 80)]), None);
    assert!(forwards_refusal(&[forward(0, 80)]).is_some());
    assert!(forwards_refusal(&[forward(80, 0)]).is_some());
    assert!(forwards_refusal(&[forward(8080, 80), forward(8080, 81)]).is_some());
    let many: Vec<Forward> = (1..=MOST_FORWARDS as u16 + 1).map(|port| forward(port, port)).collect();
    assert!(forwards_refusal(&many).is_some());
    assert_eq!(forwards_refusal(&many[..MOST_FORWARDS]), None);
}

#[test]
fn the_windows_version_is_three_numbers_and_nothing_else() {
    let parts: Vec<&str> = WINDOWS_VERSION.split('.').collect();
    assert_eq!(parts.len(), 3, "{WINDOWS_VERSION:?}");
    assert!(parts.iter().all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())));
}

#[test]
fn a_line_reads_back_as_it_was_written() {
    let mut buffer = Vec::new();
    write_line(&mut buffer, &Answer::Refused { reason: "busy".into() }).unwrap();
    write_line(&mut buffer, &Answer::Ok).unwrap();
    let mut reader = io::BufReader::new(&buffer[..]);
    assert_eq!(
        read_line::<Answer>(&mut reader).unwrap(),
        Some(Answer::Refused { reason: "busy".into() })
    );
    assert_eq!(read_line::<Answer>(&mut reader).unwrap(), Some(Answer::Ok));
    assert_eq!(read_line::<Answer>(&mut reader).unwrap(), None);
}

#[test]
fn a_line_that_isnt_a_request_is_refused_as_bad_data() {
    let mut reader = io::BufReader::new(&b"{\"ask\":\"format the host\"}\n"[..]);
    assert_eq!(read_line::<Request>(&mut reader).unwrap_err().kind(), io::ErrorKind::InvalidData);
}

#[test]
fn an_address_is_found_in_either_systems_report() {
    assert_eq!(address_in("guesthouse: address: 172.29.167.163").as_deref(), Some("172.29.167.163"));
    assert_eq!(
        address_in("address: 2: eth0    inet 172.29.164.142/20 metric 100 brd 172.29.175.255").as_deref(),
        Some("172.29.164.142")
    );
    assert_eq!(address_in("address: 1: lo    inet 127.0.0.1/8 scope host lo"), None);
    assert_eq!(address_in("card: NVIDIA GeForce RTX 5090"), None);
}

#[test]
fn the_default_user_is_one_both_systems_take() {
    use guesthouse_account_core::common::{linux_name_problem, windows_name_problem};
    assert_eq!(windows_name_problem(DEFAULT_USER), None);
    assert_eq!(linux_name_problem(DEFAULT_USER), None);
}

#[test]
fn a_user_is_one_either_system_takes() {
    for name in ["", "guesthouse", "a-b_2", "Ann B.Lee", "_a", "administrator", "root"] {
        assert_eq!(user_problem(name), None, "{name}");
    }
    for name in ["Admin istrator ", "a@b", "Guest", &"x".repeat(33)] {
        assert!(user_problem(name).is_some(), "{name}");
    }
    assert!(valid_edition("Windows 11 Pro for Workstations"));
    assert!(!valid_edition("Windows\\11"));
}

#[test]
fn a_name_is_one_a_folder_and_a_vm_take() {
    assert!(valid_name("ubuntu-24_2"));
    assert!(!valid_name(""));
    assert!(!valid_name("../x"));
    assert!(!valid_name("a b"));
}

#[test]
fn shares_read_as_a_list_and_as_the_one_share_an_older_app_sends() {
    let one = json!({"folder": "D:\\files", "read_only": true});
    let share = Share { folder: PathBuf::from(r"D:\files"), read_only: true };
    let base =
        json!({"memory_mb": 8192, "processors": 4, "card": {"card": "default"}, "nvapi_wrapper": true});
    let read = |extra: serde_json::Value| {
        let mut value = base.clone();
        value.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        serde_json::from_value::<MachineSettings>(value).unwrap().shares
    };
    assert_eq!(read(json!({})), []);
    assert_eq!(read(json!({"share": null})), []);
    assert_eq!(read(json!({"share": one})), std::slice::from_ref(&share));
    assert_eq!(read(json!({"shares": [one, {"folder": "E:\\more"}]})).len(), 2);
    let settings: MachineSettings = serde_json::from_value(base).unwrap();
    let written = serde_json::to_value(MachineSettings { shares: vec![share], ..settings }).unwrap();
    assert_eq!(written["shares"][0]["folder"], json!("D:\\files"));
}

#[test]
fn shares_are_eight_at_most_and_each_folder_once() {
    let share = |folder: &str| Share { folder: PathBuf::from(folder), read_only: false };
    assert_eq!(shares_refusal(&[share(r"C:\a"), share(r"C:\b")]), None);
    assert!(shares_refusal(&[share(r"C:\a"), share(r"c:\A\")]).unwrap().contains("twice"));
    assert!(shares_refusal(&[share("")]).is_some());
    let many: Vec<Share> = (0..=MOST_SHARES).map(|n| share(&format!(r"C:\{n}"))).collect();
    assert!(shares_refusal(&many).unwrap().contains("at most"));
    assert_eq!(shares_refusal(&many[..MOST_SHARES]), None);
}
