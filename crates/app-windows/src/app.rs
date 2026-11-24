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
use std::sync::mpsc::Sender;

use guesthouse_control::client::Unreached;
use guesthouse_control::updates::Latest;
use guesthouse_control::{
    Answer, Card, CardInfo, Event, Machine, MachineSettings, NewMachine, Share, State, System, valid_name,
    valid_user,
};
use windows_reactor::*;

use crate::bridge::{self, Ended, Job, Seen, Watch, Window};
use crate::console::{self, Console};

#[cfg(feature = "preview")]
mod preview;

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
    /// The shown machine's page.
    Machine,
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

/// What the right side shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Machine,
    New,
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
            Event::State(state) => (Kind::Note, format!("It's {}.", state_words(state).to_lowercase())),
        };
        Line { kind, text }
    }
}

fn push(lines: &mut Vec<Line>, line: Line) {
    if lines.len() == LOG_LIMIT {
        lines.remove(0);
    }
    lines.push(line);
}

pub fn state_words(state: State) -> &'static str {
    match state {
        State::Preparing => "Being made",
        State::Starting => "Starting",
        State::Running => "Running",
        State::Stopping => "Shutting down",
        State::Stopped => "Stopped",
    }
}

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
        if !valid_name(name) || image.is_empty() || !(user.is_empty() || valid_user(user)) {
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
            tpm: self.tpm,
            network: self.network,
            share: share_of(&self.share_folder, self.share_read_only),
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
        share: machine.share.clone(),
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
    pub network: bool,
    /// The folder to share, as typed or picked. Empty shares none.
    pub share_folder: String,
    pub share_read_only: bool,
    /// The size the disk is to grow to, which is asked for on its own, with Grow, as it can't be
    /// undone.
    pub size_gb: Option<f64>,
    /// Asked for, and not answered yet.
    pub saving: bool,
    /// Why the service said no, until they're changed again.
    pub refused: Option<String>,
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
            network: machine.network,
            share_folder: machine
                .share
                .as_ref()
                .map(|share| share.folder.display().to_string())
                .unwrap_or_default(),
            share_read_only: machine.share.as_ref().is_some_and(|share| share.read_only),
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
            share: share_of(&self.share_folder, self.share_read_only),
        })
    }

    /// Whether the person has changed something from what `machine`'s settings say.
    pub fn changed(&self, machine: &Machine) -> bool {
        self.settings() != Some(settings_of(machine))
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
    pub lines: Vec<Line>,
    /// `Some(None)` when it worked, `Some(Some(why))` when it didn't.
    pub ended: Option<Option<String>>,
}

pub struct Main {
    pub seen: Seen,
    pub page: Page,
    /// The machine whose page is shown, by name, as the list's order can change under it.
    pub selected: Option<String>,
    /// The selected machine's log, from its watch.
    pub log: Vec<Line>,
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
    /// A newer Windows host, once the file drop says there is one, until it's put aside.
    pub update: Option<Latest>,
    /// The snapshot whose Revert or Delete is being asked about.
    pub snapshot_ask: Option<SnapshotAsk>,
    /// The label typed for the next snapshot of the shown machine.
    pub snapshot_label: String,
    pub form: Form,
    /// The selected machine's settings, as the person is changing them.
    pub draft: Option<Draft>,
    pub making: Option<Making>,
    /// The console's window, while one is open. There's one at a time.
    pub console: Option<OpenConsole>,
    console_id: u64,
    jobs: Sender<Job>,
    window: Window,
    /// The window's handle, for the file dialog and the clipboard.
    hwnd: isize,
    pub icon: Option<Rc<str>>,
    /// Light or dark, or as Windows is. Only a preview chooses.
    pub theme: WindowTheme,
    /// Whether the machines are the service's. A preview's aren't, and nothing is asked of it.
    live: bool,
}

/// The key of Delete in a machine's More menu.
pub const DELETE_KEY: &str = "delete";

/// The tag of the navigation's New machine item. It has a space, which no machine's name can.
pub const NEW_TAG: &str = " new";

#[derive(Clone)]
pub enum Message {
    Hwnd(isize),
    Status(Seen),
    Answered(Job, Result<Answer, Unreached>),
    Watched(u64, Event),
    WatchEnded(u64, Ended),
    Creating(Event),
    Created(Option<String>),
    /// An item of the navigation was chosen, by its tag: a machine's name, or `NEW_TAG`.
    Navigate(Option<String>),
    NewMachine,
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
    UndoSettings,
    SetTpm(bool),
    SetNetwork(bool),
    SetShareFolder(String),
    SetShareReadOnly(Option<bool>),
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
    /// A newer Windows host is out.
    Update(Latest),
    /// Open the newer one's installer in the browser.
    GetUpdate,
    /// Put the update aside until the app next starts.
    LaterUpdate,
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

