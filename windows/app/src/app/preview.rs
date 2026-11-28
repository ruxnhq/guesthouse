//! Made-up machines in place of the service's, for looking at each of the window's states without a
//! service or a GPU. Only a build with the `preview` feature has it. GUESTHOUSE_APP_PREVIEW names
//! the state, GUESTHOUSE_APP_THEME may say `light` or `dark`, and GUESTHOUSE_APP_SECTION may name the
//! section a machine's page opens on (`Hardware`, `Network`, `Storage` or `Remote View`):
//!
//! ```powershell
//! cargo build -p guesthouse-app-windows --features preview
//! $env:GUESTHOUSE_APP_PREVIEW = "running"; .\target\debug\guesthouse-app.exe
//! ```
//!
//! The states are `home` (the machines' cards, some running), `running`, `linux`, `starting`,
//! `empty` (no machines yet), `away`, `refused`, `new`, `making`,
//! `failed`, `delete`, `settings` (a stopped machine's settings being changed) and `settings-refused`
//! (the service said no to them), `report-new` (the report page, being written), `report` (a
//! prepared report, and `report-sent` once it has an ID) and `report-away` (the report page when the
//! service can't be reached). `update` is Home with a newer Guesthouse out, the dot on the gear, and
//! the app's Settings page is `app-settings`, with how the updates stand as `updates-newest` (the
//! same), `updates-looking`, `updates-out`, `updates-unreached` and `updates-repo` (a build of the
//! repo), and an update's own: `updates-downloading`, `updates-download-failed`,
//! `updates-verifying`, `updates-mismatch`, `updates-ready`, `updates-ready-running` (machines that
//! an install would stop), `updates-ask` (the question about them), `updates-ready-away` and
//! `updates-ask-away` (the same while the service isn't answering), `updates-blocked` (a machine
//! being made), `updates-installing`, `updates-declined`, `updates-launch-failed` and
//! `updates-installed`. Windows' Hyper-V feature, which bridging needs, has a stopped machine's
//! Network section in each case the service says: `net-off`, `net-held` (machines running hold
//! turning it on up), `net-ask` (the question before it's turned on), `net-turning`, `net-failed`,
//! `net-restart`, `net-restart-ask` (the question before Windows restarts), `net-home` (Windows
//! Home), `net-firmware`, `net-unknown` and `net-older` (a service from before it said), and
//! `home-restart` is Home waiting for the restart. The command palette is open in `palette` (on a
//! running machine's page), `palette-query` (with `st` typed), `palette-home` and `palette-away`
//! (the service not answering). A preview counts as installed, reads and writes none of the
//! person's settings, and never reaches the file drop, downloads or runs anything: its updater is
//! `update::Fake`, whose look finds what its state did, whose download and check go well at once,
//! and whose installer ends having done nothing. Nothing is sent to the service, whatever is
//! pressed.

use guesthouse_control::client::Unreached;
use guesthouse_control::{
    AdapterInfo, Bridging, Card, CardInfo, Forward, HyperV, Machine, NetworkMode, SnapshotInfo, State, System,
};
use windows_reactor::WindowTheme;

use super::{Account, Form, Kind, Line, Main, Making, Page, Ready, ReportStage, Section};
use crate::bridge::Status;
use crate::log::Log;
use crate::preferences::WhenOut;
use crate::remote_view::{Ask, Viewing};
use crate::report::Prepared;
use crate::update::{self, DownloadError, Fake, Fetched, Updates, release_file};
use guesthouse_control::ShareInfo;
use guesthouse_control::report::Summary;
use guesthouse_remote_view_core::share::{CloseReason, DeviceInfo, Pending, Permissions, ShareState};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

const STATE: &str = "GUESTHOUSE_APP_PREVIEW";
const THEME: &str = "GUESTHOUSE_APP_THEME";
const SECTION: &str = "GUESTHOUSE_APP_SECTION";

pub fn wanted() -> bool {
    std::env::var_os(STATE).is_some()
}

