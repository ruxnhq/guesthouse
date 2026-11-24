//! The answer to each request on the service's pipe. The service runs as the system for a user who
//! isn't an administrator, so it keeps every rule itself: what may be named, how big a machine may
//! be, which files it reads for the user, and what may be done to a machine in each state. The work
//! and the pipe are behind [`Work`] and [`Connection`], so all of it is tested without either.

use crate::machines::{Entry, Machines};
use crate::store::{Settings, Store, usable_name};
use guesthouse_control::{
    Answer, Card, CardInfo, Event, Machine, MachineSettings, NewMachine, Request, State, System,
};
use guesthouse_host_windows::events::Sink;
use guesthouse_host_windows::machine::Guest;
use guesthouse_host_windows::record::Record;
use guesthouse_host_windows::settings::{BootSettings, GpuChoice, PrepareSettings};
use std::fs;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// The smallest and largest a machine may be.
const MEMORY_MB: (u64, u64) = (2048, 256 * 1024);
const SIZE_GB: (u64, u64) = (32, 4096);
/// How often a window that's watching is checked for having gone, while nothing happens.
const CHECK: Duration = Duration::from_secs(1);
/// How long the list of cards is kept before it's read again, as the app asks for it every moment.
const CARDS_KEPT: Duration = Duration::from_secs(30);

/// The work done to machines, which only Windows can do.
pub trait Work: Send + Sync + 'static {
    fn prepare(&self, settings: &PrepareSettings, sink: &dyn Sink) -> Result<(), String>;
    fn boot(&self, settings: &BootSettings, sink: Arc<dyn Sink>, stop: Arc<AtomicBool>)
    -> Result<(), String>;
    /// The cards that can be shared with a guest.
    fn cards(&self) -> Vec<CardInfo>;
    /// Stops at once the machine HCS knows by `id`, if there is one.
    fn end(&self, id: &str);
    /// Who may open a machine's console: the user signed in at the console, and administrators.
    fn console_access(&self) -> Vec<String>;
}

/// The app window a request came from.
pub trait Connection {
    fn send(&mut self, answer: &Answer) -> io::Result<()>;
    /// Whether the window has closed its end.
    fn gone(&mut self) -> bool;
    /// Whether the user asking may read the file at `path`, as the service, which may read any file,
    /// opens only what the user could.
    fn may_read(&mut self, path: &Path) -> Result<(), String>;
}

/// HCS's ID for a machine, which also names its pipes. `boot` stops any machine with its ID that's
/// still there, so it's never another program's.
pub use guesthouse_control::vm_id;

pub struct Service<W: Work> {
    machines: Arc<Machines>,
    work: Arc<W>,
    /// The machine being made, as two machines made at once fight over drive letters and diskpart.
    making: Mutex<Option<String>>,
    cards: Mutex<Option<(Instant, Vec<CardInfo>)>>,
}

impl<W: Work> Service<W> {
    /// The service, with the machines in `store`. Any of them still running from before the service
    /// last stopped is stopped, as nothing is watching it now.
    pub fn new(store: Store, work: W) -> Arc<Self> {
        let machines = Machines::new(store);
        let names: Vec<String> = machines.lock().keys().cloned().collect();
        for name in names {
            work.end(&vm_id(&name));
        }
        Arc::new(Service {
            machines,
            work: Arc::new(work),
            making: Mutex::new(None),
            cards: Mutex::new(None),
        })
    }

    pub fn machines(&self) -> &Arc<Machines> {
        &self.machines
    }

    /// Answers `request` on `connection`. A write that fails means the window has gone, and the
    /// work it asked for goes on without it.
    pub fn answer(self: &Arc<Self>, request: Request, connection: &mut dyn Connection) {
        let answer = match request {
            Request::Status => self.status(),
            Request::Create { machine } => return self.create(machine, connection),
            Request::Start { name } => self.start(&name),
            Request::Stop { name } => self.stop(&name),
            Request::End { name } => self.end(&name),
            Request::Screen { name } => self.screen(&name),
            Request::Delete { name } => self.delete(&name),
            Request::Account { name } => self.account(&name),
            Request::Watch { name } => return self.watch(&name, connection),
            Request::Change { name, settings } => self.change(&name, settings),
        };
        let _ = connection.send(&answer);
    }

