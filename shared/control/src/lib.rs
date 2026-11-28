//! The words between Guesthouse's app and the Guesthouse service on a Windows host. The service runs
//! as the system, as HCS needs, and the app runs as its user, so the app never touches a machine
//! itself: it asks the service, on the service's pipe, and shows what the service says.
//!
//! Each connection carries one request, a line of JSON, and its answer. Most answers are one line.
//! `Create` and `Watch` answer with a line for each event, as it happens, until the work ends or the
//! app closes the pipe. The service keeps every rule; the app only asks and shows. Each request,
//! and the service's status, carry the pipe's version, and an app and a service of different
//! versions refuse each other in words that say so (see [`protocol`]).
//!
//! Nothing here calls into Windows but the pipe, which std opens as a file, so it builds and is
//! tested anywhere.

pub mod client;
pub mod events;
pub mod protocol;
pub mod report;
pub mod screen;
pub mod updates;

pub use events::{Event, State};
pub use guesthouse_remote_view_core::share::ShareInfo;

use serde::{Deserialize, Serialize};
use std::io::{self, BufRead};
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
    /// Moves a stopped machine's disks to `folder`, where they go in a folder named for the machine.
    /// An empty `folder` moves them back to the service's own folder, where machines are kept unless
    /// they're given another place. It's begun as `Snapshot` is: the answer comes once it's begun,
    /// `Machine::working` says it's going on, and `Watch` has its progress and how it ended. The
    /// machine stays where it was until the copy has been checked.
    Move { name: String, folder: PathBuf },
    /// The files a bug report is made from that only the service can read: its own log, facts about
    /// this PC and, for `machine` if one is named, that machine's record and events. Each comes as
    /// `ReportPart` lines, or `ReportMissing` if it couldn't be read, then `Done`. The service only
    /// hands them over. The app scrubs, packs and sends them.
    ReportFiles { machine: Option<String> },
    /// Turns a machine's viewing from other devices on or off (`on`), and answers with `Share`.
    /// Without `on` it changes nothing and answers the same. It works whether the machine runs or
    /// not; a running machine opens or closes at once. Turning it off never cuts Guesthouse's own
    /// window onto the machine. `pairing` opens (or closes) pairing for `pairing_seconds`, the
    /// shared crate's default when not said.
    Share {
        name: String,
        #[serde(default)]
        on: Option<bool>,
        #[serde(default)]
        pairing: Option<bool>,
        #[serde(default)]
        pairing_seconds: Option<u32>,
    },
    /// Where a machine's viewing from other devices stands now, answered with `Share`. The app asks
    /// it every few seconds while it shows it.
    RemoteView { name: String },
    /// Closes an open pairing at once. A device waiting is turned away. Answered with `Share`.
    PairClose { name: String },
    /// Lets in the device waiting to pair, whose code the person compared. Its clipboard and files
    /// go off as soon as it's seen. Answered with `Share`.
    PairAccept { name: String },
    /// Turns away the device waiting to pair. Answered with `Share`.
    PairDecline { name: String },
    /// Allows or refuses a paired device one thing: `what` is `input` (control; off is view-only),
    /// `clipboard`, `files`, `sound`, `mic` or `camera`. Answered with `Share`. There is no
    /// `device_drop` on this host: remoteman's `drop` cuts whichever device leads, which may be
    /// Guesthouse's own window.
    DeviceAllow { name: String, fingerprint: String, what: String, on: bool },
    /// Stops trusting a device: it's disconnected and has to pair again. Answered with `Share`.
    DeviceRevoke { name: String, fingerprint: String },
    /// Turns on Windows' Hyper-V feature, which bridging is made on, as [`HYPER_V_FEATURE`] names
    /// it. It's refused while a machine runs, is being made or has work going on, and while it's
    /// being turned on already. The service never restarts the PC: it answers with `Event` lines as
    /// it goes, then `Done`, whose reason is `None` once it's on or waits only for Windows to
    /// restart. Asked when it's on already, or waiting for a restart, it answers `Done` at once and
    /// changes nothing. A service from before this answers it with `protocol::NOT_A_REQUEST`.
    EnableHyperV,
    /// Gives `name`, a machine that needs an owner, to `owner`, a user's account as a SID. Only an
    /// administrator asking elevated may, so the app asks it from a helper that Windows asks the
    /// person to allow. `owner` is the app's own user, whoever allows it.
    Assign { name: String, owner: String },
}

/// The Windows feature `EnableHyperV` turns on, with its parents, as Microsoft's own instructions
/// for turning on Hyper-V with DISM name it.
pub const HYPER_V_FEATURE: &str = "Microsoft-Hyper-V";

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
    /// The folder the machine's disks are kept under, in a folder of their own named for the machine.
    /// An older app sends none, which keeps them in the service's own folder, as does an empty one.
    #[serde(default)]
    pub folder: Option<PathBuf>,
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
    /// When it isn't, why, in words for the person. An app from before `hyper_v` shows this.
    #[serde(default)]
    pub reason: Option<String>,
    /// The physical adapters that are up, Ethernet and Wi-Fi.
    #[serde(default)]
    pub adapters: Vec<AdapterInfo>,
    /// Where Windows' Hyper-V feature stands, which bridging needs. A service from before this
    /// says nothing, and a case this app doesn't know reads as nothing said too, so the rest of the
    /// status still reads: either way the app falls back to `reason`.
    #[serde(default, deserialize_with = "hyper_v_or_unsaid")]
    pub hyper_v: HyperV,
}

