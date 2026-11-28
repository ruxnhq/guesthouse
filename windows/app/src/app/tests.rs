use super::*;

fn machine() -> Machine {
    Machine {
        name: "win".to_owned(),
        system: System::Windows,
        state: State::Off,
        set_up: true,
        memory_mb: 8192,
        processors: 4,
        card: Some("NVIDIA GeForce RTX 5090".to_owned()),
        chosen_card: Card::Default,
        nvapi_wrapper: true,
        tpm: true,
        network: true,
        shares: Vec::new(),
        network_mode: NetworkMode::Shared,
        forwards: Vec::new(),
        nested_virtualization: false,
        ssh_port: None,
        size_gb: Some(128),
        folder: PathBuf::new(),
        problem: None,
        snapshots: Vec::new(),
        working: None,
        address: None,
        remote_view: false,
        remote_port: None,
        console_pipe: None,
    }
}

fn cards() -> Vec<CardInfo> {
    vec![
        CardInfo { name: "NVIDIA GeForce RTX 5090".to_owned(), memory_mb: Some(32607), default: true },
        CardInfo { name: "AMD Radeon RX 7900 XTX".to_owned(), memory_mb: Some(24560), default: false },
    ]
}

#[test]
fn a_fresh_draft_is_unchanged_and_asks_for_what_the_machine_has() {
    let machine = machine();
    let draft = Draft::of(&machine);
    assert!(!draft.changed(&machine));
    assert_eq!(draft.settings(), Some(settings_of(&machine)));
}

#[test]
fn a_draft_with_a_number_missing_asks_for_nothing_and_counts_as_changed() {
    let machine = machine();
    let mut draft = Draft::of(&machine);
    draft.processors = None;
    assert_eq!(draft.settings(), None);
    assert!(draft.changed(&machine));
    draft.processors = Some(0.0);
    assert_eq!(draft.settings(), None);
}

#[test]
fn shared_folders_are_edited_as_rows_and_asked_for_once_each_has_a_path() {
    let machine = machine();
    let mut draft = Draft::of(&machine);
    draft.shares.push(ShareRow::default());
    assert!(draft.changed(&machine));
    assert!(draft.shares_problem().unwrap().contains("a path"));
    assert_eq!(draft.problem(), draft.shares_problem());
    draft.shares[0].folder = r#""D:\files""#.to_owned();
    draft.shares.push(ShareRow { folder: r"E:\more".to_owned(), read_only: true });
    assert_eq!(draft.problem(), None);
    let shares = draft.settings().unwrap().shares;
    assert_eq!(
        shares,
        [
            Share { folder: PathBuf::from(r"D:\files"), read_only: false },
            Share { folder: PathBuf::from(r"E:\more"), read_only: true },
        ]
    );
    draft.shares[1].folder = r"d:\FILES\".to_owned();
    assert!(draft.shares_problem().unwrap().contains("twice"));
    draft.shares.clear();
    assert!(!draft.changed(&machine));
}

#[test]
fn forwards_are_edited_as_rows_and_asked_for_once_each_holds_two_ports() {
    let machine = machine();
    let mut draft = Draft::of(&machine);
    draft.forwards.push(ForwardRow::NEW);
    assert!(draft.changed(&machine));
    assert_eq!(draft.settings(), None, "a row with no ports isn't a forward yet");
    assert!(draft.forwards_problem().unwrap().contains("both ports"));
    draft.forwards[0].host = Some(8080.0);
    draft.forwards[0].guest = Some(80.0);
    assert_eq!(draft.forwards_problem(), None);
    assert_eq!(draft.settings().unwrap().forwards, [Forward { host_port: 8080, guest_port: 80 }]);
    // A port that isn't one is a row that isn't finished.
    draft.forwards[0].guest = Some(70000.0);
    assert_eq!(draft.settings(), None);
    draft.forwards[0].guest = Some(80.0);
    draft.forwards.push(ForwardRow { host: Some(8080.0), guest: Some(81.0) });
    assert!(draft.forwards_problem().unwrap().contains("twice"));
    draft.forwards.pop();
    // Back to what the machine has, it's unchanged again.
    draft.forwards.clear();
    assert!(!draft.changed(&machine));
}

