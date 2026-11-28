//! Exactly which entries the palette offers in each state, and in what order a query finds them.

use super::*;
use guesthouse_control::{Card, NetworkMode};
use std::path::PathBuf;

fn machine(name: &str, system: System, state: State) -> Machine {
    Machine {
        name: name.to_owned(),
        system,
        state,
        set_up: true,
        memory_mb: 8192,
        processors: 4,
        card: None,
        chosen_card: Card::Default,
        nvapi_wrapper: true,
        tpm: true,
        network: true,
        shares: Vec::new(),
        network_mode: NetworkMode::Shared,
        forwards: Vec::new(),
        nested_virtualization: false,
        ssh_port: None,
        size_gb: None,
        folder: PathBuf::new(),
        problem: None,
        snapshots: Vec::new(),
        working: None,
        address: None,
        remote_view: false,
        remote_port: None,
        console_pipe: None,
        needs_owner: false,
    }
}

fn machines() -> Vec<Machine> {
    vec![
        machine("gaming", System::Windows, State::Running),
        machine("media", System::Windows, State::Off),
        machine("build-box", System::Linux, State::Starting),
        machine("office", System::Windows, State::Installing),
    ]
}

fn everyone(_: &Machine) -> bool {
    true
}

fn keys(list: &[Entry]) -> Vec<&str> {
    list.iter().map(|entry| entry.key.as_str()).collect()
}

fn offered(here: Here, machines: Option<&[Machine]>, can_look: bool) -> Vec<Entry> {
    entries(&Now { here, machines, addressed: &everyone, can_look })
}

#[test]
fn on_a_running_machines_page_its_own_actions_come_first_then_the_others_then_the_apps() {
    let machines = machines();
    let list = offered(Here::Machine("gaming".into(), Section::Overview), Some(&machines), true);
    assert_eq!(
        keys(&list),
        [
            "screen:gaming",
            "stop:gaming",
            "section Remote View:gaming",
            "section Hardware:gaming",
            "section Network:gaming",
            "section Storage:gaming",
            "go:media",
            "start:media",
            "go:build-box",
            "stop:build-box",
            "go:office",
            "home",
            "new",
            "settings",
            "report",
            "look",
        ]
    );
    // What each does is the messages the buttons send: another machine's goes to its page first.
    let step = |key: &str| list.iter().find(|entry| entry.key == key).unwrap().steps.clone();
    assert_eq!(step("stop:gaming"), [Step::Stop]);
    assert_eq!(step("start:media"), [Step::Open("media".into()), Step::Start]);
    assert_eq!(step("section Network:gaming"), [Step::Section(Section::Network)]);
    assert_eq!(step("look"), [Step::Settings, Step::Look]);
    assert_eq!(list[0].group.heading(), "gaming · Running");
    assert_eq!(list[1].spoken(), "Stop, asks its guest to shut down, gaming");
    assert_eq!(list[7].spoken(), "Start media, boots it");
}

#[test]
fn only_what_can_be_done_now_is_offered() {
    let mut machines = machines();
    // Off: Start and no Stop; running: Stop and no Start; stopping: neither, as Stop now isn't offered.
    machines[0].state = State::Stopping;
    machines[1].working = Some("Taking a snapshot".into());
    let list = offered(Here::Home, Some(&machines), false);
    assert_eq!(
        keys(&list),
        ["go:gaming", "go:media", "go:build-box", "stop:build-box", "go:office", "new", "settings", "report"]
    );
    for entry in &list {
        assert!(!entry.name.contains("Delete") && !entry.name.contains("Stop now"), "{}", entry.name);
    }
    // A Windows guest's screen needs it running, set up, and an address; an Ubuntu guest has none.
    machines[0].state = State::Running;
    machines[0].set_up = false;
    let nowhere = |_: &Machine| false;
    let list = entries(&Now { here: Home, machines: Some(&machines), addressed: &everyone, can_look: false });
    assert!(!keys(&list).contains(&"screen:gaming"), "not set up");
    machines[0].set_up = true;
    let list =
        entries(&Now { here: Here::Home, machines: Some(&machines), addressed: &nowhere, can_look: false });
    assert!(!keys(&list).contains(&"screen:gaming"), "no address");
    let mut ubuntu = machine("u", System::Linux, State::Running);
    ubuntu.address = Some("172.24.80.31".into());
    let list =
        offered(Here::Machine("u".into(), Section::Hardware), Some(std::slice::from_ref(&ubuntu)), false);
    assert_eq!(
        keys(&list),
        [
            "stop:u",
            "section Overview:u",
            "section Network:u",
            "section Storage:u",
            "home",
            "new",
            "settings",
            "report"
        ],
        "an Ubuntu guest has no screen and no Remote View"
    );
    // A failed machine starts again, as its button does.
    let failed = [machine("f", System::Windows, State::Failed)];
    assert!(keys(&offered(Here::Home, Some(&failed), false)).contains(&"start:f"));
}

