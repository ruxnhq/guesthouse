//! The app: what it knows, and what each message changes. What's shown is in window.rs, and the
//! calls to the service are in bridge.rs.
//!
//! The window is the whole app, so it's a single component, run as the root. Closing it ends the
//! app, and the machines go on as they are, since the service runs them.
//!
//! A build with the `preview` feature can show made-up machines instead, for looking at the
//! window's states without a service: see app/preview.rs.

use std::net::IpAddr;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::Duration;

use guesthouse_control::client::Unreached;
use guesthouse_control::report::{ProblemKind, issue_link};
use guesthouse_control::{
    Answer, Bridging, Card, CardInfo, Event, Forward, HyperV, Machine, MachineSettings, NetworkMode,
    NewMachine, Share, State, System, forwards_refusal, shares_refusal, user_problem, valid_name,
};
use windows_reactor::*;

use crate::bridge::{self, Ended, Job, Looking, Seen, Watch, Window};
use crate::console::{self, Console};
use crate::log::Log;
use crate::palette::{self, Entry, Here, Now, Step};
use crate::preferences::{Preferences, Theme, WhenOut};
use crate::remote_view::{Ask, Facts, Viewing};
use crate::report::{self, Asked, Prepared};
use crate::update::{self, DownloadError, Fetched, Updater, Updates};
use guesthouse_control::ShareInfo;
use guesthouse_remote_view_core::share::ShareState;

#[cfg(feature = "preview")]
mod preview;

/// How long after a page opens its focus is moved, by when it has been laid out.
const FOCUS_AFTER: Duration = Duration::from_millis(100);
/// How often an app left open looks for a newer Guesthouse by itself.
const LOOK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// The most lines a log keeps. The oldest go first.
const LOG_LIMIT: usize = 500;
/// What a new machine is given unless the person changes it.
const MEMORY_MB: f64 = 8192.0;
const PROCESSORS: f64 = 4.0;
const SIZE_GB: f64 = 128.0;

/// The editions the form offers, by the name DISM gives each. The first, empty, takes Pro if the
/// image has it and its first edition if not.
pub const EDITIONS: [&str; 6] = [
    "",
    "Windows 11 Pro",
    "Windows 11 Home",
    "Windows 11 Pro for Workstations",
    "Windows 11 Education",
    "Windows 11 Enterprise",
];

/// Where a folder chosen in the picker goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FolderFor {
    /// The new machine's form.
    Form,
    /// A shared folder's row on the shown machine's page, by its place.
    Machine(usize),
    /// The new machine's form, for where its disks are kept.
    Location,
    /// The shown machine's page, for where to move its disks.
    Move,
}

/// The share a folder field and its read-only box say, once the field holds a path.
fn share_of(folder: &str, read_only: bool) -> Option<Share> {
    let folder = folder.trim().trim_matches('"');
    (!folder.is_empty()).then(|| Share { folder: PathBuf::from(folder), read_only })
}

/// What the app starts with.
#[derive(Clone, PartialEq)]
pub struct Start {
    /// The service's first answer, asked before the window opens, so the window doesn't start out
    /// saying the service isn't running.
    pub first: Seen,
}

impl Start {
    pub fn new() -> Start {
        // A preview never reaches the service, not even to ask how things stand.
        if !live() {
            return Start { first: Err(String::new()) };
        }
        Start { first: bridge::status() }
    }
}

/// Whether the machines shown are the service's, which they are but in a preview.
fn live() -> bool {
    #[cfg(feature = "preview")]
    return !preview::wanted();
    #[cfg(not(feature = "preview"))]
    true
}

/// What the window shows under its title bar.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    /// The machines, a card each, to go into.
    Home,
    /// The selected machine's page, gone into from Home and out of with Back.
    Machine,
    New,
    /// Report a problem, which can be reached when the service can't be.
    Report,
    /// The app's own settings, its updates and what it is, which can be reached when the service
    /// can't be, too.
    AppSettings,
}

pub use crate::shell::{Layout, Section};

/// The report page: what's been written so far, and how far the report has got.
#[derive(Clone)]
pub struct ReportPage {
    /// The machine the problem is with, by name, if any.
    pub machine: Option<String>,
    pub kind: ProblemKind,
    /// What went wrong, in the person's words.
    pub what: String,
    /// How to reach them, if they give one.
    pub contact: String,
    pub stage: ReportStage,
}

impl Default for ReportPage {
    fn default() -> Self {
        ReportPage {
            machine: None,
            kind: ProblemKind::default(),
            what: String::new(),
            contact: String::new(),
            stage: ReportStage::Writing { problem: None },
        }
    }
}

#[derive(Clone)]
pub enum ReportStage {
    /// Being written, and why the last try to make it failed, if it did.
    Writing { problem: Option<String> },
    /// Being made, on a thread of its own.
    Preparing,
    /// Made, and shown for the person to look at. Nothing has been sent.
    Ready(Ready),
}

/// A report that's been made.
#[derive(Clone)]
pub struct Ready {
    pub prepared: Prepared,
    /// Being sent now.
    pub sending: bool,
    /// The ID the report server gave it, once it's sent.
    pub id: Option<String>,
    /// Why it wasn't sent or saved, in words.
    pub problem: Option<String>,
    /// Where a copy was saved, if one was.
    pub saved: Option<String>,
}

/// One line of a log.
#[derive(Clone, PartialEq, Eq)]
pub struct Line {
    pub kind: Kind,
    pub text: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// What the service is doing, or where the machine stands.
    Note,
    /// Something that went wrong, shown so it stands out.
    Problem,
    /// What the guest wrote to its serial port.
    Guest,
    /// A line of the report a Windows guest left on its disk.
    Report,
}

impl Line {
    fn of(event: Event) -> Line {
        let (kind, text) = match event {
            Event::Note(text) => (Kind::Note, text),
            Event::Problem(text) => (Kind::Problem, text),
            Event::Guest(text) => (Kind::Guest, text),
            Event::Report(text) => (Kind::Report, text),
            Event::State(State::Failed) => (Kind::Problem, "It failed.".to_owned()),
            Event::State(state) => (Kind::Note, format!("It's {}.", state.name().to_lowercase())),
        };
        Line { kind, text }
    }
}

/// Milliseconds since 1970, which the service's times are in.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
}

/// An empty log, which keeps up to [`LOG_LIMIT`] lines.
pub fn new_log() -> Log<Line> {
    Log::new(LOG_LIMIT)
}

#[cfg(test)]
mod state_words_tests;

/// The guest's sign-in, asked for when the person wants it and forgotten when another machine is
/// chosen.
#[derive(Clone, PartialEq, Eq)]
pub struct Account {
    pub machine: String,
    pub user: String,
    pub password: String,
}

/// A line at the foot of the window.
#[derive(Clone, PartialEq, Eq)]
pub struct News {
    pub text: String,
    pub bad: bool,
}

/// How a machine's console stands, as its window last said.
#[derive(Clone, PartialEq, Eq)]
pub enum ConsoleState {
    /// Its pipe isn't there yet, as the machine has only just started.
    Waiting,
    Connecting,
    Connected,
    /// The session ended, or never began, and why, in words.
    Ended(String),
}

/// A machine's console, open in a window of its own.
pub struct OpenConsole {
    pub machine: String,
    pub state: ConsoleState,
    /// Which console the window's events are from. Those of one that was closed are dropped.
    id: u64,
    window: Console,
}

/// The new machine's form, as the person has filled it in so far.
#[derive(Clone, PartialEq)]
pub struct Form {
    pub name: String,
    pub image: String,
    pub memory_mb: Option<f64>,
    pub processors: Option<f64>,
    pub size_gb: Option<f64>,
    pub card: Card,
    /// The place in `EDITIONS` of the edition chosen.
    pub edition: usize,
    /// The guest's administrator. Empty is the service's default.
    pub user: String,
    pub tpm: bool,
    pub network: bool,
    /// The folder to share, as typed or picked. Empty shares none.
    pub share_folder: String,
    pub share_read_only: bool,
    /// The folder the machine's disks are kept in, as typed or picked. Empty is the service's own.
    pub location: String,
}

impl Default for Form {
    fn default() -> Self {
        Form {
            name: String::new(),
            image: String::new(),
            memory_mb: Some(MEMORY_MB),
            processors: Some(PROCESSORS),
            size_gb: Some(SIZE_GB),
            card: Card::Default,
            edition: 0,
            user: String::new(),
            tpm: true,
            network: true,
            share_folder: String::new(),
            share_read_only: false,
            location: String::new(),
        }
    }
}

impl Form {
    /// The machine to ask for, once every field holds something that can be sent. Whether the
    /// numbers suit this PC is the service's to say.
    pub fn machine(&self) -> Option<NewMachine> {
        let name = self.name.trim();
        let image = self.image.trim().trim_matches('"');
        let user = self.user.trim();
        if !valid_name(name) || image.is_empty() || user_problem(user).is_some() {
            return None;
        }
        Some(NewMachine {
            name: name.to_owned(),
            image: PathBuf::from(image),
            memory_mb: whole(self.memory_mb)?,
            processors: u32::try_from(whole(self.processors)?).ok()?,
            size_gb: whole(self.size_gb)?,
            card: self.card.clone(),
            edition: EDITIONS.get(self.edition).copied().unwrap_or_default().to_owned(),
            user: user.to_owned(),
            folder: Some(self.location.trim().trim_matches('"'))
                .filter(|folder| !folder.is_empty())
                .map(PathBuf::from),
            tpm: self.tpm,
            network: self.network,
            shares: share_of(&self.share_folder, self.share_read_only).into_iter().collect(),
        })
    }
}

/// What a machine is given at each boot, as its settings say now.
pub fn settings_of(machine: &Machine) -> MachineSettings {
    MachineSettings {
        memory_mb: machine.memory_mb,
        processors: machine.processors,
        card: machine.chosen_card.clone(),
        nvapi_wrapper: machine.nvapi_wrapper,
        tpm: machine.tpm,
        network: machine.network,
        shares: machine.shares.clone(),
        network_mode: machine.network_mode.clone(),
        forwards: machine.forwards.clone(),
        nested_virtualization: machine.nested_virtualization,
    }
}

