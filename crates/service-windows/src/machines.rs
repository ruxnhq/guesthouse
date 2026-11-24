//! What each machine is doing, and the app windows watching it. A machine's events come from the
//! thread making or running it and from the thread reading its serial port, and each goes to every
//! window watching that machine. Neither thread may ever wait for a window: an event is kept in the
//! machine's recent history and put in each window's own queue, and a window whose queue is full,
//! as one that stopped reading leaves it, is let go.

use crate::store::{Settings, Store};
use guesthouse_control::report::clock;
use guesthouse_control::{Event, State, address_in};
use guesthouse_host_windows::events::Sink;
use std::collections::{BTreeMap, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard};

/// How many of a machine's events a window that starts watching is sent first.
const HISTORY: usize = 500;
/// How many events a window may be behind before it's let go.
const QUEUE: usize = 2000;
/// How long a machine's events file may grow before it's kept as the one before and begun afresh.
const EVENTS_LIMIT: u64 = 1024 * 1024;

/// One machine, as the service knows it now.
#[derive(Debug)]
pub struct Entry {
    pub settings: Settings,
    pub state: State,
    /// Set while the machine runs: once asks its guest to shut down, again stops it at once.
    pub stop: Option<Arc<AtomicBool>>,
    /// What's being done to the stopped machine, such as taking a snapshot, if anything.
    pub busy: Option<String>,
    /// The address the guest has reported since this run began, which its forwards lead to. The
    /// settings keep the last run's, which may not be this run's.
    pub run_address: Option<String>,
    history: VecDeque<Event>,
    watchers: Vec<SyncSender<Event>>,
}

impl Entry {
    pub fn new(settings: Settings, state: State) -> Self {
        Entry {
            settings,
            state,
            stop: None,
            busy: None,
            run_address: None,
            history: VecDeque::new(),
            watchers: Vec::new(),
        }
    }

    /// Whether nothing is being done with the machine, so it may be started or deleted.
    pub fn idle(&self) -> bool {
        self.state == State::Stopped && self.stop.is_none() && self.busy.is_none()
    }
}

pub struct Machines {
    pub store: Store,
    entries: Mutex<BTreeMap<String, Entry>>,
}

impl Machines {
    pub fn new(store: Store) -> Arc<Self> {
        let entries = store
            .load()
            .into_iter()
            .map(|(name, settings)| (name, Entry::new(settings, State::Stopped)))
            .collect();
        Arc::new(Machines { store, entries: Mutex::new(entries) })
    }

    /// The machines, to read or change. A thread that panicked while holding them left nothing half
    /// done that matters more than answering, so its poison is passed over.
    pub fn lock(&self) -> MutexGuard<'_, BTreeMap<String, Entry>> {
        self.entries.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The name a machine is kept under, whatever case `name` is asked in, as Windows' folders take
    /// no notice of case either.
    pub fn find(&self, name: &str) -> Option<String> {
        self.lock().keys().find(|kept| kept.eq_ignore_ascii_case(name)).cloned()
    }

    /// Takes one of a machine's events: what it says of the machine's state and address is kept,
    /// and it goes to every window watching.
    pub fn tell(&self, name: &str, event: Event) {
        let (saved, kept) = {
            let mut entries = self.lock();
            let Some(entry) = entries.get_mut(name) else { return };
            let kept = event.clone();
            let mut address_changed = false;
            match &event {
                Event::State(state) => entry.state = *state,
                Event::Guest(line) | Event::Report(line) => {
                    if let Some(address) = address_in(line) {
                        if entry.settings.address.as_deref() != Some(address.as_str()) {
                            entry.settings.address = Some(address.clone());
                            address_changed = true;
                        }
                        entry.run_address = Some(address);
                    }
                }
                Event::Note(_) | Event::Problem(_) => {}
            }
            if entry.history.len() == HISTORY {
                entry.history.pop_front();
            }
            entry.history.push_back(event.clone());
            entry.watchers.retain(|watcher| match watcher.try_send(event.clone()) {
                Ok(()) => true,
                Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => false,
            });
            // Only a machine that has been made has settings to save.
            ((address_changed && entry.state != State::Preparing).then(|| entry.settings.clone()), kept)
        };
        // The file is written with the machines free, as a slow disk must not hold up the others.
        keep_event(&self.store.events(name), &self.store.events_before(name), clock::now(), &kept);
        if let Some(settings) = saved
            && let Err(error) = self.store.save(name, &settings)
        {
            self.tell(name, Event::Problem(format!("The guest's address couldn't be kept: {error}")));
        }
    }

    /// A machine's recent events, and a queue of each one after them, until the machine is deleted.
    pub fn watch(&self, name: &str) -> Option<(Vec<Event>, Receiver<Event>)> {
        let mut entries = self.lock();
        let entry = entries.get_mut(name)?;
        let (sender, receiver) = sync_channel(QUEUE);
        entry.watchers.push(sender);
        Some((entry.history.iter().cloned().collect(), receiver))
    }

    /// Where the work on machine `name` says what it's doing.
    pub fn sink(self: &Arc<Self>, name: &str) -> Arc<dyn Sink> {
        Arc::new(Told { machines: Arc::clone(self), name: name.to_owned() })
    }
}

/// Adds `event` to the machine's events file as a line, and begins the file afresh when it has
/// grown past [`EVENTS_LIMIT`]. A report reads these, so a failure here is let go: the event has
/// been told to the windows already, and the machine mustn't stop for a file.
fn keep_event(path: &Path, before: &Path, seconds: u64, event: &Event) {
    let Some(line) = event_line(seconds, event) else { return };
    if fs::metadata(path).is_ok_and(|meta| meta.len() >= EVENTS_LIMIT) {
        let _ = fs::rename(path, before);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(line.as_bytes());
    }
}

