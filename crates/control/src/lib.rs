//! The words between Guesthouse's app and the Guesthouse service on a Windows host. The service runs
//! as the system, as HCS needs, and the app runs as its user, so the app never touches a machine
//! itself: it asks the service, on the service's pipe, and shows what the service says.
//!
//! Each connection carries one request, a line of JSON, and its answer. Most answers are one line.
//! `Create` and `Watch` answer with a line for each event, as it happens, until the work ends or the
//! app closes the pipe. The service keeps every rule; the app only asks and shows.
//!
//! Nothing here calls into Windows but the pipe, which std opens as a file, so it builds and is
//! tested anywhere.

pub mod client;
pub mod events;
pub mod report;
pub mod screen;
pub mod updates;

pub use events::{Event, State};

use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Read, Write};
use std::path::PathBuf;

/// The Windows host's version: the service's, the app's and the installer's, from
/// `windows/version.txt`. It's a line of its own, apart from the Linux host's in the workspace's
/// `Cargo.toml`, so each host is released when it has something new.
pub const WINDOWS_VERSION: &str = include_str!("../../../windows/version.txt");

/// The service's pipe, which only the system, administrators and the signed-in user may open.
pub const PIPE: &str = r"\\.\pipe\guesthouse";
/// The longest request the service reads. Requests are short, and a longer one isn't from the app.
pub const REQUEST_LIMIT: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "ask", rename_all = "snake_case")]
pub enum Request {
    /// The machines, the cards that can be shared, and the service's version.
    Status,
    /// Makes a new machine's disks from an ISO. Its events come back as it works, then `Done`.
    Create { machine: NewMachine },
    /// Boots a machine. The answer comes once it's begun; `Watch` follows it.
    Start { name: String },
    /// Asks a running machine's guest to shut down. Asked again, the machine is stopped at once.
    Stop { name: String },
    /// Stops a running machine at once, as pulling its power would, with no shutdown in the guest.
    End { name: String },
    /// What the remoteman viewer needs to show a Windows guest's screen.
    Screen { name: String },
    /// Deletes a machine that isn't running, with its disks.
    Delete { name: String },
    /// Lets a stopped machine's disk grow to `size_gb`. A disk only grows. The guest's last
    /// partition takes the room at its next start.
    Grow { name: String, size_gb: u64 },
    /// Takes a snapshot of a stopped machine that has been set up. The answer comes once it's begun;
    /// `Machine::working` says it's going on, and `Watch` how it ended.
    Snapshot { name: String, label: String },
    /// Gives a stopped machine its disk, firmware and TPM as they were in snapshot `id`. What it held
    /// since is gone. Begun as `Snapshot` is.
    Revert { name: String, id: u64 },
    /// Deletes snapshot `id`. Its space comes back once no other disk is made on it. Begun as
    /// `Snapshot` is.
    DeleteSnapshot { name: String, id: u64 },
    /// The guest's administrator and password, for Remote Desktop or SSH.
    Account { name: String },
    /// A machine's recent events, then each new one, until the app closes the pipe.
    Watch { name: String },
    /// Changes what a machine that isn't running is given at each boot.
    Change { name: String, settings: MachineSettings },
    /// The files a bug report is made from that only the service can read: its own log, facts about
    /// this PC and, for `machine` if one is named, that machine's record and events. Each comes as
    /// `ReportPart` lines, or `ReportMissing` if it couldn't be read, then `Done`. The service only
    /// hands them over. The app scrubs, packs and sends them.
    ReportFiles { machine: Option<String> },
}

/// What a machine is given at each boot, which can be changed while it's off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineSettings {
    pub memory_mb: u64,
    pub processors: u32,
    pub card: Card,
    /// Whether an NVIDIA guest gets Guesthouse's NVAPI wrapper, which DLSS needs, in place of
    /// NVIDIA's NVAPI. Some games' anti-cheat refuses it, as it isn't signed.
    pub nvapi_wrapper: bool,
    /// An app from before these sends none of them, which reads as a TPM, a network and no share.
    #[serde(default = "on")]
    pub tpm: bool,
    #[serde(default = "on")]
    pub network: bool,
    /// The folders the guest sees, in order. An app or service from before there could be more
    /// than one sends `share`, one or none, which reads as a list of it.
    #[serde(default, alias = "share", deserialize_with = "shares_or_share")]
    pub shares: Vec<Share>,
    /// Which network the machine's card is plugged into. An app from before bridging sends none,
    /// which reads as the shared one.
    #[serde(default)]
    pub network_mode: NetworkMode,
    /// The ports of this PC that lead to the machine's. An older app sends none.
    #[serde(default)]
    pub forwards: Vec<Forward>,
    /// Whether the guest may run virtual machines of its own. An app from before this sends none,
    /// which reads as off.
    #[serde(default)]
    pub nested_virtualization: bool,
}