    /// Asks every running machine's guest to shut down, waits up to `grace` for them, and stops
    /// the rest at once, for when the service itself stops.
    pub fn stop_all(&self, grace: Duration) {
        for entry in self.machines.lock().values() {
            if let Some(stop) = &entry.stop {
                stop.store(true, Ordering::SeqCst);
            }
        }
        let until = Instant::now() + grace;
        while Instant::now() < until && self.machines.lock().values().any(|entry| entry.stop.is_some()) {
            thread::sleep(Duration::from_millis(250));
        }
        let left: Vec<String> = self
            .machines
            .lock()
            .iter()
            .filter(|(_, entry)| entry.stop.is_some())
            .map(|(name, _)| name.clone())
            .collect();
        for name in left {
            self.work.end(&vm_id(&name));
        }
    }

    fn cards(&self) -> Vec<CardInfo> {
        let mut kept = self.cards.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match &*kept {
            Some((when, cards)) if when.elapsed() < CARDS_KEPT => cards.clone(),
            _ => {
                let cards = self.work.cards();
                *kept = Some((Instant::now(), cards.clone()));
                cards
            }
        }
    }

    fn status(&self) -> Answer {
        let cards = self.cards();
        let default_card = cards.iter().find(|card| card.default).map(|card| card.name.clone());
        let machines = self
            .machines
            .lock()
            .iter()
            .map(|(name, entry)| {
                let record = Record::load(&self.machines.store.disk(name)).unwrap_or_default();
                Machine {
                    name: name.clone(),
                    system: match record.guest {
                        Guest::Windows => System::Windows,
                        Guest::Linux => System::Linux,
                    },
                    state: entry.state,
                    set_up: record.installed,
                    memory_mb: entry.settings.memory_mb,
                    processors: entry.settings.processors,
                    card: match &entry.settings.card {
                        Card::Default => default_card.clone(),
                        Card::Named(name) => Some(name.clone()),
                        Card::None => None,
                    },
                    chosen_card: entry.settings.card.clone(),
                    nvapi_wrapper: entry.settings.nvapi_wrapper,
                    address: entry.settings.address.clone(),
                }
            })
            .collect();
        Answer::Status { version: guesthouse_control::WINDOWS_VERSION.into(), machines, cards }
    }

    /// Why a machine can't be given `memory_mb` and `processors`, if it can't.
    fn size_refusal(memory_mb: u64, processors: u32) -> Option<String> {
        let most = thread::available_parallelism().map_or(1, |count| count.get() as u32);
        if !(MEMORY_MB.0..=MEMORY_MB.1).contains(&memory_mb) {
            return Some(format!("A machine has {} to {} MB of memory.", MEMORY_MB.0, MEMORY_MB.1));
        }
        (!(1..=most).contains(&processors))
            .then(|| format!("A machine has 1 to {most} processors, as many as this PC has."))
    }

    /// Changes what a machine is given at each boot, while it isn't running, and keeps it.
    fn change(&self, asked: &str, wanted: MachineSettings) -> Answer {
        let Some(name) = self.machines.find(asked) else { return no_machine(asked) };
        if let Some(reason) = Self::size_refusal(wanted.memory_mb, wanted.processors) {
            return Answer::Refused { reason };
        }
        if let Card::Named(card) = &wanted.card
            && !self.cards().iter().any(|known| &known.name == card)
        {
            return Answer::Refused { reason: format!("This PC has no card called {card} to share.") };
        }
        let mut entries = self.machines.lock();
        let entry = entries.get_mut(&name).expect("found just now");
        if !entry.idle() {
            return Answer::Refused {
                reason: format!("{name} is {}. Its settings change while it's off.", words(entry.state)),
            };
        }
        let settings = Settings {
            memory_mb: wanted.memory_mb,
            processors: wanted.processors,
            card: wanted.card,
            nvapi_wrapper: wanted.nvapi_wrapper,
            ..entry.settings.clone()
        };
        match self.machines.store.save(&name, &settings) {
            Ok(()) => {
                entry.settings = settings;
                Answer::Ok
            }
            Err(reason) => Answer::Refused { reason },
        }
    }