/// Puts the state the variable names into `main`, which then asks nothing of the service.
pub fn stage(main: &mut Main) {
    let state = std::env::var(STATE).unwrap_or_default();
    main.live = false;
    // As an installed app, which looks for updates, though it never reaches the file drop.
    main.installed = true;
    main.updater = Arc::new(Fake::finding(Fetched::Unreached));
    main.updates = Updates::new(true, guesthouse_control::WINDOWS_VERSION).with_stand_in_key();
    main.theme = match std::env::var(THEME).as_deref() {
        Ok("light") => WindowTheme::Light,
        Ok("dark") => WindowTheme::Dark,
        _ => WindowTheme::System,
    };
    main.seen = Ok(status());
    match state.as_str() {
        "home" => {
            for (name, state) in [("win11", State::Running), ("build-box", State::Starting)] {
                show(main, name, state);
            }
            main.page = Page::Home;
        }
        "linux" => {
            show(main, "ubuntu", State::Running);
            main.account = Some(account("ubuntu"));
            main.log = log(guest_log());
        }
        "starting" => {
            show(main, "build-box", State::Starting);
            main.log = log(vec![
                note("Starting."),
                note("Its first boot sets it up without the card."),
                line(Kind::Guest, "Ubuntu 26.04 installing: copying files (41%)"),
            ]);
        }
        "empty" => main.seen = Ok(Status { machines: Vec::new(), ..status() }),
        "away" => main.seen = Err(Unreached::NoService.words().to_owned()),
        "refused" => main.seen = Err(Unreached::Refused.words().to_owned()),
        "new" | "making" | "failed" => {
            main.page = Page::New;
            main.form = Form {
                name: "studio".to_owned(),
                image: r"D:\iso\Win11_25H2_English_x64.iso".to_owned(),
                // A named card, so the closed list shows a card's memory and its default mark.
                card: Card::Named("NVIDIA GeForce RTX 5090".to_owned()),
                ..Form::default()
            };
            let ended = match state.as_str() {
                "making" => None,
                "failed" => {
                    Some(Some("There isn't room for a 128 GB disk on D:, which has 61 GB free.".to_owned()))
                }
                _ => return,
            };
            main.making = Some(Making {
                name: "studio".to_owned(),
                lines: log(vec![
                    note("Reading the image."),
                    note("Windows 11 Pro, from install.wim."),
                    note("Making a 128 GB disk."),
                    line(Kind::Problem, "The disk's folder is compressed, which makes it slower."),
                    note("Applying Windows to the disk. This takes a few minutes."),
                ]),
                ended,
            });
        }
        "settings" | "settings-refused" => {
            show(main, "win11", State::Off);
            if let Some(draft) = &mut main.draft {
                draft.memory_mb = Some(24576.0);
                draft.nvapi_wrapper = false;
                if state == "settings-refused" {
                    draft.processors = Some(64.0);
                    draft.refused =
                        Some("A machine has 1 to 32 processors, as many as this PC has.".to_owned());
                }
            }
        }
        "update" => {
            for (name, state) in [("win11", State::Running), ("build-box", State::Off)] {
                show(main, name, state);
            }
            main.page = Page::Home;
            updates(main, Some(Fetched::Text(release_file(&next_version()))));
        }
        "app-settings" | "updates-newest" => {
            main.page = Page::AppSettings;
            updates(main, Some(Fetched::Text(release_file(guesthouse_control::WINDOWS_VERSION))));
        }
        "updates-looking" => {
            main.page = Page::AppSettings;
            updates(main, None);
        }
        "updates-out" => {
            main.page = Page::AppSettings;
            updates(main, Some(Fetched::Text(release_file(&next_version()))));
        }
        "updates-unreached" => {
            main.page = Page::AppSettings;
            updates(main, Some(Fetched::Unreached));
        }
        staged if staged.starts_with("updates-") && STAGED.contains(&staged) => {
            main.page = Page::AppSettings;
            stage_update(main, staged);
        }
        "updates-repo" => {
            main.page = Page::AppSettings;
            main.installed = false;
            main.updates = Updates::new(false, guesthouse_control::WINDOWS_VERSION).with_stand_in_key();
        }
        "report-new" => {
            show(main, "win11", State::Off);
            main.page = Page::Report;
            main.report.machine = Some("win11".to_owned());
            main.report.what = "It stays black after the logo, and the log says nothing.".to_owned();
        }
        "report" | "report-sent" => {
            show(main, "win11", State::Off);
            main.page = Page::Report;
            main.report.machine = Some("win11".to_owned());
            main.report.stage = ReportStage::Ready(Ready {
                prepared: report(),
                sending: false,
                id: (state == "report-sent").then(|| "0a1b2c3d4e".to_owned()),
                problem: None,
                saved: None,
            });
        }
        "report-away" => {
            main.seen = Err(Unreached::NoService.words().to_owned());
            main.page = Page::Report;
        }
        "delete" => {
            show(main, "ubuntu", State::Off);
            main.deleting = Some("ubuntu".to_owned());
        }
        net if net.starts_with("net-") || net == "home-restart" => hyper_v(main, net),
        "palette" | "palette-query" | "palette-home" | "palette-away" => {
            match state.as_str() {
                "palette-home" => {
                    for (name, state) in [("win11", State::Running), ("build-box", State::Starting)] {
                        show(main, name, state);
                    }
                    main.page = Page::Home;
                }
                "palette-away" => main.seen = Err(Unreached::NoService.words().to_owned()),
                _ => show(main, "win11", State::Running),
            }
            let query = if state == "palette-query" { "st" } else { "" };
            main.palette = Some(super::Palette { query: query.to_owned(), at: 0 });
        }
        remote if remote.starts_with("rv-") => {
            remote_view(main, remote);
            main.section = Section::RemoteView;
        }
        _ => {
            show(main, "win11", State::Running);
            main.account = Some(account("win11"));
            main.log = log(vec![
                note("Starting."),
                note("Giving it NVIDIA GeForce RTX 5090."),
                line(
                    Kind::Guest,
                    "guesthouse: card: NVIDIA GeForce RTX 5090, driver 32.0.16.1062, status OK",
                ),
                line(Kind::Guest, "guesthouse: nvidia-smi: GPU 0: NVIDIA GeForce RTX 5090"),
                line(Kind::Problem, "The guest's clock was 2 minutes behind. It was set again."),
                line(Kind::Guest, "guesthouse: address: 172.24.80.12"),
                line(Kind::Report, "Display driver loaded in 3.1 s."),
                note("It's running."),
            ]);
        }
    }
    if let Some(section) = std::env::var(SECTION).ok().as_deref().and_then(Section::of_label) {
        main.section = section;
    }
}