/// The selected machine's settings as the person is changing them, before they're saved. It follows
/// the service while nothing has been changed, and goes back to what's saved once the machine starts,
/// as they can only be changed while it's off.
#[derive(Clone, PartialEq)]
pub struct Draft {
    pub machine: String,
    pub memory_mb: Option<f64>,
    pub processors: Option<f64>,
    pub card: Card,
    pub nvapi_wrapper: bool,
    pub tpm: bool,
    pub nested_virtualization: bool,
    pub network: bool,
    /// The folders to share, a row each, as the person is editing them.
    pub shares: Vec<ShareRow>,
    /// Whether the card is on the shared network or bridged onto an adapter of this PC's.
    pub network_mode: NetworkMode,
    /// The port forwards, a row each, as the person is editing them.
    pub forwards: Vec<ForwardRow>,
    /// The size the disk is to grow to, which is asked for on its own, with Grow, as it can't be
    /// undone.
    pub size_gb: Option<f64>,
    /// Asked for, and not answered yet.
    pub saving: bool,
    /// Why the service said no, until they're changed again.
    pub refused: Option<String>,
}

/// A shared folder as its field and its read-only box hold it: the field may be empty while it's
/// being typed.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct ShareRow {
    pub folder: String,
    pub read_only: bool,
}

/// A port forward as its two boxes hold it: either may be empty while it's being typed.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ForwardRow {
    pub host: Option<f64>,
    pub guest: Option<f64>,
}

impl ForwardRow {
    /// A new row, with neither port given yet.
    const NEW: ForwardRow = ForwardRow { host: None, guest: None };

    fn of(forward: &Forward) -> ForwardRow {
        ForwardRow { host: Some(f64::from(forward.host_port)), guest: Some(f64::from(forward.guest_port)) }
    }

    /// The forward the boxes say, once each holds a port.
    fn forward(&self) -> Option<Forward> {
        let port = |value: Option<f64>| {
            let value = whole(value)?;
            u16::try_from(value).ok().filter(|port| *port != 0)
        };
        Some(Forward { host_port: port(self.host)?, guest_port: port(self.guest)? })
    }
}

impl Draft {
    fn of(machine: &Machine) -> Draft {
        Draft {
            machine: machine.name.clone(),
            memory_mb: Some(machine.memory_mb as f64),
            processors: Some(machine.processors as f64),
            card: machine.chosen_card.clone(),
            nvapi_wrapper: machine.nvapi_wrapper,
            tpm: machine.tpm,
            nested_virtualization: machine.nested_virtualization,
            network: machine.network,
            shares: machine
                .shares
                .iter()
                .map(|share| ShareRow {
                    folder: share.folder.display().to_string(),
                    read_only: share.read_only,
                })
                .collect(),
            network_mode: machine.network_mode.clone(),
            forwards: machine.forwards.iter().map(ForwardRow::of).collect(),
            size_gb: machine.size_gb.map(|gb| gb as f64),
            saving: false,
            refused: None,
        }
    }

    /// The settings to ask for, once each number is one. Whether they suit this PC is the
    /// service's to say, as it is for a new machine.
    pub fn settings(&self) -> Option<MachineSettings> {
        Some(MachineSettings {
            memory_mb: whole(self.memory_mb)?,
            processors: u32::try_from(whole(self.processors)?).ok()?,
            card: self.card.clone(),
            nvapi_wrapper: self.nvapi_wrapper,
            tpm: self.tpm,
            network: self.network,
            shares: self.shares.iter().filter_map(|row| share_of(&row.folder, row.read_only)).collect(),
            network_mode: self.network_mode.clone(),
            forwards: self.forwards.iter().map(ForwardRow::forward).collect::<Option<Vec<_>>>()?,
            nested_virtualization: self.nested_virtualization,
        })
    }

    /// Why the forwards can't be saved as they are, if they can't, in the service's own words. A
    /// row with a port missing isn't one yet, and is said apart.
    pub fn forwards_problem(&self) -> Option<String> {
        let forwards: Vec<Forward> = self.forwards.iter().filter_map(ForwardRow::forward).collect();
        if forwards.len() < self.forwards.len() {
            return Some("Give each forward both ports, 1 to 65535.".to_owned());
        }
        forwards_refusal(&forwards)
    }

    /// Why the shared folders can't be saved as they are, if they can't, in the service's own
    /// words. A row with no folder yet is said apart.
    pub fn shares_problem(&self) -> Option<String> {
        let shares: Vec<Share> =
            self.shares.iter().filter_map(|row| share_of(&row.folder, row.read_only)).collect();
        if shares.len() < self.shares.len() {
            return Some("Give each shared folder a path, or remove it.".to_owned());
        }
        shares_refusal(&shares)
    }

    /// Why these can't be saved as they are, if they can't.
    pub fn problem(&self) -> Option<String> {
        self.forwards_problem().or_else(|| self.shares_problem())
    }

    /// Whether the person has changed something from what `machine`'s settings say.
    pub fn changed(&self, machine: &Machine) -> bool {
        // A shared folder's row with no path yet isn't in the settings, and is a change still.
        self.settings() != Some(settings_of(machine)) || self.shares.len() != machine.shares.len()
    }

    /// The size to grow `machine`'s disk to, if it's one it could grow to: larger than it is now,
    /// or any size when that isn't known, as the service says if it's too small.
    pub fn growth(&self, machine: &Machine) -> Option<u64> {
        let wanted = whole(self.size_gb)?;
        machine.size_gb.is_none_or(|now| wanted > now).then_some(wanted)
    }

    /// Whether a size to grow to has been typed that isn't the disk's now.
    fn size_typed(&self, machine: &Machine) -> bool {
        self.size_gb != machine.size_gb.map(|gb| gb as f64)
    }
}

/// Whether a card is NVIDIA's, by the name Windows gives it. The default is the card that's the
/// default now, and none is no one's.
pub fn nvidia(card: &Card, cards: &[CardInfo]) -> bool {
    let name = match card {
        Card::Default => cards.iter().find(|card| card.default).map(|card| card.name.as_str()),
        Card::Named(name) => Some(name.as_str()),
        Card::None => None,
    };
    name.is_some_and(|name| name.to_ascii_lowercase().contains("nvidia"))
}

/// Whether a machine's settings offer the DLSS switch: a Windows guest with an NVIDIA card, as the
/// wrapper is NVAPI's, which only NVIDIA's Windows driver has.
pub fn offers_dlss(system: System, card: &Card, cards: &[CardInfo]) -> bool {
    system == System::Windows && nvidia(card, cards)
}

/// A number field's value as a whole number, once it's one that can be sent.
fn whole(value: Option<f64>) -> Option<u64> {
    value.filter(|v| v.is_finite() && *v >= 1.0).map(|v| v.round() as u64)
}

/// The command palette, while it's open: what's typed, and which of what it finds is highlighted.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Palette {
    pub query: String,
    pub at: usize,
}

/// A snapshot whose Revert or Delete is being asked about, as each can't be undone.
#[derive(Clone, PartialEq, Eq)]
pub struct SnapshotAsk {
    pub machine: String,
    pub id: u64,
    pub label: String,
    /// Going back to it, or deleting it.
    pub revert: bool,
}

/// A new machine being made, or made: its events, and how it ended once it has.
#[derive(Clone, PartialEq, Eq)]
pub struct Making {
    pub name: String,
    pub lines: Log<Line>,
    /// `Some(None)` when it worked, `Some(Some(why))` when it didn't.
    pub ended: Option<Option<String>>,
}

pub struct Main {
    pub seen: Seen,
    pub page: Page,
    /// Where Back goes from New machine and Report a problem: Home, the machine's page or the
    /// app's settings they were opened from.
    pub back_to: Page,
    /// Where Back goes from the app's settings: Home, or the machine's page it was opened from.
    pub settings_back: Page,
    /// The part of the machine's page that shows.
    pub section: Section,
    /// How the window's width lays the pages out.
    pub layout: Layout,
    /// What takes the keyboard's focus when a page opens, so keys and a screen reader start there.
    pub focus: ElementRef,
    /// The page that last had the focus moved to it, and the timer that moves it once a new page
    /// has been laid out, as a control that isn't laid out yet can't take the focus.
    focused: Option<(Page, Option<String>, bool)>,
    focus_timer: Option<ComponentTimer>,
    /// The machine whose page is shown, by name, as the list's order can change under it.
    pub selected: Option<String>,
    /// The selected machine's log, from its watch.
    pub log: Log<Line>,
    /// The watch that fills the log, while it lasts.
    watch: Option<Watch>,
    /// Which watch the log is from. Messages from an older one are dropped.
    watch_id: u64,
    /// The watch ended because the service went away, so it starts again once the service is back.
    rewatch: bool,
    pub account: Option<Account>,
    pub news: Option<News>,
    /// The machine whose Delete is being asked about.
    pub deleting: Option<String>,
    /// Whether a newer Guesthouse is out, as the file drop last said.
    pub updates: Updates,
    /// What reaches the file drop: the network, or a preview's fake.
    updater: Arc<dyn Updater>,
    /// The timer that looks again a day after the last look by itself.
    look_timer: Option<ComponentTimer>,
    /// Stops the download that's going on, if one is.
    download_stop: Option<Arc<AtomicBool>>,
    /// Whether the person is being asked to confirm an install that stops the running machines.
    pub install_ask: bool,
    /// The installer has been started and wants the app gone, so the window is to close.
    closing: bool,
    /// The app's own settings.
    pub preferences: Preferences,
    /// Whether the app opens when the person signs in, as Windows' Run key says.
    pub opens_at_sign_in: bool,
    /// Whether this is an installed app, beside the installer's uninstaller. A preview counts as one.
    pub installed: bool,
    /// The snapshot whose Revert or Delete is being asked about.
    pub snapshot_ask: Option<SnapshotAsk>,
    /// The label typed for the next snapshot of the shown machine.
    pub snapshot_label: String,
    pub form: Form,
    pub report: ReportPage,
    /// The selected machine's settings, as the person is changing them.
    pub draft: Option<Draft>,
    /// Whether the person is being asked to confirm bridging the selected machine, which can drop
    /// this PC's own network for a few seconds.
    pub bridge_ask: bool,
    /// Whether the person is being asked to confirm turning on Windows' Hyper-V feature.
    pub hyper_v_ask: bool,
    /// Whether the app's request to turn it on is going on now.
    pub hyper_v_asking: bool,
    /// Whether the person is being asked to confirm restarting Windows, to finish it.
    pub restart_ask: bool,
    /// Whether Home's line about the restart has been closed, until the app next opens.
    pub restart_line_closed: bool,
    /// The command palette, Ctrl+K, while it's open.
    pub palette: Option<Palette>,
    /// Its box, which takes the keyboard's focus as it opens.
    pub palette_box: ElementRef,
    pub making: Option<Making>,
    /// The console's window, while one is open. There's one at a time.
    pub console: Option<OpenConsole>,
    console_id: u64,
    jobs: Sender<Job>,
    window: Window,
    /// The window's handle, for the file dialog and the clipboard.
    hwnd: isize,
    pub icon: Option<Rc<str>>,
    /// Light or dark, or as Windows is, as the app's settings say, or a preview chooses.
    pub theme: WindowTheme,
    /// Whether the machines are the service's. A preview's aren't, and nothing is asked of it.
    live: bool,
    /// The shown Windows machine's remote view.
    pub viewing: Option<Viewing>,
    /// What asks about it, while its page shows, and which of those it is.
    looking: Option<Looking>,
    looking_id: u64,
}