/// What a new machine is made from and given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewMachine {
    /// Letters, digits, - and _. It names the machine's folder and its VM.
    pub name: String,
    /// A Windows ISO, or its install.wim or install.esd, or an Ubuntu Server ISO.
    pub image: PathBuf,
    pub memory_mb: u64,
    pub processors: u32,
    /// The disk's largest size; it only takes what the guest writes.
    pub size_gb: u64,
    pub card: Card,
    /// The Windows edition in the image, by name. Empty takes Pro if the image has it, and its
    /// first edition if not. An Ubuntu image has none.
    #[serde(default)]
    pub edition: String,
    /// The guest's administrator. Empty is `DEFAULT_USER`.
    #[serde(default)]
    pub user: String,
    #[serde(default = "on")]
    pub tpm: bool,
    #[serde(default = "on")]
    pub network: bool,
    /// The folders the guest sees, in order. An app or service from before there could be more
    /// than one sends `share`, one or none, which reads as a list of it.
    #[serde(default, alias = "share", deserialize_with = "shares_or_share")]
    pub shares: Vec<Share>,
}

/// How a machine's network card reaches the network.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum NetworkMode {
    /// Windows' own shared network for its machines: the guest has a private address that only this
    /// PC reaches, and its connection goes through this PC's. Works on every edition.
    #[default]
    Shared,
    /// The guest joins this PC's network by one of its physical adapters, with an address of its
    /// own from the router. Needs Hyper-V's switch, as `Bridging` says.
    Bridged { adapter: String },
}

/// One port of this PC (TCP) that is passed on to a port of the guest's, while the machine runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Forward {
    pub host_port: u16,
    pub guest_port: u16,
}

/// The most forwards a machine has.
pub const MOST_FORWARDS: usize = 16;

/// Why `forwards` can't be one machine's, if they can't: ports 1 to 65535, at most `MOST_FORWARDS`,
/// and no port of this PC twice. That no other machine or program has a port is the service's to
/// say.
pub fn forwards_refusal(forwards: &[Forward]) -> Option<String> {
    if forwards.len() > MOST_FORWARDS {
        return Some(format!("A machine has {MOST_FORWARDS} port forwards at most."));
    }
    for (place, forward) in forwards.iter().enumerate() {
        if forward.host_port == 0 || forward.guest_port == 0 {
            return Some("A port is 1 to 65535.".into());
        }
        if forwards[..place].iter().any(|before| before.host_port == forward.host_port) {
            return Some(format!("This PC's port {} is forwarded twice.", forward.host_port));
        }
    }
    None
}

/// What the service says of bridging a machine onto this PC's network.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Bridging {
    /// Whether Hyper-V's virtual switch is there to make the bridge with.
    pub available: bool,
    /// When it isn't, why, in words for the person.
    #[serde(default)]
    pub reason: Option<String>,
    /// The physical adapters that are up, Ethernet and Wi-Fi.
    #[serde(default)]
    pub adapters: Vec<AdapterInfo>,
}

/// A physical network adapter of this PC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterInfo {
    /// Its name in Windows' network settings, which a machine's `Bridged` names it by.
    pub name: String,
    /// What it is, as its driver says.
    pub description: String,
    /// Whether it's a Wi-Fi adapter, which often can't be bridged.
    pub wifi: bool,
}

/// The guest's administrator when none is named.
pub const DEFAULT_USER: &str = "guesthouse";

/// A folder of this PC's the guest sees: as a drive linked on its desktop in Windows, and at
/// /mnt/shared in Ubuntu, then /mnt/shared2 and on for the folders after the first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Share {
    pub folder: PathBuf,
    #[serde(default)]
    pub read_only: bool,
}

/// The most folders a machine shares: a Windows guest gives each a drive of its own, S: to Z:.
pub const MOST_SHARES: usize = 8;

/// Reads the folders a machine shares as a list, or as the one `share`, or none, that an app or a
/// service from before there could be more sends.
pub fn shares_or_share<'de, D: serde::Deserializer<'de>>(reader: D) -> Result<Vec<Share>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Either {
        Many(Vec<Share>),
        One(Option<Share>),
    }
    Ok(match Either::deserialize(reader)? {
        Either::Many(shares) => shares,
        Either::One(share) => share.into_iter().collect(),
    })
}