/// A stopped Windows machine's Network section while Hyper-V stands as `state` says, named
/// `net-...`, or Home waiting for the restart, `home-restart`. Turn on answers as if it worked, at
/// once, and Restart now asks and then does nothing: a preview never asks the service or Windows.
fn hyper_v(main: &mut Main, state: &str) {
    let hyper_v = match state {
        "net-off" | "net-held" | "net-ask" => HyperV::Off,
        "net-turning" => HyperV::TurningOn { percent: Some(40) },
        "net-failed" => HyperV::Failed {
            reason:
                "Windows couldn't download the files Hyper-V needs (0x800F0906). Check that this PC reaches \
                     Windows Update."
                    .to_owned(),
            log: Some(r"C:\Windows\Logs\DISM\dism.log".to_owned()),
        },
        "net-restart" | "net-restart-ask" | "home-restart" => HyperV::RestartPending,
        "net-home" => HyperV::NotInEdition,
        "net-firmware" => HyperV::NoVirtualization,
        "net-unknown" => HyperV::Unknown {
            reason: "DISM didn't say how Hyper-V stands in 120 s, so it was stopped.".to_owned(),
        },
        // net-older: a service from before it said where Hyper-V stands.
        _ => HyperV::Unsaid,
    };
    if let Ok(status) = &mut main.seen {
        status.bridging = Bridging {
            available: false,
            reason: Some(
                "Bridging needs Windows' Hyper-V feature, which isn't turned on, or isn't in this edition: Windows \
                 Home has none. On Pro, Education and Enterprise, turn on Hyper-V in Windows Features and restart."
                    .to_owned(),
            ),
            adapters: Vec::new(),
            hyper_v,
        };
        // Nothing holds turning it on up, but in net-held, where two machines do.
        for machine in &mut status.machines {
            machine.state = match (state, machine.name.as_str()) {
                ("net-held", "ubuntu") => State::Running,
                ("net-held", "office") => State::Installing,
                ("net-restart-ask", "ubuntu") => State::Running,
                _ => State::Off,
            };
        }
    }
    if state == "home-restart" {
        main.page = Page::Home;
        return;
    }
    show(main, "win11", State::Off);
    main.section = Section::Network;
    main.hyper_v_ask = state == "net-ask";
    main.restart_ask = state == "net-restart-ask";
}

