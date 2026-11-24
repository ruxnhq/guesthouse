//! What each machine is doing, and the app windows watching it. A machine's events come from the
//! thread making or running it and from the thread reading its serial port, and each goes to every
//! window watching that machine. Neither thread may ever wait for a window: an event is kept in the
//! machine's recent history and put in each window's own queue, and a window whose queue is full,
//! as one that stopped reading leaves it, is let go.

use crate::store::{Settings, Store};
use guesthouse_control::{Event, State, address_in};
use guesthouse_host_windows::events::Sink;
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard};

/// How many of a machine's events a window that starts watching is sent first.
const HISTORY: usize = 500;
/// How many events a window may be behind before it's let go.
const QUEUE: usize = 2000;

/// One machine, as the service knows it now.
#[derive(Debug)]
pub struct Entry {
    pub settings: Settings,
    pub state: State,
    /// Set while the machine runs: once asks its guest to shut down, again stops it at once.
    pub stop: Option<Arc<AtomicBool>>,
    /// What's being done to the stopped machine, such as taking a snapshot, if anything.
    pub busy: Option<String>,
    history: VecDeque<Event>,
    watchers: Vec<SyncSender<Event>>,
}

impl Entry {
    pub fn new(settings: Settings, state: State) -> Self {
        Entry { settings, state, stop: None, busy: None, history: VecDeque::new(), watchers: Vec::new() }
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
        let saved = {
            let mut entries = self.lock();
            let Some(entry) = entries.get_mut(name) else { return };
            let mut address_changed = false;
            match &event {
                Event::State(state) => entry.state = *state,
                Event::Guest(line) | Event::Report(line) => {
                    if let Some(address) = address_in(line)
                        && entry.settings.address.as_deref() != Some(address.as_str())
                    {
                        entry.settings.address = Some(address);
                        address_changed = true;
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
            (address_changed && entry.state != State::Preparing).then(|| entry.settings.clone())
        };
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
        let _ = std::fs::remove_dir_all(machines.store.root());
    }

    #[test]
    fn a_machine_is_found_whatever_case_it_is_asked_in() {
        let machines = machines("case");
        assert_eq!(machines.find("WIN").as_deref(), Some("win"));
        assert_eq!(machines.find("ubuntu"), None);
        let _ = std::fs::remove_dir_all(machines.store.root());
    }
}