/// Why a machine can't share these folders, if it can't: there are too many, or one is there
/// twice, which Windows' paths are the same as whatever their letters' case.
pub fn shares_refusal(shares: &[Share]) -> Option<String> {
    if shares.len() > MOST_SHARES {
        return Some(format!("A machine shares {MOST_SHARES} folders at most."));
    }
    for (place, share) in shares.iter().enumerate() {
        if share.folder.as_os_str().is_empty() {
            return Some("A shared folder needs a path.".into());
        }
        let same = |other: &Share| {
            other.folder.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase()
                == share.folder.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase()
        };
        if shares[..place].iter().any(same) {
            return Some(format!("{} is shared twice.", share.folder.display()));
        }
    }
    None
}

/// Whether `name` can be a guest's administrator in both systems: Ubuntu's rule for a user, which
/// Windows takes too, and not one of Windows' own accounts, which are disabled.
pub fn valid_user(name: &str) -> bool {
    const LONGEST: usize = 20;
    const WINDOWS_OWN: [&str; 4] = ["administrator", "defaultaccount", "guest", "wdagutilityaccount"];
    let mut characters = name.chars();
    name.len() <= LONGEST
        && characters.next().is_some_and(|first| first.is_ascii_lowercase())
        && characters.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        && !WINDOWS_OWN.contains(&name)
}

/// The `ssh` line that reaches an Ubuntu guest through the port the service listens on, on this PC's
/// loopback, which needs no network in the guest.
pub fn ssh_line(port: u16, user: &str) -> String {
    format!("ssh -p {port} {user}@127.0.0.1")
}

/// Whether `edition` can name an edition: Windows' editions are letters, digits and spaces.
pub fn valid_edition(edition: &str) -> bool {
    const LONGEST: usize = 64;
    edition.len() <= LONGEST && edition.chars().all(|c| c.is_ascii_alphanumeric() || c == ' ')
}

/// Which card a machine is given.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "card", content = "name", rename_all = "snake_case")]
pub enum Card {
    /// The card with the most memory of its own, which is the discrete one whoever makes it.
    #[default]
    Default,
    /// The card of this name, as `Status` lists it.
    Named(String),
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "answer", rename_all = "snake_case")]
pub enum Answer {
    Status {
        version: String,
        machines: Vec<Machine>,
        cards: Vec<CardInfo>,
        /// A service from before bridging doesn't say, which reads as not available.
        #[serde(default)]
        bridging: Bridging,
    },
    /// Done as asked.
    Ok,
    /// Not done, and why, in words for the person.
    Refused {
        reason: String,
    },
    /// One of a machine's events, for `Create` and `Watch`.
    Event {
        event: Event,
    },
    /// The end of `Create`'s work: `reason` is `None` when it worked.
    Done {
        reason: Option<String>,
    },
    Account {
        user: String,
        password: String,
    },
    /// One piece of a report file's text, in a line of at most [`report::PIECE_BYTES`]. Pieces with
    /// the same name are the file's text, to be joined in the order they came.
    ReportPart {
        name: String,
        text: String,
    },
    /// A report file that couldn't be handed over, and why, in words for the person.
    ReportMissing {
        name: String,
        reason: String,
    },
    /// Where a guest's screen is and what the viewer proves itself and expects with, for
    /// [`screen::connect_command`]. The certificate and key are hex.
    Screen {
        address: String,
        port: u16,
        host_fingerprint: String,
        viewer_certificate: String,
        viewer_key: String,
    },
}