/// The key of Delete in a machine's More menu.
pub const DELETE_KEY: &str = "delete";

/// The key of New machine in the machine switcher. It has a space, which no machine's name can.
pub const NEW_TAG: &str = " new";

/// The key of Report a problem in a machine's More menu.
pub const REPORT_TAG: &str = " report";

#[derive(Clone)]
pub enum Message {
    Hwnd(isize),
    Status(Seen),
    Answered(Job, Result<Answer, Unreached>),
    /// Events that came close together, oldest first.
    Watched(u64, Vec<Event>),
    WatchEnded(u64, Ended),
    Creating(Event),
    Created(Option<String>),
    /// A machine was chosen in the switcher, by its name, or `NEW_TAG`. Its page keeps the
    /// section that showed.
    Navigate(Option<String>),
    /// A machine's card on Home was chosen: its page opens on its Overview.
    Open(String),
    /// Back: out of a machine's page to Home, or out of New machine or Report a problem to where
    /// they were opened from.
    Back,
    /// A section of the machine's page was chosen, by its name in the bar.
    Section(Option<String>),
    /// The window's width crossed into another layout.
    Layout(Layout),
    /// Move the keyboard's focus to the page that has opened.
    Focus,
    NewMachine,
    /// Open the report page, from the service-away page too.
    OpenReport,
    /// A machine was chosen for the report: 0 is none, then each in the list's order.
    ReportMachine(Option<usize>),
    /// A kind was chosen, by its place in `ProblemKind::ALL`.
    ReportKind(Option<usize>),
    ReportWhat(String),
    ReportContact(String),
    /// Make the report, which sends nothing.
    ReportPrepare,
    ReportPrepared(Result<Prepared, String>),
    ReportSend,
    ReportSent(Result<String, String>),
    /// Open Save As, and copy the archive where the person says.
    ReportSave,
    ReportSaved(Result<std::path::PathBuf, String>),
    /// Open a new issue with the sent report's ID in it.
    ReportIssue,
    /// Delete what was made and write another.
    ReportStartOver,
    Start,
    Stop,
    /// An item of a machine's More menu, by its key.
    More(Option<String>),
    DeleteAnswered(ContentDialogResult),
    SignIn,
    RemoteDesktop,
    /// Open the guest's screen in remoteman's viewer.
    Screen,
    /// Open the selected machine's console, or bring its window to the front.
    Console,
    /// Connect the console again, after its session ended.
    Reconnect,
    CloseConsole,
    /// What a console's window said, and which console it was.
    Consoled(u64, console::Event),
    Copy(String),
    WatchAgain,
    Name(String),
    Image(String),
    Browse,
    Picked(Option<PathBuf>),
    Memory(Option<f64>),
    Processors(Option<f64>),
    Size(Option<f64>),
    /// A card was chosen, by its place in the list: Default, each card, then None.
    Card(Option<usize>),
    /// An edition was chosen, by its place in `EDITIONS`.
    Edition(Option<usize>),
    User(String),
    Tpm(bool),
    Network(bool),
    ShareFolder(String),
    ShareReadOnly(Option<bool>),
    /// Where the new machine's disks are kept, as typed.
    Location(String),
    /// Move the machine's disks back to where the service keeps machines.
    MoveDefault,
    /// Open the folder picker, for the form or the machine's page.
    BrowseFolder(FolderFor),
    PickedFolder(FolderFor, Option<PathBuf>),
    Create,
    /// The selected machine's settings, as the person changes them.
    SetMemory(Option<f64>),
    SetProcessors(Option<f64>),
    /// By its place in `Main::settings_cards`.
    SetCard(Option<usize>),
    SetDlss(bool),
    SaveSettings,
    /// The answer to the question about bridging.
    BridgeAnswered(ContentDialogResult),
    UndoSettings,
    /// A network was chosen for the machine, by its place in `Main::network_choices`.
    SetNetworkMode(Option<usize>),
    AddForward,
    RemoveForward(usize),
    SetForwardHost(usize, Option<f64>),
    SetForwardGuest(usize, Option<f64>),
    SetTpm(bool),
    SetNested(bool),
    SetNetwork(bool),
    AddShare,
    RemoveShare(usize),
    SetShareFolder(usize, String),
    SetShareReadOnly(usize, Option<bool>),
    /// The size to grow the disk to, which Grow asks for.
    SetSize(Option<f64>),
    /// Ask the service to let the disk grow to the size in the field.
    Grow,
    SnapshotLabel(String),
    TakeSnapshot,
    /// Ask about going back to the snapshot with this ID.
    RevertSnapshot(u64),
    /// Ask about deleting the snapshot with this ID.
    DeleteSnapshot(u64),
    SnapshotAnswered(ContentDialogResult),
    NewsClosed,
    /// Open the app's settings.
    OpenSettings,
    /// A theme was chosen, by its place in `Theme::ALL`.
    SetTheme(Option<usize>),
    /// Look for a newer Guesthouse by itself, or not.
    SetLookByItself(bool),
    /// Open the app when the person signs in, or not.
    SetSignIn(bool),
    /// Look for a newer Guesthouse now.
    Look,
    /// A day has gone by since the app last looked by itself.
    LookDaily,
    /// What the look numbered here found.
    Looked(u64, Fetched),
    /// What's done when a look finds a newer one, by its place in `WhenOut::ALL`.
    SetWhenOut(Option<usize>),
    /// Download the newer one, or try again.
    GetUpdate,
    /// Stop the download, or its check.
    CancelUpdate,
    /// How far the download numbered here has got: bytes, of how many if the file drop said.
    UpdateProgress(u64, u64, Option<u64>),
    /// How the download numbered here ended.
    Downloaded(u64, Result<(), DownloadError>),
    /// The SHA-256 the check of the download numbered here found, or why it couldn't.
    Hashed(u64, Result<String, String>),
    /// Install the one that's ready, asking first if it stops running machines.
    Install,
    /// The answer to the question about the machines an install stops.
    InstallAnswered(ContentDialogResult),
    /// The installer of the install numbered here started, or didn't.
    Launched(u64, Result<(), String>),
    /// The installer of the install numbered here ended with this code.
    InstallerEnded(u64, i32),
    /// Put the update aside until the app next starts.
    LaterUpdate,
    /// Open a version's release notes.
    ReleaseNotes(String),
    /// Open the licences of what Guesthouse is built with.
    Licences,
    /// Turn on Windows' Hyper-V feature, which asks first.
    TurnOnHyperV,
    /// The answer to the question about turning it on.
    HyperVAnswered(ContentDialogResult),
    /// Turning it on has got further: the status says how far.
    HyperVNote,
    /// How turning it on ended: `None` when it worked.
    HyperVDone(Option<String>),
    /// Restart Windows, which asks first.
    RestartWindows,
    /// The answer to the question about restarting.
    RestartAnswered(ContentDialogResult),
    /// Home's line about the restart was closed.
    RestartLineClosed,
    /// Open the command palette, from Ctrl+K or the title bar's box.
    OpenPalette,
    /// What's typed in the palette's box.
    PaletteQuery(String),
    /// Up, Down or Enter in the palette's box.
    PaletteKey(palette::Key),
    /// Run the palette's entry with this key, as when it's clicked.
    PaletteRun(String),
    /// The palette closed, with Escape or once an entry ran.
    PaletteClosed,
    /// Move the keyboard's focus to the palette's box, once it's laid out.
    PaletteFocus,
    /// One of the Remote View group's controls.
    RemoteView(Ask),
    /// The service's answer to what was asked of the remote view, and which machine's it was.
    RemoteViewAnswered(u64, Ask, Result<ShareInfo, String>),
    /// How the remote view stands, with this PC's address on its network.
    RemoteViewSeen(u64, Result<ShareInfo, String>, Option<IpAddr>),
    /// Ask about removing the paired device with this fingerprint.
    RemoveDevice(String),
    RemoveAnswered(ContentDialogResult),
}

impl Main {
    pub fn status(&self) -> Option<&bridge::Status> {
        self.seen.as_ref().ok()
    }

    pub fn machine(&self) -> Option<&Machine> {
        let name = self.selected.as_deref()?;
        self.status()?.machines.iter().find(|machine| machine.name == name)
    }

    /// Whether the log is being filled.
    pub fn watching(&self) -> bool {
        self.watch.is_some()
    }

    /// Whether a new machine is being made now.
    pub fn busy(&self) -> bool {
        self.making.as_ref().is_some_and(|making| making.ended.is_none())
    }

    /// The cards the service offers, as it last said.
    pub fn cards(&self) -> &[CardInfo] {
        self.status().map(|status| status.cards.as_slice()).unwrap_or_default()
    }

    /// The cards the form offers, in the list's order.
    pub fn card_choices(&self) -> Vec<Card> {
        let named = self.status().into_iter().flat_map(|status| &status.cards);
        let named = named.map(|card| Card::Named(card.name.clone()));
        std::iter::once(Card::Default).chain(named).chain(std::iter::once(Card::None)).collect()
    }

    /// The cards a machine's settings offer: the form's, and the one it has by name if that card
    /// has gone from this PC, so the list can still say what it has.
    pub fn settings_cards(&self) -> Vec<Card> {
        let mut choices = self.card_choices();
        if let Some(draft) = &self.draft
            && !choices.contains(&draft.card)
        {
            choices.insert(choices.len() - 1, draft.card.clone());
        }
        choices
    }

    /// What the service says of bridging a machine onto this PC's network.
    pub fn bridging(&self) -> Option<&Bridging> {
        self.status().map(|status| &status.bridging)
    }