/// An event as one line of the events file: `2026-01-02 03:04:05 UTC note text`.
fn event_line(seconds: u64, event: &Event) -> Option<String> {
    let value = serde_json::to_value(event).ok()?;
    let kind = value.get("kind")?.as_str()?;
    let text = value.get("text")?.as_str()?;
    let text = text.replace(['\r', '\n'], " ");
    Some(format!("{} {kind} {text}\n", clock::utc(seconds)))
}

struct Told {
    machines: Arc<Machines>,
    name: String,
}

impl Sink for Told {
    fn event(&self, event: Event) {
        self.machines.tell(&self.name, event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guesthouse_control::Card;

    fn machines(test: &str) -> Arc<Machines> {
        let root = std::env::temp_dir().join(format!("gh-machines-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let store = Store::new(root);
        store.make("win").unwrap();
        let settings = Settings::basic(8192, 4, Card::Default);
        store.save("win", &settings).unwrap();
        Machines::new(store)
    }

    #[test]
    fn a_watcher_gets_the_history_then_each_new_event() {
        let machines = machines("watch");
        let sink = machines.sink("win");
        sink.note("one");
        let (history, events) = machines.watch("win").unwrap();
        assert_eq!(history, [Event::Note("one".into())]);
        sink.state(State::Running);
        assert_eq!(events.try_recv().unwrap(), Event::State(State::Running));
        assert_eq!(machines.lock()["win"].state, State::Running);
        let _ = std::fs::remove_dir_all(machines.store.root());
    }

    #[test]
    fn a_watcher_that_stops_reading_is_let_go_and_never_waited_for() {
        let machines = machines("slow");
        let (_, events) = machines.watch("win").unwrap();
        for number in 0..QUEUE + 10 {
            machines.tell("win", Event::Note(number.to_string()));
        }
        assert!(machines.lock()["win"].watchers.is_empty());
        assert_eq!(events.iter().count(), QUEUE);
        assert_eq!(machines.lock()["win"].history.len(), HISTORY);
        let _ = std::fs::remove_dir_all(machines.store.root());
    }

    #[test]
    fn the_guests_address_is_kept_in_its_settings() {
        let machines = machines("address");
        machines.tell("win", Event::Report("address: 172.29.1.2".into()));
        assert_eq!(machines.lock()["win"].settings.address.as_deref(), Some("172.29.1.2"));
        assert_eq!(machines.store.load()[0].1.address.as_deref(), Some("172.29.1.2"));
        assert_eq!(machines.lock()["win"].run_address.as_deref(), Some("172.29.1.2"));
        // The same address again, as a guest reports each run, is still this run's.
        machines.lock().get_mut("win").unwrap().run_address = None;
        machines.tell("win", Event::Guest("guesthouse: address: 172.29.1.2".into()));
        assert_eq!(machines.lock()["win"].run_address.as_deref(), Some("172.29.1.2"));
        let _ = std::fs::remove_dir_all(machines.store.root());
    }

    #[test]
    fn an_event_is_one_line_with_its_kind() {
        let line = event_line(0, &Event::Problem("two\nlines\r".into())).unwrap();
        assert_eq!(line, "1970-01-01 00:00:00 UTC problem two lines \n");
        let line = event_line(0, &Event::State(State::Running)).unwrap();
        assert_eq!(line, "1970-01-01 00:00:00 UTC state running\n");
    }

    #[test]
    fn each_event_is_kept_in_the_machines_folder() {
        let machines = machines("kept");
        machines.tell("win", Event::Note("making the disk".into()));
        machines.tell("win", Event::State(State::Running));
        let text = fs::read_to_string(machines.store.events("win")).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with(" UTC note making the disk"), "{}", lines[0]);
        assert!(lines[1].ends_with(" UTC state running"), "{}", lines[1]);
        let _ = fs::remove_dir_all(machines.store.root());
    }

    #[test]
    fn a_long_file_is_kept_as_the_one_before_and_begun_afresh() {
        let machines = machines("rotate");
        let path = machines.store.events("win");
        fs::write(&path, vec![b'x'; EVENTS_LIMIT as usize]).unwrap();
        fs::write(machines.store.events_before("win"), "older").unwrap();
        machines.tell("win", Event::Note("fresh".into()));
        let before = fs::read(machines.store.events_before("win")).unwrap();
        assert_eq!(before.len(), EVENTS_LIMIT as usize, "the full one replaced the older");
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 1);
        assert!(text.ends_with(" UTC note fresh\n"));
        let _ = fs::remove_dir_all(machines.store.root());
    }

    #[test]
    fn a_file_that_cant_be_written_never_stops_the_event() {
        let machines = machines("unwritable");
        // A folder in the file's place can't be opened for writing.
        fs::create_dir(machines.store.events("win")).unwrap();
        let (_, events) = machines.watch("win").unwrap();
        machines.tell("win", Event::Note("still told".into()));
        assert_eq!(events.try_recv().unwrap(), Event::Note("still told".into()));
        let _ = fs::remove_dir_all(machines.store.root());
    }

    #[test]
    fn a_machine_is_found_whatever_case_it_is_asked_in() {
        let machines = machines("case");
        assert_eq!(machines.find("WIN").as_deref(), Some("win"));
        assert_eq!(machines.find("ubuntu"), None);
        let _ = std::fs::remove_dir_all(machines.store.root());
    }
}