    /// Why `machine` can't be made, if it can't.
    fn refusal(&self, machine: &NewMachine, connection: &mut dyn Connection) -> Option<String> {
        if !usable_name(&machine.name) {
            return Some(
                "A machine's name is up to 40 letters, digits, - and _, and not one Windows keeps for a device."
                    .into(),
            );
        }
        if self.machines.find(&machine.name).is_some() || self.machines.store.folder(&machine.name).exists() {
            return Some(format!("There is a machine called {} already.", machine.name));
        }
        if let Some(reason) = Self::size_refusal(machine.memory_mb, machine.processors) {
            return Some(reason);
        }
        if !(SIZE_GB.0..=SIZE_GB.1).contains(&machine.size_gb) {
            return Some(format!("A machine's disk is {} to {} GB.", SIZE_GB.0, SIZE_GB.1));
        }
        let image = &machine.image;
        if !image.is_absolute() || image.to_string_lossy().starts_with(r"\\") {
            return Some(format!("{} isn't a file on this PC's own drives.", image.display()));
        }
        // Opened as the user first: the service looking for the file itself would tell anyone
        // whether a file is there in a folder they can't look in.
        if let Err(reason) = connection.may_read(image) {
            return Some(reason);
        }
        (!image.is_file()).then(|| format!("{} isn't a file.", image.display()))
    }