    /// Where Windows' Hyper-V feature stands, as the service last said, if it said.
    pub fn hyper_v(&self) -> HyperV {
        self.bridging().map(|bridging| bridging.hyper_v.clone()).unwrap_or_default()
    }

    /// Asks the service to turn Hyper-V on, once the person has said yes. A preview pretends it
    /// worked at once, and asks nothing.
    fn turn_on_hyper_v(&mut self) {
        if self.hyper_v_asking {
            return;
        }
        if !self.live {
            if let Ok(status) = &mut self.seen {
                status.bridging.hyper_v = HyperV::RestartPending;
                status.bridging.reason = Some(crate::hyper_v::HOME_LINE.to_owned());
            }
            let (words, bad) = crate::hyper_v::ended_words(None);
            self.tell(words, bad);
            return;
        }
        self.hyper_v_asking = true;
        bridge::enable_hyper_v(self.window.clone());
    }

    /// Restarts Windows, once the person has said yes twice. A preview never does.
    fn restart_windows(&mut self) {
        if !self.live {
            self.tell("A preview doesn't restart Windows.", false);
            return;
        }
        if let Err(words) = crate::system::restart_windows() {
            self.tell(words, true);
        }
    }

    /// The networks a machine's settings offer, in the list's order: the shared one, then each
    /// adapter if bridging is there. The one the draft has is kept in the list even if it has gone
    /// from this PC, so the list can still say what the machine has.
    pub fn network_choices(&self) -> Vec<NetworkMode> {
        let adapters =
            self.bridging().filter(|bridging| bridging.available).map(|bridging| &bridging.adapters);
        let named = adapters
            .into_iter()
            .flatten()
            .map(|adapter| NetworkMode::Bridged { adapter: adapter.name.clone() });
        let mut choices: Vec<NetworkMode> = std::iter::once(NetworkMode::Shared).chain(named).collect();
        if let Some(draft) = &self.draft
            && !choices.contains(&draft.network_mode)
        {
            choices.push(draft.network_mode.clone());
        }
        choices
    }

    /// Starts the selected machine's draft afresh from what the service says, unless the person
    /// has changed something while it's stopped.
    fn follow_draft(&mut self) {
        let Some(machine) = self.machine() else {
            self.draft = None;
            return;
        };
        let keep = self.draft.as_ref().is_some_and(|draft| {
            draft.machine == machine.name
                && machine.state == State::Off
                && (draft.saving
                    || draft.refused.is_some()
                    || draft.changed(machine)
                    || draft.size_typed(machine))
        });
        if !keep {
            self.draft = Some(Draft::of(machine));
        }
    }

    /// Changes the draft, which clears the service's last no.
    fn edit(&mut self, change: impl FnOnce(&mut Draft)) {
        if let Some(draft) = &mut self.draft
            && !draft.saving
        {
            change(draft);
            draft.refused = None;
        }
    }

    fn save_settings(&mut self) {
        let Some(machine) = self.machine() else { return };
        if machine.state != State::Off {
            return;
        }
        let Some(draft) = self.draft.as_ref().filter(|draft| !draft.saving) else { return };
        if draft.settings().is_none() || draft.problem().is_some() {
            return;
        }
        // Binding the bridge's switch to the adapter can drop this PC's own network for a few
        // seconds, so a machine's first bridging onto an adapter is asked about before it's kept.
        if matches!(draft.network_mode, NetworkMode::Bridged { .. })
            && draft.network_mode != machine.network_mode
        {
            self.bridge_ask = true;
            return;
        }
        self.keep_settings();
    }

    fn keep_settings(&mut self) {
        let Some(draft) = self.draft.as_mut().filter(|draft| !draft.saving) else { return };
        let Some(settings) = draft.settings() else { return };
        if !self.live {
            // A preview keeps them itself, as there's no service to.
            let name = draft.machine.clone();
            if let Ok(status) = &mut self.seen
                && let Some(machine) = status.machines.iter_mut().find(|machine| machine.name == name)
            {
                machine.memory_mb = settings.memory_mb;
                machine.processors = settings.processors;
                machine.chosen_card = settings.card;
                machine.nvapi_wrapper = settings.nvapi_wrapper;
                machine.tpm = settings.tpm;
                machine.network = settings.network;
                machine.shares = settings.shares;
                machine.network_mode = settings.network_mode;
                machine.forwards = settings.forwards;
                machine.nested_virtualization = settings.nested_virtualization;
            }
            self.follow_draft();
            return;
        }
        draft.saving = true;
        self.ask(|name| Job::Change(name, settings));
    }

    fn grow(&mut self) {
        let Some(machine) = self.machine() else { return };
        let Some(size_gb) = self.draft.as_ref().and_then(|draft| draft.growth(machine)) else { return };
        self.ask(|name| Job::Grow(name, size_gb));
    }

    fn browse_folder(&mut self, target: FolderFor) {
        let window = self.window.clone();
        let opened = crate::system::pick_folder(self.hwnd, move |path| {
            let _ = window.tell(Message::PickedFolder(target, path));
        });
        if !opened {
            self.tell("The folder picker didn't open. Type the folder's path instead.", true);
        }
    }

    /// Opens the report page. A report not begun is about the machine whose page it's opened from.
    fn open_report(&mut self) {
        if self.page != Page::Report
            && matches!(self.report.stage, ReportStage::Writing { .. })
            && self.report.machine.is_none()
            && self.report.what.is_empty()
        {
            self.report.machine = self.selected.clone().filter(|name| {
                self.page == Page::Machine
                    && self.status().is_some_and(|status| status.machines.iter().any(|m| &m.name == name))
            });
        }
        self.go(Page::Report);
    }

    /// Shows `page`. New machine, Report a problem and the app's settings note where they were
    /// opened from, for Back: a report opened from the settings goes back to them, and they go back
    /// to where they were opened from.
    fn go(&mut self, page: Page) {
        match page {
            Page::New | Page::Report
                if matches!(self.page, Page::Home | Page::Machine | Page::AppSettings) =>
            {
                self.back_to = self.page
            }
            Page::AppSettings if matches!(self.page, Page::Home | Page::Machine) => {
                self.settings_back = self.page
            }
            _ => {}
        }
        self.page = page;
    }

    /// Back, as the title bar's button, Alt+Left and the keyboard's Back key go.
    fn back(&mut self) {
        self.page = match self.page {
            Page::Home | Page::Machine => Page::Home,
            Page::New | Page::Report => self.back_to,
            Page::AppSettings => self.settings_back,
        };
    }

    /// Goes into the machine `name`'s page, if it's one of the list's, on `section` if it has it.
    fn enter(&mut self, name: String, section: Section) {
        let Some(machine) = self.status().and_then(|status| status.machines.iter().find(|m| m.name == name))
        else {
            return;
        };
        self.section = if section.offered(machine.system) { section } else { Section::Overview };
        self.page = Page::Machine;
        if self.selected.as_ref() != Some(&name) {
            self.select(Some(name));
        }
    }

    /// What the report needs from the last status and the form.
    fn asked(&self) -> Asked {
        let status = self.status();
        let machine = self.report.machine.clone().filter(|name| {
            status.is_some_and(|status| status.machines.iter().any(|machine| &machine.name == name))
        });
        Asked {
            machine_system: machine.as_ref().and_then(|name| {
                status?.machines.iter().find(|machine| &machine.name == name).map(|machine| machine.system)
            }),
            machine,
            machines: status
                .map(|status| status.machines.iter().map(|machine| machine.name.clone()).collect())
                .unwrap_or_default(),
            card: status.and_then(|status| {
                status.cards.iter().find(|card| card.default).map(|card| card.name.clone())
            }),
            kind: self.report.kind,
            description: self.report.what.clone(),
            contact: self.report.contact.clone(),
        }
    }

    fn prepare_report(&mut self) {
        if !self.live || !matches!(self.report.stage, ReportStage::Writing { .. }) {
            return;
        }
        self.report.stage = ReportStage::Preparing;
        report::prepare(self.asked(), self.window.clone());
    }

    fn ready(&mut self) -> Option<&mut Ready> {
        match &mut self.report.stage {
            ReportStage::Ready(ready) => Some(ready),
            _ => None,
        }
    }

    fn send_report(&mut self) {
        let live = self.live;
        let window = self.window.clone();
        let Some(ready) = self.ready().filter(|ready| !ready.sending && ready.id.is_none()) else { return };
        if !live {
            return;
        }
        ready.sending = true;
        ready.problem = None;
        report::send(&ready.prepared, window);
    }