    /// Starts the selected machine's draft afresh from what the service says, unless the person
    /// has changed something while it's stopped.
    fn follow_draft(&mut self) {
        let Some(machine) = self.machine() else {
            self.draft = None;
            return;
        };
        let keep = self.draft.as_ref().is_some_and(|draft| {
            draft.machine == machine.name
                && machine.state == State::Stopped
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
        if machine.state != State::Stopped {
            return;
        }
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
                machine.share = settings.share;
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
        let Ok(status) = &self.seen else { return };
        let names: Vec<String> = status.machines.iter().map(|machine| machine.name.clone()).collect();
        self.end_stopped_console();
        match &self.selected {
            Some(name) if !names.contains(name) => self.select(names.first().cloned()),
            Some(_) if self.rewatch && was_away => self.select(self.selected.clone()),
            Some(_) => {}
            None => self.select(names.first().cloned()),
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

    fn watched(&mut self, event: Event) {
        if let (Event::State(state), Some(name), Ok(status)) = (&event, &self.selected, &mut self.seen)
            && let Some(machine) = status.machines.iter_mut().find(|machine| &machine.name == name)
        {
            // Sooner than the next answer to Status, so the buttons follow at once.
            machine.state = *state;
        }
        self.follow_draft();
        self.end_stopped_console();
        push(&mut self.log, Line::of(event));
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
        self.console = None;
        self.console_id += 1;
        let id = self.console_id;
        let window = self.window.clone();
        let title = format!("{name} \u{2014} Console");
        let pipe = guesthouse_control::console_pipe(&name);
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
        let on = status
            .machines
            .iter()
            .any(|machine| machine.name == open.machine && machine.state != State::Stopped);
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
        push(&mut self.log, Line { kind: Kind::Problem, text });
    }

    fn create(&mut self) {
        if self.busy() || !self.live {
            return;
        }
        let Some(machine) = self.form.machine() else { return };
        self.making = Some(Making { name: machine.name.clone(), lines: Vec::new(), ended: None });
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

/// The machine's address, if it's one. It comes from what the guest wrote, so it's only used once
/// it reads as an address.
pub fn address(machine: &Machine) -> Option<IpAddr> {
    machine.address.as_deref()?.trim().parse().ok()
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
        let mut main = Main {
            seen: Err(Unreached::NoService.words().to_owned()),
            page: Page::Machine,
            selected: None,
            log: Vec::new(),
            watch: None,
            watch_id: 0,
            rewatch: false,
            account: None,
            news: None,
            deleting: None,
            update: None,
            snapshot_ask: None,
            snapshot_label: String::new(),
            form: Form::default(),
            draft: None,
            making: None,
            console: None,
            console_id: 0,
            jobs,
            window,
            hwnd: 0,
            icon: crate::system::icon_file().map(Rc::from),
            theme: WindowTheme::System,
            live,
        };
        #[cfg(feature = "preview")]
        if !live {
            preview::stage(&mut main);
            return main;
        }
        main.take_status(start.first.clone());
        // Only an installed app asks, and anyone can turn it off.
        let asked_not_to = std::env::var("GUESTHOUSE_UPDATE_CHECK").is_ok_and(|value| value == "0");
        if crate::system::installed() && !asked_not_to {
            bridge::check_for_update(main.window.clone());
        }
        main
    }

    fn update(&mut self, message: Message, _: &ComponentContext<Self>) {
        match message {
            Message::Hwnd(hwnd) => self.hwnd = hwnd,
            Message::Status(seen) => self.take_status(seen),
            Message::Answered(job, answer) => self.answered(job, answer),
            Message::Watched(id, event) if id == self.watch_id && self.watch.is_some() => self.watched(event),
            Message::WatchEnded(id, why) if id == self.watch_id && self.watch.is_some() => {
                self.watch_ended(why)
            }
            Message::Watched(..) | Message::WatchEnded(..) => {}
            Message::Creating(event) => {
                if let Some(making) = &mut self.making {
                    push(&mut making.lines, Line::of(event));
                }
            }
            Message::Created(ended) => {
                if let Some(making) = &mut self.making {
                    making.ended = Some(ended);
                }
            }
            Message::Navigate(Some(tag)) if tag == NEW_TAG => self.page = Page::New,
            Message::Navigate(Some(name)) => {
                let known =
                    self.status().is_some_and(|status| status.machines.iter().any(|m| m.name == name));
                if known {
                    self.page = Page::Machine;
                    // The navigation says what's selected whenever it's drawn, so only a new choice
                    // counts.
                    if self.selected.as_ref() != Some(&name) {
                        self.select(Some(name));
                    }
                }
            }
            Message::Navigate(None) => {}
            Message::NewMachine => self.page = Page::New,
            Message::Start => self.ask(Job::Start),
            Message::Stop => self.ask(Job::Stop),
            Message::More(key) => {
                if key.as_deref() == Some(DELETE_KEY) {
                    self.deleting = self.selected.clone();
                }
            }
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
            Message::PickedFolder(FolderFor::Machine, Some(path)) => {
                self.edit(|draft| draft.share_folder = path.display().to_string());
            }
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
            Message::UndoSettings => {
                self.draft = None;
                self.follow_draft();
            }
            Message::SetTpm(on) => self.edit(|draft| draft.tpm = on),
            Message::SetNetwork(on) => self.edit(|draft| draft.network = on),
            Message::SetShareFolder(folder) => self.edit(|draft| draft.share_folder = folder),
            Message::SetShareReadOnly(checked) => {
                self.edit(|draft| draft.share_read_only = checked.unwrap_or(false))
            }
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
            Message::Update(latest) => self.update = Some(latest),
            Message::GetUpdate => {
                if let Some(latest) = &self.update
                    && let Err(words) = crate::system::open_link(&latest.installer)
                {
                    self.tell(words, true);
                }
            }
            Message::LaterUpdate => self.update = None,
        }
    }

    fn view(&self, _: &Start, context: &mut ViewContext<Self>) -> View {
        crate::window::view(self, context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine() -> Machine {
        Machine {
            name: "win".to_owned(),
            system: System::Windows,
            state: State::Stopped,
            set_up: true,
            memory_mb: 8192,
            processors: 4,
            card: Some("NVIDIA GeForce RTX 5090".to_owned()),
            chosen_card: Card::Default,
            nvapi_wrapper: true,
            tpm: true,
            network: true,
            share: None,
            size_gb: Some(128),
            snapshots: Vec::new(),
            working: None,
            address: None,
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
}