    fn create(self: &Arc<Self>, machine: NewMachine, connection: &mut dyn Connection) {
        if let Some(reason) = self.refusal(&machine, connection) {
            let _ = connection.send(&Answer::Refused { reason });
            return;
        }
        let name = machine.name.clone();
        {
            let mut making = self.making.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(other) = &*making {
                let reason = format!("{other} is being made. One machine is made at a time, so wait for it.");
                let _ = connection.send(&Answer::Refused { reason });
                return;
            }
            if let Err(reason) = self.machines.store.make(&name) {
                let _ = connection.send(&Answer::Refused { reason });
                return;
            }
            *making = Some(name.clone());
        }
        let settings = Settings {
            memory_mb: machine.memory_mb,
            processors: machine.processors,
            card: machine.card.clone(),
            address: None,
            nvapi_wrapper: true,
        };
        self.machines.lock().insert(name.clone(), Entry::new(settings, State::Preparing));
        let (_, events) = self.machines.watch(&name).expect("the machine was just added");
        self.machines.tell(&name, Event::State(State::Preparing));

        let service = Arc::clone(self);
        let work = thread::spawn(move || service.make(machine));
        let mut window_gone = false;
        let mut pass_on = |event: Event, connection: &mut dyn Connection| {
            if !window_gone && connection.send(&Answer::Event { event }).is_err() {
                window_gone = true;
            }
        };
        loop {
            match events.recv_timeout(CHECK) {
                Ok(event) => pass_on(event, connection),
                Err(RecvTimeoutError::Timeout) if work.is_finished() => break,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        while let Ok(event) = events.try_recv() {
            pass_on(event, connection);
        }
        let reason =
            work.join().unwrap_or_else(|_| Err("Making the machine failed unexpectedly.".into())).err();
        let _ = connection.send(&Answer::Done { reason });
    }

    /// Makes a new machine's disks, on a thread of its own, as it goes on if the window that asked
    /// closes. A machine that couldn't be made is deleted, folder and all, so its name is free again.
    fn make(&self, machine: NewMachine) -> Result<(), String> {
        let name = machine.name.clone();
        let sink = self.machines.sink(&name);
        let store = &self.machines.store;
        let disk = store.disk(&name);
        let settings = PrepareSettings {
            size_gb: machine.size_gb,
            gpu: gpu_choice(&machine.card),
            ..PrepareSettings::new(disk.clone(), machine.image.clone())
        };
        let made = self
            .work
            .prepare(&settings, sink.as_ref())
            .and_then(|()| keep_installer(store, &name, sink.as_ref()));
        let made = made.and_then(|()| {
            let settings = self.machines.lock().get(&name).map(|entry| entry.settings.clone());
            settings.map_or(Ok(()), |settings| store.save(&name, &settings))
        });
        match &made {
            Ok(()) => self.machines.tell(&name, Event::State(State::Stopped)),
            Err(error) => {
                self.machines.tell(&name, Event::Problem(error.clone()));
                self.machines.lock().remove(&name);
                if let Err(error) = store.remove(&name) {
                    // Nothing is watching the machine now, so this is the one place to say it.
                    eprintln!("The folder of a machine that couldn't be made was left: {error}");
                }
            }
        }
        *self.making.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        made
    }

    fn start(self: &Arc<Self>, asked: &str) -> Answer {
        let Some(name) = self.machines.find(asked) else { return no_machine(asked) };
        let stop = Arc::new(AtomicBool::new(false));
        {
            let mut entries = self.machines.lock();
            let entry = entries.get_mut(&name).expect("found just now");
            if !entry.idle() {
                return Answer::Refused { reason: format!("{name} is {}.", words(entry.state)) };
            }
            entry.stop = Some(Arc::clone(&stop));
            entry.state = State::Starting;
        }
        let store = &self.machines.store;
        let disk = store.disk(&name);
        drop_installer(store, &name, &disk);
        let entry_settings = self.machines.lock()[&name].settings.clone();
        let settings = BootSettings {
            memory_mb: entry_settings.memory_mb,
            processors: entry_settings.processors,
            gpu: gpu_choice(&entry_settings.card),
            console_access: self.work.console_access(),
            nvapi_wrapper: entry_settings.nvapi_wrapper,
            ..BootSettings::new(&vm_id(&name), disk)
        };
        let service = Arc::clone(self);
        thread::spawn(move || {
            let sink = service.machines.sink(&name);
            if let Err(error) = service.work.boot(&settings, Arc::clone(&sink), stop) {
                sink.problem(error);
            }
            let mut entries = service.machines.lock();
            if let Some(entry) = entries.get_mut(&name) {
                entry.stop = None;
                entry.state = State::Stopped;
            }
        });
        Answer::Ok
    }

    fn stop(&self, asked: &str) -> Answer {
        let Some(name) = self.machines.find(asked) else { return no_machine(asked) };
        let entries = self.machines.lock();
        match &entries[&name].stop {
            Some(stop) => {
                stop.store(true, Ordering::SeqCst);
                Answer::Ok
            }
            None => Answer::Refused { reason: format!("{name} isn't running.") },
        }
    }

    /// Stops a running machine at once. Its boot sees it stopped and ends as for any stop.
    fn end(&self, asked: &str) -> Answer {
        let Some(name) = self.machines.find(asked) else { return no_machine(asked) };
        if self.machines.lock()[&name].stop.is_none() {
            return Answer::Refused { reason: format!("{name} isn't running.") };
        }
        self.machines.tell(&name, Event::Note("Stopping it at once, as asked.".into()));
        self.work.end(&vm_id(&name));
        Answer::Ok
    }

    fn screen(&self, asked: &str) -> Answer {
        let Some(name) = self.machines.find(asked) else { return no_machine(asked) };
        let disk = self.machines.store.disk(&name);
        let record = Record::load(&disk).unwrap_or_default();
        let Some(host_fingerprint) = record.screen else {
            return Answer::Refused {
                reason: format!(
                    "{name} has no screen yet. A Windows guest gets one at a boot after remoteman's host's files are beside the service."
                ),
            };
        };
        let Some(address) = self.machines.lock()[&name].settings.address.clone() else {
            return Answer::Refused { reason: format!("{name} hasn't said its address yet.") };
        };
        let client = guesthouse_host_windows::screen::state_beside(&disk).join("client");
        let read =
            |file: &str| fs::read(client.join(file)).map(|bytes| guesthouse_control::screen::hex(&bytes));
        match (read("cert.der"), read("key.der")) {
            (Ok(viewer_certificate), Ok(viewer_key)) => Answer::Screen {
                address,
                port: guesthouse_host_windows::screen::PORT,
                host_fingerprint,
                viewer_certificate,
                viewer_key,
            },
            _ => Answer::Refused {
                reason: format!("{name}'s viewer identity is missing from {}.", client.display()),
            },
        }
    }

    fn delete(&self, asked: &str) -> Answer {
        let Some(name) = self.machines.find(asked) else { return no_machine(asked) };
        let mut entries = self.machines.lock();
        let entry = &entries[&name];
        if !entry.idle() {
            return Answer::Refused { reason: format!("{name} is {}. Stop it first.", words(entry.state)) };
        }
        match self.machines.store.remove(&name) {
            Ok(()) => {
                entries.remove(&name);
                Answer::Ok
            }
            Err(reason) => Answer::Refused { reason },
        }
    }

    fn account(&self, asked: &str) -> Answer {
        let Some(name) = self.machines.find(asked) else { return no_machine(asked) };
        match self.machines.store.account(&name) {
            Ok((user, password)) => Answer::Account { user, password },
            Err(reason) => Answer::Refused { reason },
        }
    }

    fn watch(&self, asked: &str, connection: &mut dyn Connection) {
        let Some((history, events)) = self.machines.find(asked).and_then(|name| self.machines.watch(&name))
        else {
            let _ = connection.send(&no_machine(asked));
            return;
        };
        for event in history {
            if connection.send(&Answer::Event { event }).is_err() {
                return;
            }
        }
        loop {
            match events.recv_timeout(CHECK) {
                Ok(event) => {
                    if connection.send(&Answer::Event { event }).is_err() {
                        return;
                    }
                }
                Err(RecvTimeoutError::Timeout) if connection.gone() => return,
                Err(RecvTimeoutError::Timeout) => {}
                // The machine was deleted, or the window fell too far behind.
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

fn gpu_choice(card: &Card) -> GpuChoice {
    match card {
        Card::Default => GpuChoice::Default,
        Card::Named(name) => GpuChoice::Matching(name.clone()),
        Card::None => GpuChoice::None,
    }
}

fn no_machine(name: &str) -> Answer {
    Answer::Refused { reason: format!("There is no machine called {name}.") }
}

fn words(state: State) -> &'static str {
    match state {
        State::Preparing => "being made",
        State::Starting => "starting",
        State::Running => "running",
        State::Stopping => "shutting down",
        State::Stopped => "stopped",
    }
}

/// A Linux guest's installer, as `prepare` records it, is the ISO the user named. Each boot lets
/// the machine's own account open every disk it's given, which changes the file's permissions, and
/// the service mustn't do that to a user's file, so the machine is given a copy in its folder.
fn keep_installer(store: &Store, name: &str, sink: &dyn Sink) -> Result<(), String> {
    let disk = store.disk(name);
    let mut record = Record::load(&disk)?;
    let Some(installer) = record.installer.clone().filter(|_| record.guest == Guest::Linux) else {
        return Ok(());
    };
    let copy = store.installer(name);
    if installer == copy {
        return Ok(());
    }
    sink.note("Copying the installer into the machine's folder, as the machine is only given the service's own files.");
    fs::copy(&installer, &copy)
        .map_err(|error| format!("Copying {} to {}: {error}", installer.display(), copy.display()))?;
    record.installer = Some(copy);
    record.save(&disk)
}

/// The installer's copy, once the guest has installed and it's no longer attached.
fn drop_installer(store: &Store, name: &str, disk: &Path) {
    let Ok(mut record) = Record::load(disk) else { return };
    let copy = store.installer(name);
    if record.installed
        && record.installer.as_deref() == Some(copy.as_path())
        && fs::remove_file(&copy).is_ok()
    {
        record.installer = None;
        let _ = record.save(disk);
    }
}

#[cfg(test)]
mod tests;