    fn save_report(&mut self) {
        let (hwnd, live, window) = (self.hwnd, self.live, self.window.clone());
        let Some(ready) = self.ready() else { return };
        if !live {
            return;
        }
        let prepared = ready.prepared.clone();
        let name = prepared
            .archive
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "guesthouse-report.tar.gz".to_owned());
        let failed = window.clone();
        let opened = crate::system::pick_save(hwnd, &name, move |path| {
            // Nothing chosen is nothing to say.
            if let Some(path) = path {
                let _ = window.tell(Message::ReportSaved(report::save(&prepared, &path)));
            }
        });
        if !opened {
            let words = "The save dialog didn't open.".to_owned();
            let _ = failed.tell(Message::ReportSaved(Err(words)));
        }
    }

    fn open_issue(&mut self) {
        let link = match &self.report.stage {
            ReportStage::Ready(Ready { id: Some(id), prepared, .. }) => issue_link(id, &prepared.summary),
            _ => return,
        };
        if self.live
            && let Err(words) = crate::system::open_link(&link)
        {
            self.tell(words, true);
        }
    }

    fn start_over(&mut self) {
        let busy = match &self.report.stage {
            ReportStage::Preparing => true,
            ReportStage::Ready(ready) => ready.sending,
            ReportStage::Writing { .. } => false,
        };
        if busy {
            return;
        }
        if self.live {
            report::clear();
        }
        self.report = ReportPage::default();
    }

    fn tell(&mut self, text: impl Into<String>, bad: bool) {
        self.news = Some(News { text: text.into(), bad });
    }

    fn ask(&mut self, job: impl FnOnce(String) -> Job) {
        if !self.live {
            return;
        }
        if let Some(name) = self.selected.clone() {
            // The thread only goes when the window does, so this can't fail while it's open.
            let _ = self.jobs.send(job(name));
        }
    }

    /// What the page knows of the shown machine's remote view from the list.
    pub fn remote_view_facts(&self) -> Option<Facts> {
        let machine = self.machine()?;
        Some(Facts {
            running: machine.state == State::Running,
            bridged: matches!(machine.network_mode, NetworkMode::Bridged { .. }),
            switch: machine.remote_view,
            port: machine.remote_port,
        })
    }

    /// Asks about the shown Windows machine's remote view while its page shows, and lets go of
    /// another machine's.
    fn follow_remote_view(&mut self) {
        let wanted = self
            .machine()
            .filter(|machine| self.page == Page::Machine && machine.system == System::Windows)
            .map(|machine| machine.name.clone());
        if self.viewing.as_ref().map(|viewing| &viewing.machine) != wanted.as_ref() {
            self.viewing = wanted.as_deref().map(Viewing::new);
            self.looking = None;
        }
        let (Some(viewing), Some(facts)) = (&self.viewing, self.remote_view_facts()) else { return };
        // The list says on while the last answer said off when the switch was turned elsewhere, and
        // a look then finds it.
        let on = viewing.on(&facts) || facts.switch;
        if !self.live {
            return;
        }
        match &self.looking {
            Some(looking) => looking.set_on(on),
            None => {
                self.looking_id += 1;
                let looking = bridge::look(self.looking_id, viewing.machine.clone(), on, self.window.clone());
                self.looking = Some(looking);
            }
        }
    }

    fn ask_remote_view(&mut self, ask: Ask) {
        let Some(facts) = self.remote_view_facts() else { return };
        let Some(viewing) = self.viewing.as_mut() else { return };
        // A switch set from an answer says so too, which isn't the person asking.
        let shown = viewing.shown(&facts);
        let same = match &ask {
            Ask::Switch(on) => *on == shown.on,
            Ask::Allow { fingerprint, what, on } => shown
                .devices
                .iter()
                .filter(|device| &device.fingerprint == fingerprint)
                .flat_map(|device| &device.permissions)
                .any(|permission| permission.what == *what && permission.on == *on),
            _ => false,
        };
        if same {
            return;
        }
        viewing.asked(&ask);
        if self.live {
            if let Some(looking) = &self.looking {
                looking.ask(ask);
            }
        } else {
            #[cfg(feature = "preview")]
            self.pretend(ask);
        }
    }

    /// A preview's answer to what was asked, as a service's would be, with the request written to
    /// a file in place of being sent.
    #[cfg(feature = "preview")]
    fn pretend(&mut self, ask: Ask) {
        let Some(viewing) = &self.viewing else { return };
        let machine = viewing.machine.clone();
        preview::asked(&ask.request(&machine));
        let info = viewing.info.clone().unwrap_or_else(|| ShareInfo::off(&machine));
        let answer = match preview::refusal(&ask) {
            Some(reason) => Err(reason),
            None => crate::remote_view::pretend(&info, &ask, now_ms()),
        };
        self.remote_view_answered(ask, answer);
    }

    fn remote_view_answered(&mut self, ask: Ask, answer: Result<ShareInfo, String>) {
        let Some(viewing) = self.viewing.as_mut() else { return };
        let problem = viewing.answered(&ask, answer, now_ms());
        self.note_pairing();
        if let Some(problem) = problem {
            self.tell(problem, true);
        }
    }

    fn remote_view_seen(&mut self, seen: Result<ShareInfo, String>, address: Option<IpAddr>) {
        let Some(viewing) = self.viewing.as_mut() else { return };
        let info = seen.unwrap_or_else(|reason| {
            // Shown as the sharing's failure, with the rest as it was last seen, as Linux does.
            let mut info = viewing.info.clone().unwrap_or_else(|| ShareInfo::off(&viewing.machine));
            info.state = ShareState::Failed;
            info.reason = Some(reason);
            info
        });
        viewing.take(info, address, now_ms());
        self.note_pairing();
    }

    /// Notes whether the shown machine's pairing is open, for closing it as the app goes.
    fn note_pairing(&self) {
        if let Some(viewing) = self.viewing.as_ref().filter(|_| self.live) {
            bridge::note_pairing(&viewing.machine, viewing.pairing_open());
        }
    }

    fn ask_remove(&mut self, fingerprint: String) {
        let Some(viewing) = self.viewing.as_mut() else { return };
        let name = viewing
            .info
            .iter()
            .flat_map(|info| &info.devices)
            .find(|device| device.fingerprint == fingerprint)
            .map(crate::remote_view::device_name);
        if let Some(name) = name {
            viewing.removing = Some((fingerprint, name));
        }
    }

    /// Shows `name`'s page and starts its log afresh, as a watch begins with the recent events.
    fn select(&mut self, name: Option<String>) {
        self.selected = name;
        self.watch = None;
        self.rewatch = false;
        self.log.clear();
        self.account = None;
        self.draft = None;
        self.follow_draft();
        self.snapshot_label.clear();
        self.snapshot_ask = None;
        if let Some(name) = self.selected.clone().filter(|_| self.live) {
            self.watch_id += 1;
            self.watch = Some(bridge::watch(self.watch_id, name, self.window.clone()));
        }
    }

    fn take_status(&mut self, seen: Seen) {
        let was_away = self.seen.is_err();
        self.seen = seen;
        // The installer stops the service before it ends the app, so the app goes first.
        if !was_away && self.seen.is_err() {
            self.updates_step(update::Input::ServiceGone);
        }
        let Ok(status) = &self.seen else { return };
        let names: Vec<String> = status.machines.iter().map(|machine| machine.name.clone()).collect();
        self.end_stopped_console();
        match &self.selected {
            // Gone, as when it was deleted: its page goes too, back to Home.
            Some(name) if !names.contains(name) => {
                self.select(None);
                if self.page == Page::Machine {
                    self.page = Page::Home;
                }
                // Back from New machine, Report a problem or the app's settings has no machine's
                // page to go to now.
                if self.back_to == Page::Machine {
                    self.back_to = Page::Home;
                }
                if self.settings_back == Page::Machine {
                    self.settings_back = Page::Home;
                }
            }
            Some(_) if self.rewatch && was_away => self.select(self.selected.clone()),
            Some(_) | None => {}
        }
        self.follow_draft();
    }

    fn answered(&mut self, job: Job, answer: Result<Answer, Unreached>) {
        if let Job::Change(name, _) = &job
            && let Some(draft) = self.draft.as_mut().filter(|draft| &draft.machine == name)
        {
            draft.saving = false;
            // A no is shown beside the settings it's about, where it's seen.
            if let Ok(Answer::Refused { reason }) = &answer {
                draft.refused = Some(reason.clone());
                return;
            }
        }
        match (job, answer) {
            (Job::Change(name, _), Ok(Answer::Ok)) => {
                self.tell(format!("{name}'s settings are saved, for when it next starts."), false)
            }
            (Job::Account(machine), Ok(Answer::Account { user, password })) => {
                if self.selected.as_deref() == Some(machine.as_str()) {
                    self.account = Some(Account { machine, user, password });
                }
            }
            (
                Job::Screen(name),
                Ok(Answer::Screen { address, port, host_fingerprint, viewer_certificate, viewer_key }),
            ) => {
                let screen =
                    crate::screen::Screen { address, port, host_fingerprint, viewer_certificate, viewer_key };
                if let Err(words) = crate::screen::open(&name, &screen) {
                    self.tell(words, true);
                }
            }
            (Job::Delete(name), Ok(Answer::Ok)) => self.tell(format!("{name} is deleted."), false),
            (Job::Snapshot(name, label), Ok(Answer::Ok)) => {
                if self.selected.as_deref() == Some(name.as_str()) {
                    self.snapshot_label.clear();
                }
                self.tell(
                    format!("Taking the snapshot {label} of {name}. Its log says when it's done."),
                    false,
                );
            }
            (Job::Move(name, folder), Ok(Answer::Ok)) => {
                let to = if folder.as_os_str().is_empty() {
                    "where the service keeps its machines".to_owned()
                } else {
                    folder.display().to_string()
                };
                self.tell(format!("Moving {name}'s disks to {to}. Its log says how it goes."), false);
            }
            (Job::Grow(name, size_gb), Ok(Answer::Ok)) => {
                if let Ok(status) = &mut self.seen
                    && let Some(machine) = status.machines.iter_mut().find(|machine| machine.name == name)
                {
                    machine.size_gb = Some(size_gb);
                }
                if let Some(draft) = self.draft.as_mut().filter(|draft| draft.machine == name) {
                    draft.size_gb = Some(size_gb as f64);
                }
                self.tell(
                    format!(
                        "{name}'s disk may grow to {size_gb} GB. The guest takes the room at its next start."
                    ),
                    false,
                );
            }
            (_, Ok(Answer::Ok)) => {}
            (_, Ok(Answer::Refused { reason })) => self.tell(reason, true),
            (_, Ok(_)) => self.tell(Unreached::Garbled.words(), true),
            (_, Err(unreached)) => self.tell(unreached.words(), true),
        }
    }

    fn watched(&mut self, events: Vec<Event>) {
        for event in &events {
            if let (Event::State(state), Some(name), Ok(status)) = (event, &self.selected, &mut self.seen)
                && let Some(machine) = status.machines.iter_mut().find(|machine| &machine.name == name)
            {
                // Sooner than the next answer to Status, so the buttons follow at once.
                machine.state = *state;
            }
        }
        self.follow_draft();
        self.end_stopped_console();
        self.log.extend(events.into_iter().map(Line::of));
    }

    /// Opens the selected machine's console in a window of its own, or brings it to the front if
    /// it's open. A console of another machine is closed first, as there's one at a time.
    fn open_console(&mut self) {
        let Some(name) = self.selected.clone().filter(|_| self.live) else { return };
        if let Some(open) = &self.console
            && open.machine == name
        {
            open.window.front();
            return;
        }
        // Its pipe's name is new each boot, and the service says it once the machine runs.
        let Some(pipe) = self.machine().and_then(|machine| machine.console_pipe.clone()) else {
            self.tell(format!("{name}'s console isn't served yet. It's there once {name} runs."), true);
            return;
        };
        self.console = None;
        self.console_id += 1;
        let id = self.console_id;
        let window = self.window.clone();
        let title = format!("{name} \u{2014} Console");
        let opened = Console::open(&pipe, &title, false, true, move |event| {
            let _ = window.tell(Message::Consoled(id, event));
        });
        match opened {
            Ok(window) => {
                self.console =
                    Some(OpenConsole { machine: name, state: ConsoleState::Connecting, id, window });
            }
            Err(error) => self.tell(format!("The console's window couldn't be opened: {error}"), true),
        }
    }

    fn consoled(&mut self, id: u64, event: console::Event) {
        let Some(open) = self.console.as_mut().filter(|open| open.id == id) else { return };
        match event {
            console::Event::Waiting => open.state = ConsoleState::Waiting,
            console::Event::Connecting => open.state = ConsoleState::Connecting,
            console::Event::Connected => open.state = ConsoleState::Connected,
            console::Event::Disconnected { reason, extended, words } => {
                let words = words.unwrap_or_else(|| format!("the control gave reason {reason}, {extended}."));
                open.state = ConsoleState::Ended(format!("Its session ended: {words}"));
            }
            console::Event::Problem(words) => open.state = ConsoleState::Ended(words),
            console::Event::Note(_) => {}
            // Its window was closed, and the machine goes on as it is.
            console::Event::Closed => self.console = None,
        }
    }

    /// Ends the console of a machine that has stopped, or gone: its session first, so it doesn't
    /// hold the machine up, then its window.
    fn end_stopped_console(&mut self) {
        let Some(open) = &self.console else { return };
        let Ok(status) = &self.seen else { return };
        let on = status.machines.iter().any(|machine| {
            machine.name == open.machine && !matches!(machine.state, State::Off | State::Failed)
        });
        if !on {
            open.window.disconnect();
            open.window.close();
            self.console = None;
        }
    }

    fn watch_ended(&mut self, why: Ended) {
        self.watch = None;
        let text = match why {
            Ended::Refused(reason) => reason,
            Ended::Unreached(words) => {
                self.rewatch = true;
                format!("The log stopped: {words}")
            }
            Ended::Closed => {
                self.rewatch = true;
                "The service stopped sending this machine's log.".to_owned()
            }
        };
        self.log.push(Line { kind: Kind::Problem, text });
    }

    fn create(&mut self) {
        if self.busy() || !self.live {
            return;
        }
        let Some(machine) = self.form.machine() else { return };
        self.making = Some(Making { name: machine.name.clone(), lines: new_log(), ended: None });
        bridge::create(machine, self.window.clone());
    }

    fn remote_desktop(&mut self) {
        let Some(address) = self.machine().and_then(address).filter(|_| self.live) else { return };
        if let Err(words) = crate::system::remote_desktop(&address.to_string()) {
            self.tell(words, true);
        }
        // Remote Desktop asks for them at once, so they're shown beside it.
        if self.account.is_none() {
            self.ask(Job::Account);
        }
    }

    /// Takes in what happened to the updates, and does what that says to.
    fn updates_step(&mut self, input: update::Input) {
        let now = now_ms() / 1000;
        for effect in self.updates.step(input, now) {
            let (updater, window) = (self.updater.clone(), self.window.clone());
            match effect {
                update::Effect::Fetch { attempt } => bridge::look_for_update(updater, attempt, window),
                update::Effect::Download { attempt, latest } => {
                    let stop = Arc::new(AtomicBool::new(false));
                    self.download_stop = Some(stop.clone());
                    bridge::download_update(updater, attempt, latest, stop, window);
                }
                update::Effect::StopDownload => {
                    if let Some(stop) = self.download_stop.take() {
                        stop.store(true, Ordering::SeqCst);
                    }
                }
                update::Effect::Hash { attempt } => bridge::hash_update(updater, attempt, window),
                update::Effect::Discard => bridge::discard_update(updater),
                update::Effect::Launch { attempt, .. } => bridge::launch_update(updater, attempt, window),
                update::Effect::Close => self.closing = true,
            }
        }
    }

    /// The machines, as the service last said, or none while it can't be asked.
    pub fn machines(&self) -> &[Machine] {
        self.status().map(|status| status.machines.as_slice()).unwrap_or_default()
    }

    /// Why the update that's ready can't be installed now, if it can't.
    pub fn install_blocked(&self) -> Option<String> {
        update::install_blocked(self.machines(), self.busy())
    }

    /// Installs the update that's ready, if nothing holds it back. Running machines are asked about
    /// first, unless `asked`, as the installer stops the service, which shuts them down.
    fn install(&mut self, asked: bool) {
        if !self.updates.ready() {
            return;
        }
        if let Some(why) = self.install_blocked() {
            self.tell(why, true);
            return;
        }
        // With the service not answering, what it would stop can't be known, so it's always asked.
        let stops = self.status().is_none() || !update::stopped_by_install(self.machines()).is_empty();
        if !asked && stops {
            self.install_ask = true;
            return;
        }
        self.updates_step(update::Input::Install);
    }

    /// Whether the app looks for a newer Guesthouse by itself: the settings say to, and
    /// GUESTHOUSE_UPDATE_CHECK=0 doesn't say not to.
    pub fn looks_by_itself(&self) -> bool {
        self.preferences.look_by_itself && !looking_turned_off()
    }

    /// Changes the app's settings and keeps them. A preview keeps them only while it runs, so it
    /// never writes over the person's own.
    fn prefer(&mut self, change: impl FnOnce(&mut Preferences)) {
        let mut preferences = self.preferences.clone();
        change(&mut preferences);
        if preferences == self.preferences {
            return;
        }
        self.preferences = preferences;
        self.theme = window_theme(self.preferences.theme);
        self.updates.download_by_itself = self.preferences.when_out == WhenOut::Download;
        if !self.live {
            return;
        }
        let kept = match crate::preferences::folder() {
            Some(folder) => self.preferences.write(&folder).map_err(|error| error.to_string()),
            None => Err("Windows doesn't say where this user's app data is".to_owned()),
        };
        if let Err(why) = kept {
            self.tell(
                format!("The app's settings couldn't be kept: {why}. They last until it closes."),
                true,
            );
        }
    }

    fn set_sign_in(&mut self, on: bool) {
        if on == self.opens_at_sign_in {
            return;
        }
        if !self.live {
            self.opens_at_sign_in = on;
            return;
        }
        match crate::system::set_opens_at_sign_in(on) {
            Ok(()) => self.opens_at_sign_in = crate::system::opens_at_sign_in(),
            Err(words) => self.tell(words, true),
        }
    }

    /// Opens a page in the browser, which a preview only pretends to.
    fn open_page(&mut self, link: &str) {
        if self.live
            && let Err(words) = crate::system::open_link(link)
        {
            self.tell(words, true);
        }
    }

    fn browse(&mut self) {
        let window = self.window.clone();
        let opened = crate::system::pick_image(self.hwnd, move |path| {
            let _ = window.tell(Message::Picked(path));
        });
        if !opened {
            self.tell("The file dialog didn't open. Type the image's path instead.", true);
        }
    }
}