/// A machine as the service sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Machine {
    pub name: String,
    pub system: System,
    pub state: State,
    /// Whether its first boot has set it up, after which it boots with its card.
    pub set_up: bool,
    pub memory_mb: u64,
    pub processors: u32,
    /// The card it's given, by name, if any.
    pub card: Option<String>,
    /// The card as its settings say: the default, one by name, or none. A service from before
    /// settings could be changed doesn't say, and gives the default.
    #[serde(default)]
    pub chosen_card: Card,
    /// Whether an NVIDIA guest gets the NVAPI wrapper, which is on unless turned off.
    #[serde(default = "on")]
    pub nvapi_wrapper: bool,
    #[serde(default = "on")]
    pub tpm: bool,
    #[serde(default = "on")]
    pub network: bool,
    /// The folders the guest sees, in order. An app or service from before there could be more
    /// than one sends `share`, one or none, which reads as a list of it.
    #[serde(default, alias = "share", deserialize_with = "shares_or_share")]
    pub shares: Vec<Share>,
    #[serde(default)]
    pub network_mode: NetworkMode,
    #[serde(default)]
    pub forwards: Vec<Forward>,
    /// Whether the guest may run virtual machines of its own, which is off unless turned on.
    #[serde(default)]
    pub nested_virtualization: bool,
    /// The port on this PC's loopback that reaches an Ubuntu guest's SSH over a Hyper-V socket
    /// while it runs, with no network. Only an Ubuntu guest has one.
    #[serde(default)]
    pub ssh_port: Option<u16>,
    /// The largest its disk may grow to, if the service knows.
    #[serde(default)]
    pub size_gb: Option<u64>,
    /// Its snapshots, oldest first.
    #[serde(default)]
    pub snapshots: Vec<SnapshotInfo>,
    /// What the service is doing to it while it's stopped, such as taking a snapshot, if anything.
    /// Nothing else can be done to it meanwhile.
    #[serde(default)]
    pub working: Option<String>,
    /// The address the guest last reported, for Remote Desktop or SSH.
    pub address: Option<String>,
}

/// One of a machine's snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotInfo {
    pub id: u64,
    pub label: String,
    /// When it was taken, in seconds since 1970 in UTC.
    pub taken: u64,
    /// Whether the machine runs on from it now, as it does just after it's taken or gone back to.
    pub current: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum System {
    Windows,
    Linux,
}

/// A card that can be shared with a guest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardInfo {
    pub name: String,
    /// The memory it has of its own, in MB, if its driver says.
    pub memory_mb: Option<u64>,
    /// Whether it's the card a machine gets when none is named.
    pub default: bool,
}

fn on() -> bool {
    true
}

/// Reads one line of JSON, at most `REQUEST_LIMIT` long, as a `T`.
pub fn read_line<T: for<'de> Deserialize<'de>>(reader: &mut impl BufRead) -> io::Result<Option<T>> {
    let mut line = String::new();
    let read = reader.by_ref().take(REQUEST_LIMIT as u64).read_line(&mut line)?;
    if read == 0 {
        return Ok(None);
    }
    serde_json::from_str(line.trim_end())
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Writes `value` as one line of JSON, and flushes it so the other end has it at once.
pub fn write_line<T: Serialize>(writer: &mut impl Write, value: &T) -> io::Result<()> {
    let mut line = serde_json::to_string(value).map_err(io::Error::other)?;
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    writer.flush()
}

/// The IPv4 address in one of a guest's report lines, other than loopback. A Windows guest writes
/// `address: 172.29.1.2`; an Ubuntu guest writes what `ip -o address` says, such as
/// `address: 2: eth0    inet 172.29.1.2/20 ...`.
pub fn address_in(line: &str) -> Option<String> {
    let rest = line.strip_prefix("guesthouse: address: ").or_else(|| line.strip_prefix("address: "))?;
    rest.split(|c: char| c.is_whitespace() || c == '/')
        .find(|word| word.parse::<std::net::Ipv4Addr>().is_ok_and(|address| !address.is_loopback()))
        .map(str::to_owned)
}

/// HCS's ID for machine `name`, which also names its pipes. Every other program's machines are left
/// alone.
pub fn vm_id(name: &str) -> String {
    format!("guesthouse-{name}")
}

/// The pipe machine `name`'s console is served on: its own display, which HCS's worker serves for
/// Windows' Remote Desktop control to reach through RdpBase's named-pipe connector, for only the
/// user at the console and administrators.
pub fn console_pipe(name: &str) -> String {
    format!(r"\\.\pipe\{}-console", vm_id(name))
}

/// Whether `name` can name a machine: its folder and its VM take it as it is.
pub fn valid_name(name: &str) -> bool {
    const LONGEST: usize = 40;
    !name.is_empty()
        && name.len() <= LONGEST
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[cfg(test)]
mod tests {
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
    }

    #[test]
    fn a_machine_from_a_service_without_its_chosen_card_reads_as_default() {
        let machine: Machine = serde_json::from_value(json!({
            "name": "win", "system": "windows", "state": "stopped", "set_up": true,
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
        let older = json!({"name": "win", "system": "windows", "state": "stopped", "set_up": true,
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
    fn a_user_is_one_both_systems_take() {
        assert!(valid_user("guesthouse"));
        assert!(valid_user("a-b_2"));
        for name in ["", "Admin", "2a", "a b", "administrator", "guest", "averyveryverylongusername"] {
            assert!(!valid_user(name), "{name}");
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
}
