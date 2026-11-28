//! The command palette, Ctrl+K: which actions it offers now, and which of them a query finds, in
//! what order. It has nothing of Windows in it, so its tests run on any system.
//!
//! It offers only what the page's own buttons would do now, and each entry is the very messages
//! those buttons send, in order: an action on a machine whose page isn't the one shown goes to its
//! page first, as the buttons are there. So nothing can be done from the palette that the buttons
//! would refuse. What can't be undone isn't offered: Delete, and Stop now, which turns a machine off
//! as pulling its plug would, are left to their buttons, which ask first.
//!
//! A query matches an entry when each of its words, in any order and any case, begins a word of the
//! entry's name or of its machine's name: `win st` finds Start win11. The entries are then in three
//! runs, each in the palette's own order: those whose name begins with the whole query, then those
//! whose machine's name begins with its first word, then the rest.

// Off Windows only the tests use these.
#![cfg_attr(not(windows), allow(dead_code))]

use guesthouse_control::{Machine, State, System};

use crate::shell::Section;

/// The most entries the palette lists at once. A query narrows the rest down.
pub const MOST_SHOWN: usize = 8;

/// Whether a machine's Start can be pressed now, in its title bar and the palette.
pub fn can_start(machine: &Machine) -> bool {
    matches!(machine.state, State::Off | State::Failed) && machine.working.is_none()
}

/// Whether a machine can be asked to shut down now: Stop, and not Stop now, which its button turns
/// into once it's stopping, and which the palette doesn't offer.
pub fn can_ask_to_stop(machine: &Machine) -> bool {
    matches!(machine.state, State::Starting | State::Running)
}

/// Whether a machine's screen can be opened now: a Windows guest that's running and set up, with
/// an address, as remoteman's host is put in at the boots after its first.
pub fn can_open_screen(machine: &Machine, has_address: bool) -> bool {
    machine.system == System::Windows && has_address && machine.state == State::Running && machine.set_up
}

/// One thing a palette entry does, as the button that does it would: each is one of the app's
/// messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Go into a machine's page, on its Overview, as its card on Home does.
    Open(String),
    /// Show a section of the machine's page that's shown.
    Section(Section),
    /// The shown machine's Start, Stop and Open screen.
    Start,
    Stop,
    Screen,
    /// Back to Home, as Back does from a machine's page.
    Home,
    NewMachine,
    Settings,
    Report,
    /// Look for a newer Guesthouse now, as Settings' Look now does.
    Look,
}

/// What an entry is about, which the list shows as a heading over its entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Group {
    /// The machine whose page is shown, or another one, by name, with its state.
    Machine { name: String, state: State },
    /// The app's own: New machine, Settings, Report a problem.
    App,
}

impl Group {
    pub fn heading(&self) -> String {
        match self {
            Group::Machine { name, state } => format!("{name} · {}", state.name()),
            Group::App => "App".to_owned(),
        }
    }
}

/// One thing the palette offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Which entry it is, unique in the list.
    pub key: String,
    /// What it's called.
    pub name: String,
    /// A few words beside it, if it needs them.
    pub hint: String,
    pub group: Group,
    /// What it does, in order.
    pub steps: Vec<Step>,
}

impl Entry {
    /// The machine it's about, if any.
    fn machine(&self) -> Option<&str> {
        match &self.group {
            Group::Machine { name, .. } => Some(name),
            Group::App => None,
        }
    }

    /// What the list's row is called by a screen reader: its name, its hint, and its group.
    pub fn spoken(&self) -> String {
        let mut words = self.name.clone();
        if !self.hint.is_empty() {
            words.push_str(", ");
            words.push_str(&self.hint);
        }
        if let Group::Machine { name, .. } = &self.group
            && !self.name.contains(name.as_str())
        {
            words.push_str(", ");
            words.push_str(name);
        }
        words
    }
}

/// Where the window is, as far as the palette goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Here {
    Home,
    /// A machine's page, by name, on a section.
    Machine(String, Section),
    /// New machine, Report a problem or the app's settings.
    Elsewhere,
}

/// What the palette is offered from: where the window is, the machines as the service last said
/// (none while it can't be asked), which of them have an address, and whether a look for a newer
/// Guesthouse can be asked for now.
pub struct Now<'a> {
    pub here: Here,
    /// `None` while the service can't be asked: then only the app's own are offered.
    pub machines: Option<&'a [Machine]>,
    /// Whether each machine, by name, has an address, for Open screen.
    pub addressed: &'a dyn Fn(&Machine) -> bool,
    pub can_look: bool,
}