impl Main {
    /// Whether a question is being asked in a dialog now, when nothing else may open.
    pub fn asking(&self) -> bool {
        self.deleting.is_some()
            || self.snapshot_ask.is_some()
            || self.bridge_ask
            || self.install_ask
            || self.hyper_v_ask
            || self.restart_ask
            || self.viewing.as_ref().is_some_and(|viewing| viewing.removing.is_some())
    }

    /// What the palette offers now, in its own order.
    pub fn palette_entries(&self) -> Vec<Entry> {
        let here = match (self.page, self.machine()) {
            (Page::Home, _) if self.seen.is_ok() => Here::Home,
            (Page::Machine, Some(machine)) => Here::Machine(machine.name.clone(), self.section),
            _ => Here::Elsewhere,
        };
        palette::entries(&Now {
            here,
            machines: self.status().map(|status| status.machines.as_slice()),
            addressed: &|machine| address(machine).is_some(),
            can_look: self.updates.can_look(),
        })
    }

    /// What the palette lists now for what's typed, at most [`palette::MOST_SHOWN`] of them, and
    /// how many it found in all.
    pub fn palette_found(&self) -> (Vec<Entry>, usize) {
        let query = self.palette.as_ref().map(|palette| palette.query.as_str()).unwrap_or_default();
        let mut found = palette::matching(&self.palette_entries(), query);
        let all = found.len();
        found.truncate(palette::MOST_SHOWN);
        (found, all)
    }

    /// Runs the palette's entry `key`, if it's still one it offers now, as what can be done may
    /// have changed since it was listed: it closes, and each of the entry's steps is the message
    /// its button sends.
    fn run_palette(&mut self, key: &str, context: &ComponentContext<Self>) {
        let entry = self.palette_found().0.into_iter().find(|entry| entry.key == key);
        self.palette = None;
        let Some(entry) = entry else {
            self.tell("That can't be done now.", true);
            return;
        };
        for step in entry.steps {
            let message = match step {
                Step::Open(name) => Message::Open(name),
                Step::Section(section) => Message::Section(Some(section.label().to_owned())),
                Step::Start => Message::Start,
                Step::Stop => Message::Stop,
                Step::Screen => Message::Screen,
                Step::Home => {
                    self.go(Page::Home);
                    continue;
                }
                Step::NewMachine => Message::NewMachine,
                Step::Settings => Message::OpenSettings,
                Step::Report => Message::OpenReport,
                Step::Look => Message::Look,
            };
            self.update(message, context);
        }
    }

    /// Moves the palette's highlight, and says where it is to a screen reader, as the keyboard's
    /// focus stays in the box.
    fn palette_key(&mut self, key: palette::Key, context: &ComponentContext<Self>) {
        let (found, _) = self.palette_found();
        let Some(at) = self.palette.as_ref().map(|palette| palette.at.min(found.len().saturating_sub(1)))
        else {
            return;
        };
        if key == palette::Key::Run {
            if let Some(entry) = found.get(at) {
                self.run_palette(&entry.key.clone(), context);
            }
            return;
        }
        let at = palette::moved(at, key, found.len());
        if let Some(palette) = &mut self.palette {
            palette.at = at;
        }
        if let Some(entry) = found.get(at) {
            let said = format!("{}, {} of {}", entry.spoken(), at + 1, found.len());
            let _ = self.palette_box.request_notification(AutomationNotification::new(said));
        }
    }
}

/// Whether GUESTHOUSE_UPDATE_CHECK=0 turns looking by itself off.
pub fn looking_turned_off() -> bool {
    std::env::var("GUESTHOUSE_UPDATE_CHECK").is_ok_and(|value| value == "0")
}