/// Pretends the app is installed and has looked for a newer Guesthouse: the look is still going
/// with `None`, and found what `fetched` says otherwise. Look now finds the same again.
fn updates(main: &mut Main, fetched: Option<Fetched>) {
    main.installed = true;
    main.updates = Updates::new(true, guesthouse_control::WINDOWS_VERSION).with_stand_in_key();
    main.updater = Arc::new(Fake::finding(fetched.clone().unwrap_or(Fetched::Unreached)));
    let [update::Effect::Fetch { attempt }] = main.updates.step(update::Input::Look, NOW)[..] else {
        return;
    };
    if let Some(fetched) = fetched {
        main.updates.step(update::Input::Looked { attempt, fetched }, NOW);
    }
    // From here on, as the settings say.
    main.updates.download_by_itself = main.preferences.when_out == WhenOut::Download;
}

/// The update's states past finding one, which [`stage_update`] goes to.
const STAGED: [&str; 14] = [
    "updates-downloading",
    "updates-download-failed",
    "updates-verifying",
    "updates-mismatch",
    "updates-ready",
    "updates-ready-running",
    "updates-ask",
    "updates-ready-away",
    "updates-ask-away",
    "updates-blocked",
    "updates-installing",
    "updates-declined",
    "updates-launch-failed",
    "updates-installed",
];

/// Takes a newer Guesthouse that's out as far as `state` says, as the app would get there, step by
/// step, with what each step would have done left undone. Its machines are as each state needs:
/// none running when it's ready, two running when an install would stop them, and one being made
/// when an install has to wait.
fn stage_update(main: &mut Main, state: &str) {
    let running = matches!(state, "updates-ready-running" | "updates-ask");
    if let Ok(status) = &mut main.seen {
        for machine in &mut status.machines {
            machine.state = match machine.name.as_str() {
                "win11" | "build-box" if running => State::Running,
                "office" if state == "updates-blocked" => State::Installing,
                _ => State::Off,
            };
        }
    }
    updates(main, Some(Fetched::Text(release_file(&next_version()))));
    main.install_ask = matches!(state, "updates-ask" | "updates-ask-away");
    if state.ends_with("-away") {
        // A service of another version, say, which doesn't answer: what an update puts right.
        main.seen = Err(Unreached::NoService.words().to_owned());
    }
    let mut go = |input| main.updates.step(input, NOW);
    let [update::Effect::Download { attempt, ref latest }] = go(update::Input::Download)[..] else { return };
    let sha = latest.sha256.clone();
    let size = 68 << 20;
    match state {
        "updates-downloading" => {
            go(update::Input::Progress { attempt, got: 30 << 20, total: Some(size) });
            return;
        }
        "updates-download-failed" => {
            go(update::Input::Downloaded { attempt, result: Err(DownloadError::Unreached) });
            return;
        }
        _ => {}
    }
    go(update::Input::Downloaded { attempt, result: Ok(()) });
    match state {
        "updates-verifying" => return,
        "updates-mismatch" => {
            go(update::Input::Hashed { attempt, result: Ok("0".repeat(64)) });
            return;
        }
        _ => {}
    }
    go(update::Input::Hashed { attempt, result: Ok(sha) });
    let install = match state {
        "updates-installing" => None,
        "updates-declined" => Some(Ok(update::INSTALLER_CANCELLED)),
        "updates-launch-failed" => Some(Err("The system cannot find the file specified".to_owned())),
        "updates-installed" => Some(Ok(update::INSTALLER_DONE)),
        _ => return,
    };
    let [update::Effect::Launch { attempt, .. }] = go(update::Input::Install)[..] else { return };
    match install {
        None => {}
        Some(Ok(code)) => {
            go(update::Input::Launched { attempt, result: Ok(()) });
            go(update::Input::InstallerEnded { attempt, code });
        }
        Some(Err(why)) => {
            go(update::Input::Launched { attempt, result: Err(why) });
        }
    }
}