/// Where Windows' Hyper-V feature stands on this PC, as the service finds it, each case one the app
/// words on its own.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum HyperV {
    /// Its virtual machine management service is there, so a machine can be bridged.
    On,
    /// Off, and this edition has it, so it can be turned on.
    Off,
    /// This edition of Windows doesn't have it: DISM doesn't know the feature, as on Windows Home.
    NotInEdition,
    /// Turned on, and waiting for Windows to restart.
    RestartPending,
    /// No hypervisor is running, and the processor says virtualization is off in the firmware, or
    /// it has none. Turning Hyper-V on wouldn't make it run.
    NoVirtualization,
    /// Being turned on now, `percent` of the way as DISM last said.
    TurningOn {
        #[serde(default)]
        percent: Option<u8>,
    },
    /// Still off, as the last try to turn it on failed: why, and DISM's log, if it has one.
    Failed {
        reason: String,
        #[serde(default)]
        log: Option<String>,
    },
    /// Where it stands couldn't be told, and why.
    Unknown { reason: String },
    /// Not said: a service from before this, or a case newer than the app. Never sent.
    #[default]
    Unsaid,
}

/// Reads `hyper_v` as it was sent, or as [`HyperV::Unsaid`] when it's a case this app doesn't know.
fn hyper_v_or_unsaid<'de, D: serde::Deserializer<'de>>(reader: D) -> Result<HyperV, D::Error> {
    let value = serde_json::Value::deserialize(reader)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
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

/// Why `name` won't do as a new machine's user, before its image says which system it is: the
/// common rules' reason, if neither a Windows nor an Ubuntu guest takes it. Empty is `DEFAULT_USER`,
/// which both take. The guest's own rule is asked once the image is opened. A machine that has its
/// account already is never checked again.
pub fn user_problem(name: &str) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    guesthouse_account_core::common::either_name_problem(name)
}

/// The `ssh` line that reaches an Ubuntu guest through the port the service listens on, on this PC's
/// loopback, which needs no network in the guest.
pub use guesthouse_basics::words::ssh_line;

/// Whether `edition` can name an edition: Windows' editions are letters, digits and spaces.
pub use guesthouse_settings_core::windows_host::valid_edition;

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
        /// The pipe's version, `protocol::PROTOCOL`. A service from before it says none.
        #[serde(default = "protocol_before")]
        protocol: u32,
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
    /// Where a machine's viewing from other devices stands, for `Share` and `RemoteView`. The
    /// address a device types in is this PC's on its network with `info.port`; the app finds it
    /// itself with `guesthouse_relay::lan_address`, as the user, since it's the same PC's.
    Share {
        info: ShareInfo,
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
    /// The folder its disks are in. A service from before machines could be kept elsewhere says none.
    #[serde(default)]
    pub folder: PathBuf,
    /// Why its disks can't be used now, if they can't: their drive is gone, or the folder isn't the
    /// service's own any more. It can't be started, snapshotted or moved until it's put right.
    #[serde(default)]
    pub problem: Option<String>,
    /// Its snapshots, oldest first.
    #[serde(default)]
    pub snapshots: Vec<SnapshotInfo>,
    /// What the service is doing to it while it's stopped, such as taking a snapshot, if anything.
    /// Nothing else can be done to it meanwhile.
    #[serde(default)]
    pub working: Option<String>,
    /// The address the guest last reported, for Remote Desktop or SSH.
    pub address: Option<String>,
    /// Whether it may be viewed from other devices while it runs. A service from before this says
    /// nothing, which reads as off.
    #[serde(default)]
    pub remote_view: bool,
    /// Its port on this PC's network for that, which it keeps once given, on or off.
    #[serde(default)]
    pub remote_port: Option<u16>,
    /// The pipe its console is served on while it runs: its own display, which HCS's worker serves
    /// for Windows' Remote Desktop control to reach through RdpBase's named-pipe connector, for only
    /// the user at the console and administrators. Its name is new each boot. None while it's off, or
    /// from a service from before this, which has no console to offer.
    #[serde(default)]
    pub console_pipe: Option<String>,
    /// Whether it has no owner yet: a machine from before owners that the service couldn't give to
    /// anyone. Every user at the console sees it, with its name, system and state only, and nothing
    /// can be done to it but `Assign`, unless they ask elevated.
    #[serde(default)]
    pub needs_owner: bool,
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

fn protocol_before() -> u32 {
    protocol::BEFORE
}

/// Reads one line of JSON, at most `REQUEST_LIMIT` long, as a `T`. Every host's JSON lines are in
/// `guesthouse_basics::json_lines`.
pub fn read_line<T: for<'de> Deserialize<'de>>(reader: &mut impl BufRead) -> io::Result<Option<T>> {
    guesthouse_basics::json_lines::read_line_within(reader, REQUEST_LIMIT)
}

/// Writes `value` as one line of JSON, and flushes it so the other end has it at once.
pub use guesthouse_basics::json_lines::write_line;

/// The IPv4 address in one of a guest's report lines, other than loopback.
pub use guesthouse_serial_words_core::windows_host::address_in;

/// HCS's ID for machine `name`, which also starts its pipes' names. Every other program's machines
/// are left alone.
pub fn vm_id(name: &str) -> String {
    format!("guesthouse-{name}")
}

/// Whether `name` can name a machine: its folder and its VM take it as it is.
pub use guesthouse_settings_core::windows_host::valid_name;

#[cfg(test)]
mod tests;