/// The window's theme for the one the settings say.
fn window_theme(theme: Theme) -> WindowTheme {
    match theme {
        Theme::System => WindowTheme::System,
        Theme::Light => WindowTheme::Light,
        Theme::Dark => WindowTheme::Dark,
    }
}

/// The release notes of a version, on the release's page.
pub fn notes_of(version: &str) -> String {
    format!("https://github.com/ruxnlabs/guesthouse/releases/tag/windows-v{version}")
}

/// The machine's address, if it's one. It comes from what the guest wrote, so it's only used once
/// it reads as an address.
pub fn address(machine: &Machine) -> Option<IpAddr> {
    machine.address.as_deref()?.trim().parse().ok()
}

/// The `ssh` lines that reach an Ubuntu guest for `user`, each with what it goes through: the port on
/// this PC's loopback first, which needs no network, then its address on the network, if it has one
/// and is up.
pub fn ssh_lines(machine: &Machine, user: &str) -> Vec<(&'static str, String)> {
    let mut lines = Vec::new();
    if let Some(port) = machine.ssh_port {
        lines.push(("SSH, no network needed", guesthouse_control::ssh_line(port, user)));
    }
    if let Some(address) = address(machine) {
        lines.push(("SSH on the network", format!("ssh {user}@{address}")));
    }
    lines
}

impl Component for Main {
    type Input = Start;
    type Message = Message;

    fn create(start: &Start, context: &ComponentContext<Self>) -> Self {
        let _ = context.run_window(|window| Message::Hwnd(window.as_raw() as isize));
        let window = Window(context.completion());
        let live = live();
        // A preview's calls go nowhere, as nothing takes them.
        let jobs = if live { bridge::start(window.clone()) } else { std::sync::mpsc::channel().0 };
        // A preview reads none of the person's settings, and writes none.
        let preferences = match crate::preferences::folder().filter(|_| live) {
            Some(folder) => Preferences::read(&folder),
            None => Preferences::default(),
        };
        let installed = crate::system::installed();
        let mut main = Main {
            seen: Err(Unreached::NoService.words().to_owned()),
            page: Page::Home,
            back_to: Page::Home,
            settings_back: Page::Home,
            section: Section::Overview,
            layout: Layout::of(crate::window::WIDTH),
            focus: ElementRef::new(),
            focused: None,
            focus_timer: None,
            selected: None,
            log: new_log(),
            watch: None,
            watch_id: 0,
            rewatch: false,
            account: None,
            news: None,
            deleting: None,
            bridge_ask: false,
            hyper_v_ask: false,
            hyper_v_asking: false,
            restart_ask: false,
            restart_line_closed: false,
            palette: None,
            palette_box: ElementRef::new(),
            updates: Updates::new(installed, guesthouse_control::WINDOWS_VERSION),
            updater: Arc::new(crate::system::Network::new(
                crate::preferences::folder().filter(|_| live && installed).as_deref(),
            )),
            look_timer: None,
            download_stop: None,
            install_ask: false,
            closing: false,
            theme: window_theme(preferences.theme),
            preferences,
            opens_at_sign_in: live && crate::system::opens_at_sign_in(),
            installed,
            snapshot_ask: None,
            snapshot_label: String::new(),
            form: Form::default(),
            report: ReportPage::default(),
            draft: None,
            making: None,
            console: None,
            console_id: 0,
            jobs,
            window,
            hwnd: 0,
            icon: crate::system::icon_file().map(Rc::from),
            live,
            viewing: None,
            looking: None,
            looking_id: 0,
        };
        main.updates.download_by_itself = main.preferences.when_out == WhenOut::Download;
        // A day from now, and a day from each look after, it looks again if it's to.
        main.look_timer = Some(context.set_timeout(LOOK_EVERY, Message::LookDaily));
        #[cfg(feature = "preview")]
        if !live {
            preview::stage(&mut main);
            return main;
        }
        main.take_status(start.first.clone());
        // With only one machine, its page is where the app opens, and Home is one Back away.
        let only = main
            .status()
            .filter(|status| status.machines.len() == 1)
            .map(|status| status.machines[0].name.clone());
        if let Some(name) = only {
            main.enter(name, Section::Overview);
        }
        // Only an installed app looks, and the settings or GUESTHOUSE_UPDATE_CHECK=0 can turn it off.
        if main.looks_by_itself() {
            main.updates_step(update::Input::Look);
        }
        main
    }