/// When a preview's look was, in seconds since 1970.
const NOW: u64 = 1_795_000_000;

/// The version after this one, as a newer release would be.
fn next_version() -> String {
    let parts: Vec<u64> =
        guesthouse_control::WINDOWS_VERSION.trim().split('.').filter_map(|part| part.parse().ok()).collect();
    match parts[..] {
        [major, minor, _] => format!("{major}.{}.0", minor + 1),
        _ => "99.0.0".to_owned(),
    }
}

/// The Remote View group of the running Windows machine in one of its states, named `rv-...`. Its
/// buttons answer as a service would, through `remote_view::pretend`, and each request they would
/// have sent is written to a file, see [`asked`].
fn remote_view(main: &mut Main, state: &str) {
    let name = "win11";
    let stopped = matches!(state, "rv-off" | "rv-stopped" | "rv-refused");
    show(main, name, if stopped { State::Off } else { State::Running });
    let on = !matches!(state, "rv-off" | "rv-refused");
    if let Ok(status) = &mut main.seen
        && let Some(machine) = status.machines.iter_mut().find(|machine| machine.name == name)
    {
        machine.remote_view = on;
        machine.remote_port = Some(7792);
        if state == "rv-bridged" {
            machine.network_mode = NetworkMode::Bridged { adapter: "Ethernet".to_owned() };
        }
    }
    // The draft was made from the machine before its network changed here.
    main.follow_draft();
    let now = super::now_ms();
    let fingerprint = |head: &str| format!("{head}{}", "0".repeat(64 - head.len()));
    let device = |name: &str, head: &str, connected: bool, clipboard: bool| DeviceInfo {
        fingerprint: fingerprint(head),
        name: name.to_owned(),
        connected,
        permissions: Permissions { clipboard, files: clipboard, ..Permissions::default() },
    };
    let listening = ShareInfo {
        state: ShareState::Listening,
        port: Some(7792),
        fingerprint: Some("9f2c41ab07d3e85c11aa22bb33cc44dd55ee66ff77008899aabbccddeeff0011".to_owned()),
        ..ShareInfo::off(name)
    };
    let info = match state {
        "rv-off" | "rv-refused" => ShareInfo { port: Some(7792), ..ShareInfo::off(name) },
        "rv-stopped" | "rv-starting" => ShareInfo { state: ShareState::Starting, ..listening },
        "rv-pairing" => ShareInfo { pairing: true, pairing_until_ms: Some(now + 95_000), ..listening },
        "rv-waiting" => ShareInfo {
            pending: Some(Pending {
                name: "Phone".to_owned(),
                fingerprint: fingerprint("7c8d41ab"),
                code: "482 917".to_owned(),
                seconds: 52,
            }),
            ..listening
        },
        "rv-devices" => ShareInfo {
            closed_reason: Some(CloseReason::Paired),
            paired: Some("Phone".to_owned()),
            devices: vec![
                device("Living room laptop", "1a2b41ab", false, true),
                device("Phone", "7c8d41ab", false, false),
            ],
            ..listening
        },
        "rv-viewing" => ShareInfo {
            viewer: Some("Phone".to_owned()),
            viewer_placement: Some("only".to_owned()),
            devices: vec![
                device("Phone", "7c8d41ab", true, false),
                device("Living room laptop", "1a2b41ab", false, true),
            ],
            ..listening
        },
        "rv-failed" => ShareInfo {
            state: ShareState::Failed,
            reason: Some("Port 7792 is already in use by another program.".to_owned()),
            devices: vec![device("Phone", "7c8d41ab", false, false)],
            ..listening
        },
        "rv-unlisted" => ShareInfo {
            reason: Some(
                "It isn't listed by name on the network, as something else has that name. Its address and port \
                 still work."
                    .to_owned(),
            ),
            ..listening
        },
        // rv-ready, rv-bridged
        _ => listening,
    };
    let mut viewing = Viewing::new(name);
    viewing.take(info, Some(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20))), now);
    if state == "rv-refused" {
        viewing.refused = refusal(&Ask::Switch(true));
    }
    main.viewing = Some(viewing);
}

