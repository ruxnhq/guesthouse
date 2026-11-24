//! Made-up machines in place of the service's, for looking at each of the window's states without a
//! service or a GPU. Only a build with the `preview` feature has it. GUESTHOUSE_APP_PREVIEW names
//! the state, and GUESTHOUSE_APP_THEME may say `light` or `dark`:
//!
//! ```powershell
//! cargo build -p guesthouse-app-windows --features preview
//! $env:GUESTHOUSE_APP_PREVIEW = "running"; .\target\debug\guesthouse-app.exe
//! ```
//!
//! The states are `running`, `linux`, `starting`, `empty`, `away`, `refused`, `new`, `making`,
//! `failed`, `delete`, `settings` (a stopped machine's settings being changed) and `settings-refused`
//! (the service said no to them), `report-new` (the report page, being written), `report` (a
//! prepared report, and `report-sent` once it has an ID) and `report-away` (the report page when the
//! service can't be reached). Nothing is sent to the service, whatever is pressed.

use guesthouse_control::client::Unreached;
use guesthouse_control::{
    AdapterInfo, Bridging, Card, CardInfo, Forward, Machine, NetworkMode, SnapshotInfo, State, System,
};
use windows_reactor::WindowTheme;

use super::{Account, Form, Kind, Line, Main, Making, Page, Ready, ReportStage};
use crate::bridge::Status;
use crate::report::Prepared;
use guesthouse_control::report::Summary;

const STATE: &str = "GUESTHOUSE_APP_PREVIEW";
const THEME: &str = "GUESTHOUSE_APP_THEME";

pub fn wanted() -> bool {
    std::env::var_os(STATE).is_some()
}

/// Puts the state the variable names into `main`, which then asks nothing of the service.
pub fn stage(main: &mut Main) {
    let state = std::env::var(STATE).unwrap_or_default();
    main.live = false;
    main.theme = match std::env::var(THEME).as_deref() {
        Ok("light") => WindowTheme::Light,
        Ok("dark") => WindowTheme::Dark,
        _ => WindowTheme::System,
    };
    main.seen = Ok(status());
    match state.as_str() {
        "linux" => {
            show(main, "ubuntu", State::Running);
            main.account = Some(account("ubuntu"));
            main.log = guest_log();
        }
        "starting" => {
            show(main, "build-box", State::Starting);
            main.log = vec![
                note("Starting."),
                note("Its first boot sets it up without the card."),
                line(Kind::Guest, "Ubuntu 26.04 installing: copying files (41%)"),
            ];
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
                lines: vec![
                    note("Reading the image."),
                    note("Windows 11 Pro, from install.wim."),
                    note("Making a 128 GB disk."),
                    line(Kind::Problem, "The disk's folder is compressed, which makes it slower."),
                    note("Applying Windows to the disk. This takes a few minutes."),
                ],
                ended,
            });
        }
        "settings" | "settings-refused" => {
            show(main, "win11", State::Stopped);
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
            show(main, "win11", State::Stopped);
            main.update = Some(guesthouse_control::updates::Latest {
                version: "0.10.0".to_owned(),
                installer: "https://files.ruxn.com/p/guesthouse/guesthouse-setup-0.10.0.exe".to_owned(),
                sha256: "0".repeat(64),
                notes: String::new(),
            });
        }
        "report-new" => {
            show(main, "win11", State::Stopped);
            main.page = Page::Report;
            main.report.machine = Some("win11".to_owned());
            main.report.what = "It stays black after the logo, and the log says nothing.".to_owned();
        }
        "report" | "report-sent" => {
            show(main, "win11", State::Stopped);
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
            show(main, "ubuntu", State::Stopped);
            main.deleting = Some("ubuntu".to_owned());
        }
        _ => {
            show(main, "win11", State::Running);
            main.account = Some(account("win11"));
            main.log = vec![
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
            ];
        }
    }
}

fn show(main: &mut Main, name: &str, state: State) {
    if let Ok(status) = &mut main.seen
        && let Some(machine) = status.machines.iter_mut().find(|machine| machine.name == name)
    {
        machine.state = state;
    }
    main.page = Page::Machine;
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
        share: None,
        // The Windows machine has two forwards, so the section has rows to look at.
        network_mode: NetworkMode::Shared,
        forwards: if name == "win11" {
            vec![Forward { host_port: 8080, guest_port: 80 }, Forward { host_port: 3390, guest_port: 3389 }]
        } else {
            Vec::new()
        },
        size_gb: Some(256),
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
    };
    let card = |name: &str, memory_mb, default| CardInfo { name: name.to_owned(), memory_mb, default };
    Status {
        version: "0.7.1".to_owned(),
        machines: vec![
            machine(
                "win11",
                System::Windows,
                State::Stopped,
                true,
                Some("NVIDIA GeForce RTX 5090"),
                Some("172.24.80.12"),
            ),
            machine(
                "ubuntu",
                System::Linux,
                State::Stopped,
                true,
                Some("AMD Radeon RX 7900 XTX"),
                Some("172.24.80.31"),
            ),
            machine("build-box", System::Linux, State::Stopped, false, Some("NVIDIA GeForce RTX 5090"), None),
            machine("office", System::Windows, State::Preparing, false, None, None),
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

fn note(text: &str) -> Line {
    line(Kind::Note, text)
}

fn line(kind: Kind, text: &str) -> Line {
    Line { kind, text: text.to_owned() }
}