#[test]
fn a_machine_bridged_is_a_change_from_shared_and_asks_for_the_adapter_by_name() {
    let machine = machine();
    let mut draft = Draft::of(&machine);
    draft.network_mode = NetworkMode::Bridged { adapter: "Ethernet".to_owned() };
    assert!(draft.changed(&machine));
    assert_eq!(
        draft.settings().unwrap().network_mode,
        NetworkMode::Bridged { adapter: "Ethernet".to_owned() }
    );
}

#[test]
fn an_ubuntu_guest_is_reached_by_its_port_and_by_its_address_when_it_has_one() {
    let mut machine = Machine { system: System::Linux, ..machine() };
    assert!(ssh_lines(&machine, "me").is_empty());
    machine.ssh_port = Some(2203);
    assert_eq!(
        ssh_lines(&machine, "me"),
        [("SSH, no network needed", "ssh -p 2203 me@127.0.0.1".to_owned())]
    );
    machine.address = Some("172.24.80.12".to_owned());
    let lines = ssh_lines(&machine, "me");
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1].1, "ssh me@172.24.80.12");
    machine.address = Some("not an address".to_owned());
    assert_eq!(ssh_lines(&machine, "me").len(), 1);
}

#[test]
fn nested_virtualization_starts_off_and_turning_it_on_is_a_change() {
    let machine = machine();
    let mut draft = Draft::of(&machine);
    assert!(!draft.settings().unwrap().nested_virtualization);
    draft.nested_virtualization = true;
    assert!(draft.changed(&machine));
    assert!(draft.settings().unwrap().nested_virtualization);
}

#[test]
fn turning_dlss_off_is_a_change() {
    let machine = machine();
    let mut draft = Draft::of(&machine);
    draft.nvapi_wrapper = false;
    assert!(draft.changed(&machine));
    assert!(!draft.settings().unwrap().nvapi_wrapper);
}

#[test]
fn dlss_is_offered_for_a_windows_guest_with_an_nvidia_card_only() {
    let cards = cards();
    let named = |name: &str| Card::Named(name.to_owned());
    assert!(offers_dlss(System::Windows, &Card::Default, &cards));
    assert!(offers_dlss(System::Windows, &named("NVIDIA GeForce RTX 5090"), &cards));
    assert!(!offers_dlss(System::Windows, &named("AMD Radeon RX 7900 XTX"), &cards));
    assert!(!offers_dlss(System::Windows, &Card::None, &cards));
    assert!(!offers_dlss(System::Linux, &Card::Default, &cards));
    // With no card the default, the default is no one's.
    assert!(!offers_dlss(System::Windows, &Card::Default, &cards[1..]));
}

#[test]
fn a_user_either_system_takes_is_sent_and_one_neither_takes_isnt() {
    let form = |user: &str| Form {
        name: "win".into(),
        image: "win.iso".into(),
        user: user.into(),
        ..Form::default()
    };
    assert_eq!(form("").machine().unwrap().user, "", "empty is the default");
    assert_eq!(form(" Ann B.Lee ").machine().unwrap().user, "Ann B.Lee");
    assert_eq!(form("_build").machine().unwrap().user, "_build");
    assert!(form("a@b").machine().is_none());
}

#[test]
fn a_location_is_asked_for_only_when_one_is_typed() {
    let mut form = Form { name: "win".into(), image: "win.iso".into(), ..Form::default() };
    assert_eq!(form.machine().unwrap().folder, None, "empty is the service's own folder");
    form.location = "  \"D:\\vms\"  ".into();
    assert_eq!(form.machine().unwrap().folder, Some(PathBuf::from("D:\\vms")));
    form.location = "   ".into();
    assert_eq!(form.machine().unwrap().folder, None);
}