/// What the service says to `ask` in a state that refuses it: `rv-refused` turns the switch on for a
/// machine with no network.
pub fn refusal(ask: &Ask) -> Option<String> {
    let refused = std::env::var(STATE).is_ok_and(|state| state == "rv-refused");
    (refused && *ask == Ask::Switch(true)).then(|| {
        "win11 has no network, so no other device can reach it. Turn its network on first.".to_owned()
    })
}

/// Where a preview writes the requests its Remote View buttons would have sent, a line each,
/// so a test can press them and read what they asked.
pub fn asked_file() -> std::path::PathBuf {
    std::env::temp_dir().join("guesthouse-app").join("preview-requests.txt")
}

/// Writes a request a preview's button would have sent.
pub fn asked(request: &guesthouse_control::Request) {
    use std::io::Write;
    let path = asked_file();
    if let Some(folder) = path.parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{request:?}");
    }
}

fn show(main: &mut Main, name: &str, state: State) {
    if let Ok(status) = &mut main.seen
        && let Some(machine) = status.machines.iter_mut().find(|machine| machine.name == name)
    {
        machine.state = state;
    }
    main.page = Page::Machine;
    main.back_to = Page::Machine;
    main.selected = Some(name.to_owned());
    main.follow_draft();
}

fn status() -> Status {
    let machine = |name: &str, system, state, set_up, card: Option<&str>, address: Option<&str>| Machine {
        name: name.to_owned(),
        system,
        state,
        set_up,
        memory_mb: 16384,
        processors: 8,
        card: card.map(str::to_owned),
        chosen_card: card.map_or(Card::None, |card| Card::Named(card.to_owned())),
        nvapi_wrapper: true,
        tpm: system == System::Windows,
        network: true,
        shares: Vec::new(),
        // The Windows machine has two forwards, so the section has rows to look at.
        network_mode: NetworkMode::Shared,
        forwards: if name == "win11" {
            vec![Forward { host_port: 8080, guest_port: 80 }, Forward { host_port: 3390, guest_port: 3389 }]
        } else {
            Vec::new()
        },
        nested_virtualization: false,
        ssh_port: (system == System::Linux).then_some(2201),
        size_gb: Some(256),
        folder: std::path::PathBuf::from(r"C:\ProgramData\Guesthouse\machines"),
        problem: None,
        // The stopped Windows machine has two, so the section has rows to look at.
        snapshots: if name == "win11" {
            vec![
                SnapshotInfo { id: 1, label: "Set up".to_owned(), taken: 1_795_000_000, current: false },
                SnapshotInfo {
                    id: 2,
                    label: "Before the driver update".to_owned(),
                    taken: 1_795_600_000,
                    current: true,
                },
            ]
        } else {
            Vec::new()
        },
        working: None,
        address: address.map(str::to_owned),
        remote_view: false,
        remote_port: None,
        console_pipe: None,
    };
    let card = |name: &str, memory_mb, default| CardInfo { name: name.to_owned(), memory_mb, default };
    Status {
        version: "0.7.1".to_owned(),
        machines: vec![
            machine(
                "win11",
                System::Windows,
                State::Off,
                true,
                Some("NVIDIA GeForce RTX 5090"),
                Some("172.24.80.12"),
            ),
            machine(
                "ubuntu",
                System::Linux,
                State::Off,
                true,
                Some("AMD Radeon RX 7900 XTX"),
                Some("172.24.80.31"),
            ),
            machine("build-box", System::Linux, State::Off, false, Some("NVIDIA GeForce RTX 5090"), None),
            machine("office", System::Windows, State::Installing, false, None, None),
        ],
        cards: vec![
            card("NVIDIA GeForce RTX 5090", Some(32607), true),
            card("AMD Radeon RX 7900 XTX", Some(24560), false),
            card("Intel(R) UHD Graphics 770", None, false),
        ],
        bridging: Bridging {
            available: true,
            reason: None,
            adapters: vec![
                AdapterInfo {
                    name: "Ethernet".to_owned(),
                    description: "Realtek PCIe 2.5GbE".to_owned(),
                    wifi: false,
                },
                AdapterInfo {
                    name: "Wi-Fi".to_owned(),
                    description: "Intel Wi-Fi 6E AX211".to_owned(),
                    wifi: true,
                },
            ],
            hyper_v: HyperV::On,
        },
    }
}