    fn update(&mut self, message: Message, context: &ComponentContext<Self>) {
        match message {
            Message::Hwnd(hwnd) => self.hwnd = hwnd,
            Message::Status(seen) => self.take_status(seen),
            Message::Answered(job, answer) => self.answered(job, answer),
            Message::Watched(id, events) if id == self.watch_id && self.watch.is_some() => {
                self.watched(events)
            }
            Message::WatchEnded(id, why) if id == self.watch_id && self.watch.is_some() => {
                self.watch_ended(why)
            }
            Message::Watched(..) | Message::WatchEnded(..) => {}
            Message::Creating(event) => {
                if let Some(making) = &mut self.making {
                    making.lines.push(Line::of(event));
                }
            }
            Message::Created(ended) => {
                if let Some(making) = &mut self.making {
                    making.ended = Some(ended);
                }
            }
            Message::Navigate(Some(tag)) if tag == NEW_TAG => self.go(Page::New),
            Message::Navigate(Some(tag)) if tag == REPORT_TAG => self.open_report(),
            Message::Navigate(Some(name)) => self.enter(name, self.section),
            Message::Navigate(None) => {}
            Message::Open(name) => self.enter(name, Section::Overview),
            Message::Back => self.back(),
            Message::Section(label) => {
                let system = self.machine().map(|machine| machine.system);
                if let (Some(section), Some(system)) = (label.as_deref().and_then(Section::of_label), system)
                    && section.offered(system)
                {
                    self.section = section;
                }
            }
            Message::Layout(layout) => self.layout = layout,
            Message::Focus => {
                self.focus_timer = None;
                let _ = self.focus.request_focus();
            }
            Message::NewMachine => self.go(Page::New),
            Message::OpenReport => self.open_report(),
            Message::ReportMachine(index) => {
                let names: Vec<String> = self
                    .status()
                    .map(|status| status.machines.iter().map(|machine| machine.name.clone()).collect())
                    .unwrap_or_default();
                self.report.machine =
                    index.and_then(|index| index.checked_sub(1)).and_then(|n| names.get(n).cloned());
            }
            Message::ReportKind(Some(index)) => {
                if let Some(kind) = ProblemKind::ALL.get(index) {
                    self.report.kind = *kind;
                }
            }
            Message::ReportKind(None) => {}
            Message::ReportWhat(text) => self.report.what = text,
            Message::ReportContact(text) => self.report.contact = text,
            Message::ReportPrepare => self.prepare_report(),
            Message::ReportPrepared(result) => {
                if matches!(self.report.stage, ReportStage::Preparing) {
                    self.report.stage = match result {
                        Ok(prepared) => ReportStage::Ready(Ready {
                            prepared,
                            sending: false,
                            id: None,
                            problem: None,
                            saved: None,
                        }),
                        Err(problem) => ReportStage::Writing { problem: Some(problem) },
                    };
                }
            }
            Message::ReportSend => self.send_report(),
            Message::ReportSent(result) => {
                if let Some(ready) = self.ready() {
                    ready.sending = false;
                    match result {
                        Ok(id) => ready.id = Some(id),
                        Err(problem) => ready.problem = Some(problem),
                    }
                }
            }
            Message::ReportSave => self.save_report(),
            Message::ReportSaved(result) => {
                if let Some(ready) = self.ready() {
                    match result {
                        Ok(path) => {
                            ready.saved = Some(path.display().to_string());
                            ready.problem = None;
                        }
                        Err(problem) => ready.problem = Some(problem),
                    }
                }
            }
            Message::ReportIssue => self.open_issue(),
            Message::ReportStartOver => self.start_over(),
            Message::Start => self.ask(Job::Start),
            Message::Stop => self.ask(Job::Stop),
            Message::More(key) => match key.as_deref() {
                Some(DELETE_KEY) => self.deleting = self.selected.clone(),
                Some(REPORT_TAG) => self.open_report(),
                _ => {}
            },
            Message::DeleteAnswered(result) => {
                if let Some(name) = self.deleting.take()
                    && result == ContentDialogResult::Primary
                {
                    let _ = self.jobs.send(Job::Delete(name));
                }
            }
            Message::SignIn => self.ask(Job::Account),
            Message::Screen => self.ask(Job::Screen),
            Message::Console => self.open_console(),
            Message::Reconnect => {
                if let Some(open) = &mut self.console {
                    open.state = ConsoleState::Connecting;
                    open.window.reconnect();
                }
            }
            Message::CloseConsole => self.console = None,
            Message::Consoled(id, event) => self.consoled(id, event),
            Message::RemoteDesktop => self.remote_desktop(),
            Message::Copy(text) => {
                if crate::system::copy(self.hwnd, &text) {
                    self.tell("Copied.", false);
                } else {
                    self.tell("The clipboard couldn't be reached. Try again.", true);
                }
            }
            Message::WatchAgain => self.select(self.selected.clone()),
            Message::Name(name) => self.form.name = name,
            Message::Image(image) => self.form.image = image,
            Message::Browse => self.browse(),
            Message::Picked(path) => {
                if let Some(path) = path {
                    self.form.image = path.display().to_string();
                }
            }
            Message::Memory(value) => self.form.memory_mb = value,
            Message::Processors(value) => self.form.processors = value,
            Message::Size(value) => self.form.size_gb = value,
            Message::Card(Some(index)) => {
                if let Some(card) = self.card_choices().get(index) {
                    self.form.card = card.clone();
                }
            }
            Message::Card(None) => {}
            Message::Edition(Some(index)) if index < EDITIONS.len() => self.form.edition = index,
            Message::Edition(_) => {}
            Message::User(user) => self.form.user = user,
            Message::Tpm(on) => self.form.tpm = on,
            Message::Network(on) => self.form.network = on,
            Message::ShareFolder(folder) => self.form.share_folder = folder,
            Message::ShareReadOnly(checked) => self.form.share_read_only = checked.unwrap_or(false),
            Message::BrowseFolder(target) => self.browse_folder(target),
            Message::PickedFolder(_, None) => {}
            Message::PickedFolder(FolderFor::Form, Some(path)) => {
                self.form.share_folder = path.display().to_string();
            }
            Message::PickedFolder(FolderFor::Machine(index), Some(path)) => self.edit(|draft| {
                if let Some(row) = draft.shares.get_mut(index) {
                    row.folder = path.display().to_string();
                }
            }),
            Message::PickedFolder(FolderFor::Location, Some(path)) => {
                self.form.location = path.display().to_string();
            }
            // Picking the folder is asking for the move, as the picker was opened to say where to.
            Message::PickedFolder(FolderFor::Move, Some(path)) => self.ask(|name| Job::Move(name, path)),
            Message::Location(folder) => self.form.location = folder,
            Message::MoveDefault => self.ask(|name| Job::Move(name, PathBuf::new())),
            Message::Create => self.create(),
            Message::SetMemory(value) => self.edit(|draft| draft.memory_mb = value),
            Message::SetProcessors(value) => self.edit(|draft| draft.processors = value),
            Message::SetCard(Some(index)) => {
                if let Some(card) = self.settings_cards().get(index).cloned() {
                    self.edit(|draft| draft.card = card);
                }
            }
            Message::SetCard(None) => {}
            Message::SetDlss(on) => self.edit(|draft| draft.nvapi_wrapper = on),
            Message::SaveSettings => self.save_settings(),
            Message::BridgeAnswered(result) => {
                if std::mem::take(&mut self.bridge_ask) && result == ContentDialogResult::Primary {
                    self.keep_settings();
                }
            }
            Message::SetNetworkMode(Some(index)) => {
                if let Some(mode) = self.network_choices().get(index).cloned() {
                    self.edit(|draft| draft.network_mode = mode);
                }
            }
            Message::SetNetworkMode(None) => {}
            Message::AddForward => self.edit(|draft| {
                if draft.forwards.len() < guesthouse_control::MOST_FORWARDS {
                    draft.forwards.push(ForwardRow::NEW);
                }
            }),
            Message::RemoveForward(index) => self.edit(|draft| {
                if index < draft.forwards.len() {
                    draft.forwards.remove(index);
                }
            }),
            Message::SetForwardHost(index, value) => self.edit(|draft| {
                if let Some(row) = draft.forwards.get_mut(index) {
                    row.host = value;
                }
            }),
            Message::SetForwardGuest(index, value) => self.edit(|draft| {
                if let Some(row) = draft.forwards.get_mut(index) {
                    row.guest = value;
                }
            }),
            Message::UndoSettings => {
                self.bridge_ask = false;
                self.draft = None;
                self.follow_draft();
            }
            Message::SetTpm(on) => self.edit(|draft| draft.tpm = on),
            Message::SetNested(on) => self.edit(|draft| draft.nested_virtualization = on),
            Message::SetNetwork(on) => self.edit(|draft| draft.network = on),
            Message::AddShare => self.edit(|draft| {
                if draft.shares.len() < guesthouse_control::MOST_SHARES {
                    draft.shares.push(ShareRow::default());
                }
            }),
            Message::RemoveShare(index) => self.edit(|draft| {
                if index < draft.shares.len() {
                    draft.shares.remove(index);
                }
            }),
            Message::SetShareFolder(index, folder) => self.edit(|draft| {
                if let Some(row) = draft.shares.get_mut(index) {
                    row.folder = folder;
                }
            }),
            Message::SetShareReadOnly(index, checked) => self.edit(|draft| {
                if let Some(row) = draft.shares.get_mut(index) {
                    row.read_only = checked.unwrap_or(false);
                }
            }),
            Message::SetSize(value) => self.edit(|draft| draft.size_gb = value),
            Message::Grow => self.grow(),
            Message::SnapshotLabel(label) => self.snapshot_label = label,
            Message::TakeSnapshot => {
                let label = self.snapshot_label.trim().to_owned();
                if !label.is_empty() {
                    self.ask(|name| Job::Snapshot(name, label));
                }
            }
            Message::RevertSnapshot(id) | Message::DeleteSnapshot(id) => {
                let revert = matches!(message, Message::RevertSnapshot(_));
                if let Some(machine) = self.machine()
                    && let Some(snapshot) = machine.snapshots.iter().find(|snapshot| snapshot.id == id)
                {
                    self.snapshot_ask = Some(SnapshotAsk {
                        machine: machine.name.clone(),
                        id,
                        label: snapshot.label.clone(),
                        revert,
                    });
                }
            }
            Message::SnapshotAnswered(result) => {
                if let Some(ask) = self.snapshot_ask.take()
                    && result == ContentDialogResult::Primary
                    && self.live
                {
                    let job = if ask.revert {
                        Job::Revert(ask.machine, ask.id)
                    } else {
                        Job::DeleteSnapshot(ask.machine, ask.id)
                    };
                    let _ = self.jobs.send(job);
                }
            }
            Message::NewsClosed => self.news = None,
            Message::OpenSettings => {
                // Windows' Run key may have been changed elsewhere, as in Task Manager.
                if self.live {
                    self.opens_at_sign_in = crate::system::opens_at_sign_in();
                }
                self.go(Page::AppSettings);
            }
            Message::SetTheme(Some(index)) => {
                if let Some(theme) = Theme::ALL.get(index).copied() {
                    self.prefer(|preferences| preferences.theme = theme);
                }
            }
            Message::SetTheme(None) => {}
            Message::SetLookByItself(on) => self.prefer(|preferences| preferences.look_by_itself = on),
            Message::SetSignIn(on) => self.set_sign_in(on),
            Message::Look => self.updates_step(update::Input::Look),
            Message::LookDaily => {
                self.look_timer = Some(context.set_timeout(LOOK_EVERY, Message::LookDaily));
                if self.looks_by_itself() {
                    self.updates_step(update::Input::Look);
                }
            }
            Message::Looked(attempt, fetched) => {
                self.updates_step(update::Input::Looked { attempt, fetched })
            }
            Message::SetWhenOut(Some(index)) => {
                if let Some(when_out) = WhenOut::ALL.get(index).copied() {
                    self.prefer(|preferences| preferences.when_out = when_out);
                }
            }
            Message::SetWhenOut(None) => {}
            Message::GetUpdate => self.updates_step(update::Input::Download),
            Message::CancelUpdate => self.updates_step(update::Input::Cancel),
            Message::UpdateProgress(attempt, got, total) => {
                self.updates_step(update::Input::Progress { attempt, got, total })
            }
            Message::Downloaded(attempt, result) => {
                self.updates_step(update::Input::Downloaded { attempt, result })
            }
            Message::Hashed(attempt, result) => self.updates_step(update::Input::Hashed { attempt, result }),
            Message::Install => self.install(false),
            Message::InstallAnswered(result) => {
                if std::mem::take(&mut self.install_ask) && result == ContentDialogResult::Primary {
                    self.install(true);
                }
            }
            Message::Launched(attempt, result) => {
                self.updates_step(update::Input::Launched { attempt, result })
            }
            Message::InstallerEnded(attempt, code) => {
                self.updates_step(update::Input::InstallerEnded { attempt, code })
            }
            Message::LaterUpdate => self.updates_step(update::Input::PutAside),
            Message::ReleaseNotes(version) => {
                // The newer one's file may name its page, which is on the release's own site.
                let link = match self.updates.newer() {
                    Some(latest) if latest.version == version && !latest.notes.is_empty() => {
                        latest.notes.clone()
                    }
                    _ => notes_of(&version),
                };
                self.open_page(&link);
            }
            Message::Licences => {
                if self.live
                    && let Err(words) = crate::system::open_licences()
                {
                    self.tell(words, true);
                }
            }
            Message::TurnOnHyperV => self.hyper_v_ask = true,
            Message::HyperVAnswered(result) => {
                if std::mem::take(&mut self.hyper_v_ask) && result == ContentDialogResult::Primary {
                    self.turn_on_hyper_v();
                }
            }
            // The status says how far it has got, from any window.
            Message::HyperVNote => {}
            Message::HyperVDone(reason) => {
                self.hyper_v_asking = false;
                let (words, bad) = crate::hyper_v::ended_words(reason.as_deref());
                self.tell(words, bad);
            }
            Message::RestartWindows => self.restart_ask = true,
            Message::RestartAnswered(result) => {
                if std::mem::take(&mut self.restart_ask) && result == ContentDialogResult::Primary {
                    self.restart_windows();
                }
            }
            Message::RestartLineClosed => self.restart_line_closed = true,
            Message::OpenPalette => {
                if self.palette.is_none() && !self.asking() {
                    self.palette = Some(Palette::default());
                    self.focus_timer = Some(context.set_timeout(FOCUS_AFTER, Message::PaletteFocus));
                }
            }
            Message::PaletteQuery(query) => {
                if let Some(palette) = &mut self.palette {
                    palette.query = query;
                    palette.at = 0;
                }
            }
            Message::PaletteKey(key) => self.palette_key(key, context),
            Message::PaletteRun(key) => self.run_palette(&key, context),
            Message::PaletteClosed => self.palette = None,
            Message::PaletteFocus => {
                let _ = self.palette_box.request_focus();
            }
            Message::RemoteView(ask) => self.ask_remote_view(ask),
            Message::RemoteViewAnswered(id, ask, answer) if id == self.looking_id => {
                self.remote_view_answered(ask, answer)
            }
            Message::RemoteViewSeen(id, seen, address) if id == self.looking_id => {
                self.remote_view_seen(seen, address)
            }
            Message::RemoteViewAnswered(..) | Message::RemoteViewSeen(..) => {}
            Message::RemoveDevice(fingerprint) => self.ask_remove(fingerprint),
            Message::RemoveAnswered(result) => {
                let removing = self.viewing.as_mut().and_then(|viewing| viewing.removing.take());
                if let Some((fingerprint, _)) = removing
                    && result == ContentDialogResult::Primary
                {
                    self.ask_remote_view(Ask::Revoke(fingerprint));
                }
            }
        }
        if self.closing {
            self.closing = false;
            let _ = context.close_window();
        }
        self.follow_remote_view();
        let opened =
            (self.page, self.selected.clone().filter(|_| self.page == Page::Machine), self.seen.is_ok());
        if self.focused.as_ref() != Some(&opened) {
            self.focused = Some(opened);
            self.focus_timer = Some(context.set_timeout(FOCUS_AFTER, Message::Focus));
        }
    }

    fn view(&self, _: &Start, context: &mut ViewContext<Self>) -> View {
        crate::window::view(self, context)
    }
}

#[cfg(test)]
mod tests;