use Here::Home;

#[test]
fn with_the_service_away_only_the_apps_own_are_offered() {
    assert_eq!(keys(&offered(Home, None, true)), ["settings", "report", "look"]);
    assert_eq!(keys(&offered(Here::Elsewhere, None, false)), ["settings", "report"]);
    // A page that isn't a machine's offers Home, and New machine with the service there.
    assert_eq!(keys(&offered(Here::Elsewhere, Some(&[]), false)), ["home", "new", "settings", "report"]);
}

#[test]
fn a_query_finds_entries_by_the_starts_of_their_words_in_three_runs() {
    let machines = machines();
    let list = offered(Here::Machine("gaming".into(), Section::Overview), Some(&machines), true);
    let found =
        |query: &str| keys(&matching(&list, query)).into_iter().map(str::to_owned).collect::<Vec<_>>();
    assert_eq!(found(""), keys(&list), "an empty query finds every entry, in order");
    assert_eq!(found("   "), keys(&list));
    // Names that begin with it, in the palette's order.
    assert_eq!(found("st"), ["stop:gaming", "section Storage:gaming", "start:media", "stop:build-box"]);
    // Then those of a machine whose name begins with it, then the rest.
    assert_eq!(found("media"), ["go:media", "start:media"]);
    assert_eq!(found("box st"), ["stop:build-box"]);
    assert_eq!(found("o"), ["screen:gaming", "go:office"]);
    assert_eq!(
        found("gam"),
        [
            "screen:gaming",
            "stop:gaming",
            "section Remote View:gaming",
            "section Hardware:gaming",
            "section Network:gaming",
            "section Storage:gaming"
        ]
    );
    // Any case, any order of the words.
    assert_eq!(found("MEDIA start"), ["start:media"]);
    assert_eq!(found("start med"), ["start:media"]);
    // A name's parts are words: build-box is found by box, and by build-b.
    assert_eq!(found("box"), ["go:build-box", "stop:build-box"]);
    assert_eq!(found("build-b"), ["go:build-box", "stop:build-box"]);
    assert_eq!(found("go"), ["go:media", "go:build-box", "go:office"]);
    assert_eq!(found("report"), ["report"]);
    assert_eq!(found("updates"), ["look"]);
    assert_eq!(found("new"), ["new"]);
    // The middle of a word isn't its start.
    assert!(found("aming").is_empty());
    assert!(found("delete").is_empty());
}

#[test]
fn the_keys_move_and_run_and_ctrl_k_is_only_ctrl_k() {
    assert_eq!(Key::of(0x26), Some(Key::Up));
    assert_eq!(Key::of(0x28), Some(Key::Down));
    assert_eq!(Key::of(0x0D), Some(Key::Run));
    assert_eq!(Key::of(0x1B), None, "Escape is the dialog's");
    assert_eq!(Key::of(0x41), None, "letters are typed");
    assert_eq!(moved(0, Key::Up, 5), 0);
    assert_eq!(moved(0, Key::Down, 5), 1);
    assert_eq!(moved(4, Key::Down, 5), 4);
    assert_eq!(moved(0, Key::Down, 0), 0);
    assert!(is_palette_key(0x4B, true, false, false));
    assert!(!is_palette_key(0x4B, false, false, false), "K alone is typed");
    assert!(!is_palette_key(0x4B, true, true, false), "AltGr is Ctrl+Alt");
    assert!(!is_palette_key(0x4B, true, false, true));
    assert!(!is_palette_key(0x4C, true, false, false));
}

#[test]
fn the_buttons_and_the_palette_agree_on_what_can_be_done() {
    for state in [
        State::Off,
        State::Installing,
        State::Starting,
        State::Running,
        State::Stopping,
        State::StoppingStalled,
        State::Failed,
    ] {
        let machine = machine("m", System::Windows, state);
        assert_eq!(can_start(&machine), matches!(state, State::Off | State::Failed), "{state:?}");
        assert_eq!(can_ask_to_stop(&machine), matches!(state, State::Starting | State::Running), "{state:?}");
        assert_eq!(can_open_screen(&machine, true), state == State::Running, "{state:?}");
        assert!(!can_open_screen(&machine, false));
    }
}