/// A prepared report, with nothing on disk behind it.
fn report() -> Prepared {
    let mut summary = Summary::default();
    summary.set("version", "0.10.0");
    summary.set("host", "windows");
    summary.set("os", "Windows 11 Pro 24H2 build 26100");
    summary.set("card", "NVIDIA GeForce RTX 5090");
    summary.set("machine", "windows");
    summary.set("kind", "wont-start");
    let files = [
        ("service.log", 41_207),
        ("setup.log", 8_933),
        ("windows.txt", 120),
        ("services.txt", 1_804),
        ("virtual-machine-platform.txt", 1_120),
        ("hyper-v-events.txt", 6_412),
        ("wsl.txt", 96),
        ("machines.txt", 138),
        ("machine.json", 214),
        ("machine.guesthouse.json", 133),
        ("snapshots/index.json", 402),
        ("events.log", 18_550),
        ("app.txt", 33),
        ("description.txt", 56),
        ("summary.json", 340),
        ("manifest.txt", 1_030),
    ];
    let reports = r"C:\Users\you\AppData\Local\Temp\guesthouse-app\reports";
    Prepared {
        archive: std::path::PathBuf::from(format!(r"{reports}\guesthouse-report-20261123-141500.tar.gz")),
        files: files.iter().map(|(name, bytes)| ((*name).to_owned(), *bytes)).collect(),
        missing: vec![
            ("crashes.txt".to_owned(), "wevtutil.exe failed (exit code: 5): Access is denied.".to_owned()),
            ("events.1.log".to_owned(), "The machine has no older events.".to_owned()),
            ("panic.txt".to_owned(), "There is none.".to_owned()),
        ],
        summary,
        bytes: 23_419,
    }
}

fn account(machine: &str) -> Account {
    Account {
        machine: machine.to_owned(),
        user: "guesthouse".to_owned(),
        password: "kettle-amber-74-orbit".to_owned(),
    }
}

fn guest_log() -> Vec<Line> {
    vec![
        note("Starting."),
        note("Giving it AMD Radeon RX 7900 XTX."),
        line(Kind::Guest, "guesthouse: dxgkrnl loaded"),
        line(Kind::Guest, "guesthouse: card: AMD Radeon RX 7900 XTX"),
        line(Kind::Guest, "guesthouse: address: 172.24.80.31"),
        note("It's running."),
    ]
}

fn log(lines: Vec<Line>) -> Log<Line> {
    let mut log = super::new_log();
    log.extend(lines);
    log
}

fn note(text: &str) -> Line {
    line(Kind::Note, text)
}

fn line(kind: Kind, text: &str) -> Line {
    Line { kind, text: text.to_owned() }
}
