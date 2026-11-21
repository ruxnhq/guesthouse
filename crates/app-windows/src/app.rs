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
use guesthouse_control::{Answer, Card, CardInfo, Event, Machine, NewMachine, State, valid_name};
use windows_reactor::*;

use crate::bridge::{self, Ended, Job, Seen, Watch, Window};

#[cfg(feature = "preview")]
mod preview;

/// The most lines a log keeps. The oldest go first.
const LOG_LIMIT: usize = 500;
/// What a new machine is given unless the person changes it.
const MEMORY_MB: f64 = 8192.0;
const PROCESSORS: f64 = 4.0;
const SIZE_GB: f64 = 128.0;

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

/// The new machine's form, as the person has filled it in so far.
#[derive(Clone, PartialEq)]
pub struct Form {
    pub name: String,
    pub image: String,
    pub memory_mb: Option<f64>,
    pub processors: Option<f64>,
    pub size_gb: Option<f64>,
    pub card: Card,
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
        }
    }
}

impl Form {
    /// The machine to ask for, once every field holds something that can be sent. Whether the
    /// numbers suit this PC is the service's to say.
    pub fn machine(&self) -> Option<NewMachine> {
        let whole =
            |value: Option<f64>| value.filter(|v| v.is_finite() && *v >= 1.0).map(|v| v.round() as u64);
        let name = self.name.trim();
        let image = self.image.trim().trim_matches('"');
        if !valid_name(name) || image.is_empty() {
            return None;
        }
        Some(NewMachine {
            name: name.to_owned(),
            image: PathBuf::from(image),
            memory_mb: whole(self.memory_mb)?,
            processors: u32::try_from(whole(self.processors)?).ok()?,
            size_gb: whole(self.size_gb)?,
            card: self.card.clone(),
        })
    }
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
    pub form: Form,
    pub making: Option<Making>,
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
    Create,
    NewsClosed,
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
        match &self.selected {
            Some(name) if !names.contains(name) => self.select(names.first().cloned()),
            Some(_) if self.rewatch && was_away => self.select(self.selected.clone()),
            Some(_) => {}
            None => self.select(names.first().cloned()),
        }
    }

    fn answered(&mut self, job: Job, answer: Result<Answer, Unreached>) {
        match (job, answer) {
            (Job::Account(machine), Ok(Answer::Account { user, password })) => {
                if self.selected.as_deref() == Some(machine.as_str()) {
                    self.account = Some(Account { machine, user, password });
                }
            }
            (Job::Delete(name), Ok(Answer::Ok)) => self.tell(format!("{name} is deleted."), false),
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
        push(&mut self.log, Line::of(event));
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
            form: Form::default(),
            making: None,
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
            Message::Create => self.create(),
            Message::NewsClosed => self.news = None,
        }
    }

    fn view(&self, _: &Start, context: &mut ViewContext<Self>) -> View {
        crate::window::view(self, context)
    }
}