/// What the palette offers now, in its own order: the shown machine's actions and its other
/// sections, then each other machine's, then the app's own.
pub fn entries(now: &Now) -> Vec<Entry> {
    let mut list = Vec::new();
    let shown = match &now.here {
        Here::Machine(name, section) => Some((name.as_str(), *section)),
        _ => None,
    };
    let machines = now.machines.unwrap_or_default();
    let shown_machine = shown.and_then(|(name, _)| machines.iter().find(|machine| machine.name == name));
    if let (Some(machine), Some((_, section))) = (shown_machine, shown) {
        let group = Group::Machine { name: machine.name.clone(), state: machine.state };
        let mut add = |key: &str, name: &str, hint: &str, steps: Vec<Step>| {
            list.push(Entry {
                key: format!("{key}:{}", machine.name),
                name: name.to_owned(),
                hint: hint.to_owned(),
                group: group.clone(),
                steps,
            });
        };
        if can_open_screen(machine, (now.addressed)(machine)) {
            add("screen", "Open screen", "", vec![Step::Screen]);
        }
        if can_start(machine) {
            add("start", "Start", "boots it", vec![Step::Start]);
        }
        if can_ask_to_stop(machine) {
            add("stop", "Stop", "asks its guest to shut down", vec![Step::Stop]);
        }
        for other in
            Section::ALL.into_iter().filter(|other| other.offered(machine.system) && *other != section)
        {
            add(&format!("section {}", other.label()), other.label(), "section", vec![Step::Section(other)]);
        }
    }
    for machine in
        machines.iter().filter(|machine| Some(machine.name.as_str()) != shown.map(|(name, _)| name))
    {
        let group = Group::Machine { name: machine.name.clone(), state: machine.state };
        let name = &machine.name;
        let open = Step::Open(name.clone());
        let mut add = |key: &str, words: String, hint: &str, steps: Vec<Step>| {
            list.push(Entry {
                key: format!("{key}:{name}"),
                name: words,
                hint: hint.to_owned(),
                group: group.clone(),
                steps,
            });
        };
        add("go", format!("Go to {name}"), "", vec![open.clone()]);
        if can_open_screen(machine, (now.addressed)(machine)) {
            add("screen", format!("Open {name}'s screen"), "", vec![open.clone(), Step::Screen]);
        }
        if can_start(machine) {
            add("start", format!("Start {name}"), "boots it", vec![open.clone(), Step::Start]);
        }
        if can_ask_to_stop(machine) {
            add("stop", format!("Stop {name}"), "asks its guest to shut down", vec![open, Step::Stop]);
        }
    }
    let mut app = |key: &str, name: &str, hint: &str, steps: Vec<Step>| {
        list.push(Entry {
            key: key.to_owned(),
            name: name.to_owned(),
            hint: hint.to_owned(),
            group: Group::App,
            steps,
        });
    };
    if now.machines.is_some() && now.here != Here::Home {
        app("home", "Home", "every machine", vec![Step::Home]);
    }
    if now.machines.is_some() {
        app("new", "New machine", "", vec![Step::NewMachine]);
    }
    app("settings", "Settings", "the app's own", vec![Step::Settings]);
    app("report", "Report a problem", "", vec![Step::Report]);
    if now.can_look {
        app("look", "Look for updates", "in Settings", vec![Step::Settings, Step::Look]);
    }
    list
}

/// The words of `text`, lowercase, split at spaces and at the dashes, underscores and apostrophes
/// inside a name, with each whole name too.
fn words(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for word in text.split_whitespace() {
        let word = word.to_lowercase();
        found.extend(word.split(['-', '_', '\'']).filter(|part| !part.is_empty()).map(str::to_owned));
        found.push(word);
    }
    found
}

/// The entries `query` finds, in the order described at the top: each word of the query begins a
/// word of the entry's name or its machine's, in any case. An empty query finds them all.
pub fn matching(entries: &[Entry], query: &str) -> Vec<Entry> {
    let asked: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if asked.is_empty() {
        return entries.to_vec();
    }
    let whole = query.trim().to_lowercase();
    let mut runs: [Vec<Entry>; 3] = Default::default();
    for entry in entries {
        let mut known = words(&entry.name);
        if let Some(machine) = entry.machine() {
            known.extend(words(machine));
        }
        if !asked.iter().all(|word| known.iter().any(|known| known.starts_with(word.as_str()))) {
            continue;
        }
        let run = if entry.name.to_lowercase().starts_with(&whole) {
            0
        } else if entry.machine().is_some_and(|machine| machine.to_lowercase().starts_with(asked[0].as_str()))
        {
            1
        } else {
            2
        };
        runs[run].push(entry.clone());
    }
    runs.into_iter().flatten().collect()
}

/// A key pressed in the palette's box, by its virtual-key code: Up and Down move, Enter runs.
/// Escape is the dialog's own, which closes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Run,
}

impl Key {
    pub fn of(key: u32) -> Option<Key> {
        match key {
            0x26 => Some(Key::Up),
            0x28 => Some(Key::Down),
            0x0D => Some(Key::Run),
            _ => None,
        }
    }
}

/// Whether a key pressed is Ctrl+K, which opens the palette: K with Control, and neither Alt nor
/// Windows, so AltGr's characters and Windows' own shortcuts are left alone.
pub fn is_palette_key(key: u32, control: bool, alt: bool, windows: bool) -> bool {
    const K: u32 = 0x4B;
    key == K && control && !alt && !windows
}

/// Where the highlight goes from `at` on `key`, in a list `count` long: it stops at either end.
pub fn moved(at: usize, key: Key, count: usize) -> usize {
    match key {
        Key::Up => at.saturating_sub(1),
        Key::Down => (at + 1).min(count.saturating_sub(1)),
        Key::Run => at,
    }
}

#[cfg(test)]
mod tests;
