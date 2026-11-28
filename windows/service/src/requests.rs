//! The answer to each request on the service's pipe. The service runs as the system for a user who
//! isn't an administrator, so it keeps every rule itself: what may be named, how big a machine may
//! be, which files it reads for the user, and what may be done to a machine in each state. The work
//! and the pipe are behind [`Work`] and [`Connection`], so all of it is tested without either.

use crate::machines::{Entry, Machines};
use crate::report_parts::tail_file;
use crate::store::{Settings, Store, empty, usable_name};
use disk_tree::{copy_tree, remove_disks, tree};
use guesthouse_control::report::pieces;
use guesthouse_control::{
    Answer, Bridging, Card, CardInfo, DEFAULT_USER, Event, HyperV, Machine, MachineSettings, NetworkMode,
    NewMachine, Request, Share, SnapshotInfo, State, System, forwards_refusal, shares_refusal, user_problem,
    valid_edition,
};
use guesthouse_host_windows::events::Sink;
use guesthouse_host_windows::guest_ctl::GuestCtl;
use guesthouse_host_windows::machine::Guest;
use guesthouse_host_windows::network::Network;
use guesthouse_host_windows::record::Record;
use guesthouse_host_windows::settings::{self as host_settings, BootSettings, GpuChoice, PrepareSettings};
use guesthouse_host_windows::snapshots::{self, Disks};
use guesthouse_host_windows::ssh;
use guesthouse_remote_view_core::pairing::Event as PairingEvent;
use guesthouse_remote_view_core::session::Sessions;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
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
/// How much of each of a machine's own files a report takes: its end, as the newest is what matters.
const REPORT_TAIL: u64 = 2 * 1024 * 1024;
/// How long the list of cards is kept before it's read again, as the app asks for it every moment.
const CARDS_KEPT: Duration = Duration::from_secs(30);
/// How long what bridging offers is kept, for the same reason: the adapters are read from Windows.
const BRIDGING_KEPT: Duration = Duration::from_secs(30);
/// How long it's kept while Hyper-V isn't on, when no adapters are read: the host keeps its read of
/// Hyper-V's feature itself, and reads it again beside a status, never in it.
const HYPER_V_KEPT: Duration = Duration::from_secs(2);
/// How long a machine that's been ended has to take its port forwards away, as the service stops.
const UNMAKE_WAIT: Duration = Duration::from_secs(15);

/// The work done to machines, which only Windows can do.
pub trait Work: Send + Sync + 'static {
    fn prepare(&self, settings: &PrepareSettings, sink: &dyn Sink) -> Result<(), String>;
    /// Runs the machine until it stops. `ctl` is the way to ask remoteman's host in its guest while
    /// it runs, which the boot gives the guest's serial port.
    fn boot(
        &self,
        settings: &BootSettings,
        sink: Arc<dyn Sink>,
        stop: Arc<AtomicBool>,
        ctl: GuestCtl,
    ) -> Result<(), String>;
    /// The cards that can be shared with a guest.
    fn cards(&self) -> Vec<CardInfo>;
    /// Stops at once the machine HCS knows by `id`, if there is one.
    fn end(&self, id: &str);
    /// The owner of the file or folder at `path`, as a SID.
    fn files_owner(&self, path: &Path) -> Option<String>;
    /// Who may open a machine's console: its owner, if it has one, and administrators.
    fn console_access(&self, owner: Option<&str>) -> Vec<String>;
    /// Lets a stopped machine's disk grow to `size_gb`, and never shrinks it.
    fn grow(&self, disk: &Path, size_gb: u64) -> Result<(), String>;
    /// The largest the disk may grow to now, in GB, if it can be read.
    fn disk_size_gb(&self, disk: &Path) -> Option<u64>;
    /// Snapshots' disks.
    fn disks(&self) -> &dyn Disks;
    /// The host's files for a bug report, each by name with its text or why it couldn't be read: the
    /// service's own log, the installer's, and facts about this PC.
    fn report_parts(&self) -> Vec<(String, Result<String, String>)>;

    /// The host's side of machines' networks: port forwards, and bridging.
    fn network(&self) -> &dyn Network;
    /// Whether `port` on this PC's loopback can be listened on now.
    fn port_free(&self, port: u16) -> bool;
    /// Whether a remote view's relay could listen on UDP `port` on every network of this PC now.
    fn udp_port_free(&self, port: u16) -> bool;
    /// Opens a machine's remote view on this PC's network, announced by the machine's name where it
    /// can be, which stays open until what comes back is dropped, and has taken its announcement
    /// back and let go of its port once it has been.
    fn open_view(&self, view: &ViewSettings) -> Result<Opened, String>;
    /// This PC's own addresses now, which a Windows guest lets reach its screen's port.
    fn this_pcs_addresses(&self) -> Vec<std::net::Ipv4Addr> {
        guesthouse_host_windows::screen::this_pcs_addresses()
    }
    /// The address of this PC's that a packet to `guest` leaves from.
    fn source_towards(&self, guest: std::net::Ipv4Addr) -> Option<std::net::Ipv4Addr> {
        guesthouse_host_windows::screen::source_towards(guest)
    }
    /// Makes `folder`, which must not be there yet, for one machine's disks, so that only the system
    /// and administrators may open it or anything made in it. Its parent is the user's, so nothing
    /// is made but this one folder.
    fn private_folder(&self, folder: &Path) -> Result<(), String> {
        fs::create_dir(folder).map_err(|error| format!("{}: {error}", folder.display()))
    }
    /// How many bytes are free on the drive `folder` is on.
    fn free_space(&self, folder: &Path) -> Result<u64, String>;
    /// Writes a line to the service's own log, for what no window may be watching.
    fn log(&self, _text: &str) {}
}

/// The app window a request came from.
pub trait Connection {
    fn send(&mut self, answer: &Answer) -> io::Result<()>;
    /// Whether the window has closed its end.
    fn gone(&mut self) -> bool;
    /// Whether the user asking may read the file at `path`, as the service, which may read any file,
    /// opens only what the user could. What comes back is the path of the file that opened, with
    /// every link on the way resolved, and the file held as the user opened it, so while it's held
    /// it isn't written, moved or replaced, and nor is any folder it's in.
    fn may_read(&mut self, path: &Path) -> Result<(PathBuf, Held), String>;
    /// Whether the user asking may share the folder at `path`: read it, write it unless
    /// `read_only`, and change who may open it. Sharing it lets the machine's own account open it,
    /// which the service does as the system, so only a folder the user could do that to is shared.
    /// It must be the folder itself, by its own path, and not a link to another. What comes back
    /// holds the folder open, so nothing can put another in its place while it's held.
    fn may_share(&mut self, path: &Path, read_only: bool) -> Result<Held, String>;
    /// Whether the user asking may keep a machine in the folder at `path`: make a folder in it. It
    /// must be the folder itself, by its own path, not a link and not reached through one, on one
    /// of this PC's fixed drives. What comes back holds it open while the machine's folder is made.
    fn may_place(&mut self, path: &Path) -> Result<Held, String>;
    /// Who is asking.
    fn asker(&self) -> Asker;
}

/// The folder a check holds open, for as long as it's kept.
pub use crate::store::Held;

pub use owners::Asker;
pub use remote_view::{Opened, Unannounced, ViewSettings, open_relay};

/// HCS's ID for a machine, which also names its pipes. `boot` stops any machine with its ID that's
/// still there, so it's never another program's.
pub use guesthouse_control::vm_id;

/// What a machine's place is given beyond the files being moved to it, in bytes, to be free.
const SPARE: u64 = 64 << 20;
/// A file at least this large is said to be copied, as its copy takes a while.
const LARGE_FILE: u64 = 64 << 20;

pub struct Service<W: Work> {
    machines: Arc<Machines>,
    work: Arc<W>,
    /// The machine being made, as two machines made at once fight over drive letters and diskpart.
    making: Mutex<Option<String>>,
    cards: Mutex<Option<(Instant, Vec<CardInfo>)>>,
    bridging: Mutex<Option<(Instant, Bridging)>>,
    /// How turning Hyper-V on stands, while it's done and after it failed.
    turning: Mutex<hyper_v::Turning>,
    /// Port forwards are made and taken away one machine at a time, as `netsh` is.
    forwarding: Mutex<()>,
    /// Whether the forwards and remote views' rules a stopped service left behind have been looked
    /// for. No machine's are made before they have, or they'd be taken away with the rest.
    cleaned: AtomicBool,
    /// Remote views are turned on one at a time, so two never take the same port.
    switching: Mutex<()>,
    /// What was last read of each running machine's remoteman, and its pairing.
    sessions: Sessions,
    /// Each machine's pairing is driven one step at a time.
    driving: Mutex<std::collections::HashMap<String, Arc<Mutex<()>>>>,
    /// What was found of each machine's guest's address, by the card that answers there.
    checked: Mutex<std::collections::HashMap<String, guest_address::Checked>>,
}

impl<W: Work> Service<W> {
    /// The service, with the machines in `store`. Any of them still running from before the service
    /// last stopped is stopped, as nothing is watching it now.
    pub fn new(store: Store, work: W) -> Arc<Self> {
        let machines = Machines::new(store);
        let names: Vec<String> = machines.lock().keys().cloned().collect();
        for name in names {
            work.end(&vm_id(&name));
            owners::from_files(&machines, &work, &name);
            // The disks of a machine whose place can't be used aren't touched.
            if let Some(problem) = machines.store.problem(&name) {
                work.log(&format!("{name}'s disks can't be used: {problem}"));
                continue;
            }
            // A snapshot a crash cut short is finished or undone before anything uses the disk.
            if let Err(error) = snapshots::repair(&machines.store.disk(&name)) {
                eprintln!("{name}'s snapshots couldn't be put right: {error}");
            }
            // A machine from before its disk's size was kept has it read once.
            {
                let mut entries = machines.lock();
                let entry = entries.get_mut(&name).expect("listed just now");
                if entry.settings.size_gb.is_none()
                    && let Some(size_gb) = work.disk_size_gb(&machines.store.disk(&name))
                {
                    entry.settings.size_gb = Some(size_gb);
                    let _ = machines.store.save(&name, &entry.settings);
                }
            }
            // An Ubuntu guest from before the SSH port has one given now, so the app can say it.
            if let Some(said) = keep_port(&machines, &work, &name).1 {
                work.log(&format!("{name}: {said}"));
            }
        }
        let service = Arc::new(Service {
            machines,
            work: Arc::new(work),
            making: Mutex::new(None),
            cards: Mutex::new(None),
            bridging: Mutex::new(None),
            turning: Mutex::default(),
            forwarding: Mutex::new(()),
            cleaned: AtomicBool::new(false),
            switching: Mutex::new(()),
            sessions: Sessions::default(),
            driving: Mutex::default(),
            checked: Mutex::default(),
        });
        // Port forwards a stopped service left behind are no use to anyone, and hold their ports.
        // Listing the firewall's rules takes seconds, and the service manager is waiting for the
        // service to run, so it's done on a thread of its own.
        let service_again = Arc::clone(&service);
        thread::spawn(move || {
            service_again.clear_leftover_forwards();
            service_again.clear_leftover_views();
            service_again.cleaned.store(true, Ordering::SeqCst);
        });
        service
    }

    pub fn machines(&self) -> &Arc<Machines> {
        &self.machines
    }

    /// Answers `request` on `connection`. A write that fails means the window has gone, and the
    /// work it asked for goes on without it.
    /// A machine that isn't the asker's is answered as one that isn't there.
    pub fn answer(self: &Arc<Self>, request: Request, connection: &mut dyn Connection) {
        let asker = connection.asker();
        self.take_unowned(&asker);
        if let Some(asked) = owners::named(&request).filter(|asked| !self.theirs(asked, &asker)) {
            let _ = connection.send(&no_machine(asked));
            return;
        }
        let answer = match request {
            Request::Status => self.status(&asker),
            Request::Create { machine } => return self.create(machine, connection),
            Request::Start { name } => self.start(&name, connection),
            Request::Stop { name } => self.stop(&name),
            Request::End { name } => self.end(&name),
            Request::Screen { name } => self.screen(&name),
            Request::Delete { name } => self.delete(&name),
            Request::Change { name, settings } => self.change(&name, settings, connection),
            Request::Grow { name, size_gb } => self.grow(&name, size_gb),
            Request::Move { name, folder } => self.move_machine(&name, folder, connection),
            Request::Snapshot { name, label } => self.take_snapshot(&name, label),
            Request::Revert { name, id } => self.revert(&name, id),
            Request::DeleteSnapshot { name, id } => self.delete_snapshot(&name, id),
            Request::Account { name } => self.account(&name),
            Request::Watch { name } => return self.watch(&name, connection),
            Request::ReportFiles { machine } => return self.report_files(machine, connection),
            Request::Share { name, on, pairing, pairing_seconds } => {
                self.share(&name, on, pairing, pairing_seconds)
            }
            Request::RemoteView { name } => self.remote_view(&name),
            Request::PairClose { name } => self.pairing_event(&name, PairingEvent::Stop),
            Request::PairAccept { name } => self.pairing_event(&name, PairingEvent::Accept),
            Request::PairDecline { name } => self.pairing_event(&name, PairingEvent::Decline),
            Request::DeviceAllow { name, fingerprint, what, on } => {
                self.device(&name, |policy| policy.device_allow(&fingerprint, &what, on))
            }
            Request::DeviceRevoke { name, fingerprint } => {
                self.device(&name, |policy| policy.device_revoke(&fingerprint))
            }
            Request::EnableHyperV => return self.enable_hyper_v(connection),
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
        for name in &left {
            self.work.end(&vm_id(name));
        }
        // An ended machine's boot ends, and its forwards are taken away, on their own threads.
        let until = Instant::now() + UNMAKE_WAIT;
        while Instant::now() < until && self.machines.lock().values().any(|entry| entry.stop.is_some()) {
            thread::sleep(Duration::from_millis(50));
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

    /// Whether a machine can be bridged onto this PC's network, and onto which adapters.
    fn bridging(&self) -> Bridging {
        let mut kept = self.bridging.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let bridging = match &*kept {
            Some((when, bridging))
                if when.elapsed()
                    < if bridging.hyper_v == HyperV::On { BRIDGING_KEPT } else { HYPER_V_KEPT } =>
            {
                bridging.clone()
            }
            _ => {
                let bridging = self.work.network().bridging();
                *kept = Some((Instant::now(), bridging.clone()));
                bridging
            }
        };
        drop(kept);
        self.with_turning(bridging)
    }

    /// Reads what bridging offers again at the next look.
    fn forget_bridging(&self) {
        *self.bridging.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    fn status(&self, asker: &Asker) -> Answer {
        // Each place is held again as it stands: one that's gone is a problem, and one that's back
        // isn't.
        let names: Vec<String> = self.machines.lock().keys().cloned().collect();
        for name in &names {
            self.refresh(name);
        }
        let cards = self.cards();
        let bridging = self.bridging();
        let default_card = cards.iter().find(|card| card.default).map(|card| card.name.clone());
        let machines = self
            .machines
            .lock()
            .iter()
            .filter(|(_, entry)| asker.may_use(entry))
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
                    tpm: entry.settings.tpm,
                    network: entry.settings.network,
                    shares: entry.settings.shares.clone(),
                    network_mode: entry.settings.network_mode.clone(),
                    forwards: entry.settings.forwards.clone(),
                    nested_virtualization: entry.settings.nested_virtualization,
                    ssh_port: entry.settings.ssh_port,
                    size_gb: entry.settings.size_gb,
                    folder: self.machines.store.recorded(name),
                    problem: self.machines.store.problem(name),
                    snapshots: snapshots::list(&self.machines.store.disk(name))
                        .unwrap_or_default()
                        .into_iter()
                        .map(|snapshot| SnapshotInfo {
                            id: snapshot.id,
                            label: snapshot.label,
                            taken: snapshot.taken,
                            current: snapshot.current,
                        })
                        .collect(),
                    working: entry.busy.clone(),
                    address: entry.settings.address.clone(),
                    remote_view: entry.remote_view.values().0,
                    remote_port: entry.remote_view.values().1,
                    console_pipe: entry.console_pipe.clone(),
                }
            })
            .collect();
        Answer::Status {
            version: guesthouse_control::WINDOWS_VERSION.into(),
            protocol: guesthouse_control::protocol::PROTOCOL,
            machines,
            cards,
            bridging,
        }
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
        if let Some(problem) = user_problem(&machine.user) {
            return Some(problem);
        }
        if !valid_edition(&machine.edition) {
            return Some(
                "An edition is named with letters, digits and spaces, as Windows names them.".into(),
            );
        }
        if let Some(reason) = shares_refusal(&machine.shares) {
            return Some(reason);
        }
        for share in &machine.shares {
            if let Err(reason) = share_refusal(share, connection) {
                return Some(reason);
            }
        }
        let image = &machine.image;
        (!image.is_absolute() || image.to_string_lossy().starts_with(r"\\"))
            .then(|| format!("{} isn't a file on this PC's own drives.", image.display()))
    }

    /// Opens `machine`'s image as the user, and has it read from then on by the path that open found,
    /// held as the user opened it until the machine is made. The service looking for the file itself
    /// would tell anyone whether a file is there in a folder they can't look in.
    fn image(machine: &mut NewMachine, connection: &mut dyn Connection) -> Result<Held, String> {
        let (image, held) = connection.may_read(&machine.image)?;
        if !image.is_file() {
            return Err(format!("{} isn't a file.", machine.image.display()));
        }
        machine.image = image;
        Ok(held)
    }

    fn create(self: &Arc<Self>, mut machine: NewMachine, connection: &mut dyn Connection) {
        let image =
            self.refusal(&machine, connection).map_or_else(|| Self::image(&mut machine, connection), Err);
        let image = match image {
            Ok(held) => held,
            Err(reason) => {
                let _ = connection.send(&Answer::Refused { reason });
                return;
            }
        };
        let name = machine.name.clone();
        // The folder it's to be kept in is checked as the user, and held until the machine's own
        // folder is made in it.
        let placement = match machine.folder.as_deref().filter(|folder| !folder.as_os_str().is_empty()) {
            Some(folder) => match self.placement(&name, folder, connection) {
                Ok(placement) => Some(placement),
                Err(reason) => {
                    let _ = connection.send(&Answer::Refused { reason });
                    return;
                }
            },
            None => None,
        };
        {
            let mut making = self.making.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(other) = &*making {
                let other = if self.visible(other, &connection.asker()) { other } else { "Another machine" };
                let reason = format!("{other} is being made. One machine is made at a time, so wait for it.");
                let _ = connection.send(&Answer::Refused { reason });
                return;
            }
            if self.turning_on() {
                let reason =
                    "Windows' Hyper-V feature is being turned on. Make the machine once that's done.";
                let _ = connection.send(&Answer::Refused { reason: reason.into() });
                return;
            }
            if let Err(reason) = self.machines.store.make(&name) {
                let _ = connection.send(&Answer::Refused { reason });
                return;
            }
            if let Some((place, _)) = &placement
                && let Err(reason) = self.make_place(&name, place)
            {
                let _ = self.machines.store.remove(&name);
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
            tpm: machine.tpm,
            network: machine.network,
            shares: machine.shares.clone(),
            network_mode: NetworkMode::Shared,
            forwards: Vec::new(),
            nested_virtualization: false,
            ssh_port: None,
            size_gb: Some(machine.size_gb),
            owner: Some(connection.asker().user),
        };
        self.machines.lock().insert(name.clone(), Entry::new(settings, State::Installing));
        let (_, events) = self.machines.watch(&name).expect("the machine was just added");
        self.machines.tell(&name, Event::State(State::Installing));

        let service = Arc::clone(self);
        let work = thread::spawn(move || {
            let _image = image;
            service.make(machine)
        });
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

    /// Why `folder` can't be where `name`'s disks go, if it can't, or the machine's folder in it, which
    /// doesn't exist yet, and the folder held open.
    fn placement(
        &self,
        name: &str,
        folder: &Path,
        connection: &mut dyn Connection,
    ) -> Result<(PathBuf, Held), String> {
        if !folder.is_absolute() || folder.to_string_lossy().starts_with(r"\\") {
            return Err(format!("{} isn't a folder on this PC's own drives.", folder.display()));
        }
        if inside(folder, self.machines.store.root()) {
            return Err(format!(
                "{} is the service's own folder, or in it. Leave the place empty to keep a machine there.",
                folder.display()
            ));
        }
        // Asked as the user first, as for a share.
        let held = connection.may_place(folder)?;
        if !folder.is_dir() {
            return Err(format!("{} isn't a folder.", folder.display()));
        }
        // The folder as Windows has it, with its slashes the right way and its dots resolved.
        let place = std::path::absolute(folder).unwrap_or_else(|_| folder.to_owned()).join(name);
        if place.exists() {
            return Err(format!("{} is there already, so {name} isn't made in it.", place.display()));
        }
        Ok((place, held))
    }

    /// Makes `place`, the folder for the disks of the machine `name`, whose own folder is made,
    /// and gives the machine it.
    fn make_place(&self, name: &str, place: &Path) -> Result<(), String> {
        self.work.private_folder(place)?;
        self.machines.store.locate(name, place).inspect_err(|_| {
            let _ = fs::remove_dir(place);
        })
    }

    /// Makes a new machine's disks, on a thread of its own, as it goes on if the window that asked
    /// closes. A machine that couldn't be made is deleted, folder and all, so its name is free again.
    fn make(&self, machine: NewMachine) -> Result<(), String> {
        let name = machine.name.clone();
        let sink = self.machines.sink(&name);
        let store = &self.machines.store;
        let disk = store.disk(&name);
        let user = if machine.user.is_empty() { DEFAULT_USER.to_owned() } else { machine.user.clone() };
        let settings = PrepareSettings {
            size_gb: machine.size_gb,
            gpu: gpu_choice(&machine.card),
            user,
            edition: machine.edition.trim().to_owned(),
            ..PrepareSettings::new(disk.clone(), machine.image.clone())
        };
        let made = self
            .work
            .prepare(&settings, sink.as_ref())
            .and_then(|()| keep_installer(store, &name, sink.as_ref()));
        // An Ubuntu guest's SSH port is chosen as it's made, so the app can say it from the start.
        let made = made.inspect(|()| {
            if let Some(said) = keep_port(&self.machines, &*self.work, &name).1 {
                self.work.log(&format!("{name}: {said}"));
            }
        });
        let made = made.and_then(|()| {
            let settings = self.machines.lock().get(&name).map(|entry| entry.settings.clone());
            settings.map_or(Ok(()), |settings| store.save(&name, &settings))
        });
        match &made {
            Ok(()) => self.machines.tell(&name, Event::State(State::Off)),
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

    /// Holds a machine's place again, if the machine is doing nothing, as what's done to a machine
    /// that is, such as moving it, has its own hold of it.
    fn refresh(&self, name: &str) {
        if self.machines.lock().get(name).is_some_and(Entry::idle) {
            self.machines.store.refresh(name);
        }
    }

    /// Why a machine's disks can't be used, as an answer, if they can't: their place is gone, or isn't
    /// the service's own any more.
    fn unusable(&self, name: &str) -> Option<Answer> {
        self.refresh(name);
        self.machines
            .store
            .problem(name)
            .map(|problem| Answer::Refused { reason: format!("{name}'s disks can't be used now. {problem}") })
    }

    fn start(self: &Arc<Self>, asked: &str, connection: &mut dyn Connection) -> Answer {
        let Some(name) = self.machines.find(asked) else { return no_machine(asked) };
        if let Some(refused) = self.unusable(&name) {
            return refused;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let ctl = GuestCtl::new();
        {
            let mut entries = self.machines.lock();
            let entry = entries.get_mut(&name).expect("found just now");
            if !entry.idle() {
                return Answer::Refused { reason: format!("{name} is {}.", busy_words(entry)) };
            }
            if self.turning_on() {
                return Answer::Refused {
                    reason: format!(
                        "Windows' Hyper-V feature is being turned on. Start {name} once that's done."
                    ),
                };
            }
            entry.stop = Some(Arc::clone(&stop));
            entry.ctl = Some(ctl.clone());
            entry.boot_addresses = Vec::new();
            entry.state = State::Starting;
            // What the guest said last run isn't where its forwards lead until it says it again.
            entry.run_address = None;
            entry.console_pipe = None;
        }
        let store = &self.machines.store;
        let disk = store.disk(&name);
        drop_installer(store, &name, &disk);
        let entry_settings = self.machines.lock()[&name].settings.clone();
        // Each share is checked again at every start, as whoever is starting the machine, and held
        // open while it runs: what was checked when it was chosen could have been put in a link's
        // place since, and the boot gives the folder to the machine as the system. One refused
        // leaves only itself out, and its slot empty, so the others keep their names in the guest.
        let mut held = Vec::new();
        let mut shares = Vec::new();
        for (slot, share) in entry_settings.shares.iter().enumerate() {
            match share_refusal(share, connection) {
                Ok(folder) => {
                    held.push(folder);
                    shares.push(host_settings::Share {
                        folder: share.folder.clone(),
                        read_only: share.read_only,
                        slot,
                    });
                }
                Err(reason) => self.machines.tell(
                    &name,
                    Event::Problem(format!("This boot doesn't share {}: {reason}", share.folder.display())),
                ),
            }
        }
        let host_addresses = self.work.this_pcs_addresses();
        if let Some(entry) = self.machines.lock().get_mut(&name) {
            entry.boot_addresses = host_addresses.clone();
        }
        // An Ubuntu guest's SSH port, chosen again if another program has taken it since.
        let (ssh_port, said) = keep_port(&self.machines, &*self.work, &name);
        if let Some(said) = said {
            self.work.log(&format!("{name}: {said}"));
            self.machines.tell(&name, Event::Note(said));
        }
        let settings = BootSettings {
            ssh_port,
            memory_mb: entry_settings.memory_mb,
            processors: entry_settings.processors,
            gpu: gpu_choice(&entry_settings.card),
            console_access: self.work.console_access(entry_settings.owner.as_deref()),
            nvapi_wrapper: entry_settings.nvapi_wrapper,
            tpm: entry_settings.tpm,
            nested_virtualization: entry_settings.nested_virtualization,
            network: entry_settings.network,
            shares,
            bridge: match &entry_settings.network_mode {
                NetworkMode::Shared => None,
                NetworkMode::Bridged { adapter } => Some(adapter.clone()),
            },
            // Only this PC may reach the guest's screen, whichever network it's on.
            host_addresses: Some(host_addresses.clone()),
            ..BootSettings::new(&vm_id(&name), disk)
        };
        if settings.bridge.is_some() && !entry_settings.forwards.is_empty() {
            self.machines.tell(
                &name,
                Event::Note(
                    "Its port forwards aren't made while it's bridged: it has an address of its own on the network, \
                     and its ports are reached there."
                        .into(),
                ),
            );
        }
        // What an earlier run read of its remoteman is not this run's.
        self.sessions.remove(&name);
        let service = Arc::clone(self);
        thread::spawn(move || {
            let sink = service.machines.sink(&name);
            // The forwards and the remote view are kept while the machine runs and taken away before
            // it counts as stopped.
            let over = Arc::new(AtomicBool::new(false));
            let forwarder = {
                let (service, name, over) = (Arc::clone(&service), name.clone(), Arc::clone(&over));
                thread::spawn(move || service.keep_forwards(&name, &over))
            };
            let viewer = {
                let (service, name, over) = (Arc::clone(&service), name.clone(), Arc::clone(&over));
                thread::spawn(move || service.keep_remote_view(&name, &over))
            };
            let pairer = {
                let (service, name, over) = (Arc::clone(&service), name.clone(), Arc::clone(&over));
                thread::spawn(move || service.keep_pairing(&name, &over))
            };
            if let Err(error) = service.work.boot(&settings, Arc::clone(&sink), stop, ctl) {
                sink.problem(error);
            }
            over.store(true, Ordering::SeqCst);
            let _ = forwarder.join();
            let _ = viewer.join();
            let _ = pairer.join();
            service.view_stopped(&name);
            drop(held);
            let mut entries = service.machines.lock();
            if let Some(entry) = entries.get_mut(&name) {
                entry.stop = None;
                entry.state = State::Off;
                entry.console_pipe = None;
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
        if let Some(refused) = self.unusable(&name) {
            return refused;
        }
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
            return Answer::Refused { reason: format!("{name} is {}. Stop it first.", busy_words(entry)) };
        }
        match self.machines.store.remove(&name) {
            Ok(()) => {
                entries.remove(&name);
                Answer::Ok
            }
            Err(reason) => Answer::Refused { reason },
        }
    }

    /// Gives a stopped machine what `wanted` says from its next start. A Windows guest's disk
    /// isn't touched: each boot puts the driver of the card it's given on it, and picks the screen's
    /// encoder for that card. An Ubuntu guest's files disk was made only as large as its card's
    /// driver needed, so its card can't change to another.
    fn change(&self, asked: &str, wanted: MachineSettings, connection: &mut dyn Connection) -> Answer {
        let Some(name) = self.machines.find(asked) else { return no_machine(asked) };
        if let Some(reason) = Self::size_refusal(wanted.memory_mb, wanted.processors) {
            return Answer::Refused { reason };
        }
        if let Some(reason) = forwards_refusal(&wanted.forwards) {
            return Answer::Refused { reason };
        }
        if let Some(reason) = shares_refusal(&wanted.shares) {
            return Answer::Refused { reason };
        }
        let (before, shared_before, mode_before) = {
            let entries = self.machines.lock();
            let settings = &entries[&name].settings;
            (settings.card.clone(), settings.shares.clone(), settings.network_mode.clone())
        };
        // A mode is checked only when it changes, as a card is: an adapter that's gone since needn't
        // stop a change of memory.
        if wanted.network_mode != mode_before
            && let NetworkMode::Bridged { adapter } = &wanted.network_mode
        {
            let bridging = self.bridging();
            if !bridging.available {
                return Answer::Refused {
                    reason: bridging.reason.unwrap_or_else(|| "This PC can't bridge a machine.".into()),
                };
            }
            if !bridging.adapters.iter().any(|known| known.name == *adapter) {
                return Answer::Refused {
                    reason: format!(
                        "This PC has no Ethernet or Wi-Fi adapter called {adapter} that's up to bridge onto."
                    ),
                };
            }
        }
        // A share is checked only when it's new, or read-only before and writable now: the user
        // who shared one that's kept may not be the one asking, and needn't be for its memory to
        // change. A share can always go.
        for share in wanted.shares.iter().filter(|share| !shared_before.contains(share)) {
            if let Err(reason) = share_refusal(share, connection) {
                return Answer::Refused { reason };
            }
        }
        if wanted.card != before {
            // Asked before the machines are held, as the cards can take a moment to read.
            let cards = self.cards();
            if let Card::Named(card) = &wanted.card
                && !cards.iter().any(|known| known.name == *card)
            {
                return Answer::Refused { reason: format!("This PC has no card called {card} to share.") };
            }
            let record = Record::load(&self.machines.store.disk(&name)).unwrap_or_default();
            if record.guest == Guest::Linux && resolved(&wanted.card, &cards) != resolved(&before, &cards) {
                return Answer::Refused {
                    reason: format!(
                        "{name}'s files disk was made for the driver of the card it had, so an Ubuntu \
                         guest's card can't change to another yet. Its memory and processors can."
                    ),
                };
            }
        }
        let saved = {
            let mut entries = self.machines.lock();
            // This PC's ports are the machines' together: one is one machine's forward only.
            let asker = connection.asker();
            for (other, entry) in entries.iter().filter(|(other, _)| **other != name) {
                let other = if asker.may_use(entry) { other.as_str() } else { "another machine" };
                if let Some(taken) = wanted.forwards.iter().find(|forward| {
                    entry.settings.forwards.iter().any(|kept| kept.host_port == forward.host_port)
                }) {
                    return Answer::Refused {
                        reason: format!(
                            "This PC's port {} is forwarded to {other} already.",
                            taken.host_port
                        ),
                    };
                }
            }
            let entry = entries.get_mut(&name).expect("found just now");
            if !entry.idle() {
                return Answer::Refused {
                    reason: format!("{name} is {}. Its settings change while it's off.", busy_words(entry)),
                };
            }
            let changed = Settings {
                memory_mb: wanted.memory_mb,
                processors: wanted.processors,
                card: wanted.card,
                nvapi_wrapper: wanted.nvapi_wrapper,
                tpm: wanted.tpm,
                network: wanted.network,
                shares: wanted.shares,
                // With no network, or on another one, the guest's last address leads nowhere.
                address: entry
                    .settings
                    .address
                    .clone()
                    .filter(|_| wanted.network && wanted.network_mode == mode_before),
                network_mode: wanted.network_mode,
                forwards: wanted.forwards,
                nested_virtualization: wanted.nested_virtualization,
                ..entry.settings.clone()
            };
            if changed == entry.settings {
                return Answer::Ok;
            }
            // Kept on disk first, so what the service says is what the next start of the service
            // reads.
            if let Err(error) = self.machines.store.save(&name, &changed) {
                return Answer::Refused { reason: format!("The change couldn't be kept: {error}") };
            }
            entry.settings = changed.clone();
            changed
        };
        let card = match &saved.card {
            Card::Default => "the default card".to_owned(),
            Card::Named(card) => card.clone(),
            Card::None => "no card".to_owned(),
        };
        let mut words = format!(
            "Changed: from its next start it has {} MB of memory, {} processors and {card}",
            saved.memory_mb, saved.processors
        );
        if !saved.nvapi_wrapper {
            words.push_str(", NVIDIA's own NVAPI in place of the wrapper DLSS needs");
        }
        if !saved.tpm {
            words.push_str(", no TPM");
        }
        if !saved.network {
            words.push_str(", no network");
        } else if let NetworkMode::Bridged { adapter } = &saved.network_mode {
            words.push_str(&format!(", a place on this PC's network through {adapter}"));
        }
        if !saved.forwards.is_empty() {
            let ports: Vec<String> = saved
                .forwards
                .iter()
                .map(|forward| format!("{} to {}", forward.host_port, forward.guest_port))
                .collect();
            words.push_str(&format!(", ports forwarded ({})", ports.join(", ")));
        }
        if saved.nested_virtualization {
            words.push_str(", nested virtualization");
        }
        if saved.shares.is_empty() {
            words.push_str(", and no shared folder.");
        } else {
            let shared: Vec<String> = saved
                .shares
                .iter()
                .map(|share| {
                    let how = if share.read_only { "read-only" } else { "to read and write" };
                    format!("{} shared {how}", share.folder.display())
                })
                .collect();
            words.push_str(&format!(", and {}.", shared.join(", ")));
        }
        self.machines.tell(&name, Event::Note(words));
        Answer::Ok
    }

    /// Lets a stopped machine's disk grow. The machines are held while it does, so nothing starts
    /// the machine meanwhile; growing a disk that grows as it's written only changes its header.
    fn grow(&self, asked: &str, size_gb: u64) -> Answer {
        let Some(name) = self.machines.find(asked) else { return no_machine(asked) };
        if let Some(refused) = self.unusable(&name) {
            return refused;
        }
        if !(SIZE_GB.0..=SIZE_GB.1).contains(&size_gb) {
            return Answer::Refused {
                reason: format!("A machine's disk is {} to {} GB.", SIZE_GB.0, SIZE_GB.1),
            };
        }
        let mut entries = self.machines.lock();
        let entry = entries.get_mut(&name).expect("found just now");
        if !entry.idle() {
            return Answer::Refused { reason: format!("{name} is {}. Stop it first.", busy_words(entry)) };
        }
        // Deleted snapshots count too: the disk is made on them until they've gone.
        if snapshots::made_on_another(&self.machines.store.disk(&name)).unwrap_or(true) {
            return Answer::Refused {
                reason: format!(
                    "{name}'s disk is made on its snapshots, so it can't grow. Delete them first."
                ),
            };
        }
        match entry.settings.size_gb {
            Some(now) if size_gb < now => {
                return Answer::Refused {
                    reason: format!(
                        "{name}'s disk is {now} GB, and a disk only grows: a smaller one would cut into the guest's partitions."
                    ),
                };
            }
            Some(now) if size_gb == now => return Answer::Ok,
            _ => {}
        }
        if let Err(reason) = self.work.grow(&self.machines.store.disk(&name), size_gb) {
            return Answer::Refused { reason };
        }
        entry.settings.size_gb = Some(size_gb);
        let saved = self.machines.store.save(&name, &entry.settings);
        drop(entries);
        if let Err(error) = saved {
            self.machines.tell(
                &name,
                Event::Problem(format!("The disk grew, but its size couldn't be kept: {error}")),
            );
        }
        self.machines.tell(
            &name,
            Event::Note(format!(
                "Its disk may grow to {size_gb} GB now. The guest's own partition takes the room at its next start."
            )),
        );
        Answer::Ok
    }

    fn take_snapshot(self: &Arc<Self>, asked: &str, label: String) -> Answer {
        let Some(name) = self.machines.find(asked) else { return no_machine(asked) };
        if let Some(refused) = self.unusable(&name) {
            return refused;
        }
        let label = match snapshots::checked_label(&label) {
            Ok(label) => label.to_owned(),
            Err(reason) => return Answer::Refused { reason },
        };
        let disk = self.machines.store.disk(&name);
        if !Record::load(&disk).unwrap_or_default().installed {
            return Answer::Refused {
                reason: format!(
                    "{name} isn't set up yet. A machine has snapshots once its first boot has set it up."
                ),
            };
        }
        if let Some(reason) =
            snapshots::count_refusal(&name, snapshots::list(&disk).map_or(0, |list| list.len()))
        {
            return Answer::Refused { reason };
        }
        let doing = format!("Taking the snapshot {label}");
        self.in_background(&name, doing, move |service, disk| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_secs());
            snapshots::take(disk, &label, now, service.work.disks())
                .map(|_| format!("Took the snapshot {label}."))
        })
    }

    fn revert(self: &Arc<Self>, asked: &str, id: u64) -> Answer {
        let Some(name) = self.machines.find(asked) else { return no_machine(asked) };
        if let Some(refused) = self.unusable(&name) {
            return refused;
        }
        let Some(label) = self.snapshot_label(&name, id) else { return no_snapshot(&name, id) };
        self.in_background(&name, format!("Going back to the snapshot {label}"), move |service, disk| {
            snapshots::revert(disk, id, service.work.disks())
                .map(|()| format!("Went back to the snapshot {label}. What the machine held since is gone."))
        })
    }

    fn delete_snapshot(self: &Arc<Self>, asked: &str, id: u64) -> Answer {
        let Some(name) = self.machines.find(asked) else { return no_machine(asked) };
        if let Some(refused) = self.unusable(&name) {
            return refused;
        }
        let Some(label) = self.snapshot_label(&name, id) else { return no_snapshot(&name, id) };
        self.in_background(&name, format!("Deleting the snapshot {label}"), move |service, disk| {
            snapshots::delete(disk, id, service.work.disks())
                .map(|()| format!("Deleted the snapshot {label}."))
        })
    }

    /// The label of one of a machine's snapshots. Only an ID the index has is ever used, as it ends
    /// up in a path the service writes.
    fn snapshot_label(&self, name: &str, id: u64) -> Option<String> {
        let list = snapshots::list(&self.machines.store.disk(name)).ok()?;
        list.into_iter().find(|snapshot| snapshot.id == id).map(|snapshot| snapshot.label)
    }

    /// Does `work` to a stopped machine on a thread of its own, as merging a disk can take minutes,
    /// with the machine marked busy so nothing else is done to it meanwhile. The answer is that it
    /// has begun; how it ended goes to the machine's log.
    fn in_background(
        self: &Arc<Self>,
        name: &str,
        doing: String,
        work: impl FnOnce(&Service<W>, &Path) -> Result<String, String> + Send + 'static,
    ) -> Answer {
        {
            let mut entries = self.machines.lock();
            let entry = entries.get_mut(name).expect("found by the caller");
            if !entry.idle() {
                return Answer::Refused {
                    reason: format!("{name} is {}. Stop it first.", busy_words(entry)),
                };
            }
            entry.busy = Some(doing.clone());
        }
        self.machines.tell(name, Event::Note(format!("{doing}.")));
        let service = Arc::clone(self);
        let name = name.to_owned();
        thread::spawn(move || {
            let disk = service.machines.store.disk(&name);
            let ended = work(&service, &disk);
            if let Some(entry) = service.machines.lock().get_mut(&name) {
                entry.busy = None;
            }
            match ended {
                Ok(words) => service.machines.tell(&name, Event::Note(words)),
                Err(error) => service.machines.tell(&name, Event::Problem(error)),
            }
        });
        Answer::Ok
    }

    /// Moves a stopped machine's disks to a folder of its name in `folder`, or back to the service's
    /// own folder if `folder` is empty. The user asking is checked as for a new machine's place, and
    /// there must be room. What's left is done on a thread of its own, as the copy takes a while, with
    /// the machine marked busy: see [`Service::move_disks`].
    fn move_machine(
        self: &Arc<Self>,
        asked: &str,
        folder: PathBuf,
        connection: &mut dyn Connection,
    ) -> Answer {
        let Some(name) = self.machines.find(asked) else { return no_machine(asked) };
        if let Some(refused) = self.unusable(&name) {
            return refused;
        }
        {
            let entries = self.machines.lock();
            let entry = &entries[&name];
            if !entry.idle() {
                return Answer::Refused {
                    reason: format!(
                        "{name} is {}. Stop it first: its disks are moved while it's off.",
                        busy_words(entry)
                    ),
                };
            }
        }
        let store = &self.machines.store;
        let source = store.place(&name);
        let own = folder.as_os_str().is_empty();
        let target = if own { store.folder(&name) } else { folder.join(&name) };
        let refused = |reason: String| Answer::Refused { reason };
        if same_place(&target, &source) {
            return refused(format!("{name}'s disks are in {} already.", source.display()));
        }
        if inside(&target, &source) {
            return refused(format!(
                "{} is inside the folder the disks are in now, so they can't be moved there.",
                target.display()
            ));
        }
        let (target, held) = if own {
            match tree(&target) {
                Ok(left) if left.is_empty() => (target, Box::new(()) as Held),
                Ok(_) => {
                    return refused(format!(
                        "{} has files in it that aren't the machine's own, so the disks aren't moved there.",
                        target.display()
                    ));
                }
                Err(reason) => return refused(reason),
            }
        } else {
            match self.placement(&name, &folder, connection) {
                Ok(placed) => placed,
                Err(reason) => return refused(reason),
            }
        };
        let needed = match tree(&source) {
            Ok(files) => files.values().sum::<u64>(),
            Err(reason) => return refused(reason),
        };
        let on = if own { target.clone() } else { target.parent().map_or(folder, Path::to_owned) };
        match self.work.free_space(&on) {
            Ok(free) if free >= needed.saturating_add(SPARE) => {}
            Ok(free) => {
                return refused(format!(
                    "{} has {} free, and the disks take {}, so there isn't room.",
                    on.display(),
                    size_words(free),
                    size_words(needed)
                ));
            }
            Err(reason) => return refused(reason),
        }
        let doing = format!("Moving its disks to {}", target.display());
        self.in_background(&name.clone(), doing, move |service, _| {
            service.move_disks(&name, &source, &target, own, held)
        })
    }

    /// Copies a machine's disks, and everything beside them, to `target`, then checks the copy, then
    /// makes it the machine's place, and only then deletes the old. The machine is where it was until
    /// the copy has been checked and its disks have been told where they are, and a copy that fails
    /// any step is deleted. A `target` that isn't the machine's own folder is made here, as the
    /// system's and administrators'. `parent` holds the folder it's made in until it's made.
    fn move_disks(
        &self,
        name: &str,
        source: &Path,
        target: &Path,
        own: bool,
        parent: Held,
    ) -> Result<String, String> {
        let store = &self.machines.store;
        let sink = self.machines.sink(name);
        let old_disk = store.disk(name);
        if !own {
            self.work.private_folder(target)?;
        }
        let held = if own { Ok(Box::new(()) as Held) } else { store.hold(target) };
        drop(parent);
        // The copy is emptied while it's held and then let go, as a machine is deleted. One that isn't
        // held, as it couldn't be, may not be the folder that was made, so only an empty one goes.
        let discard = |held: Option<Held>, why: String| {
            let remove = || fs::remove_dir(target).map_err(|error| format!("{}: {error}", target.display()));
            let left = match (own, held) {
                (true, held) => {
                    drop(held);
                    remove_disks(target)
                }
                (false, Some(held)) => {
                    let emptied = empty(target);
                    drop(held);
                    emptied.and_then(|()| remove())
                }
                (false, None) => remove(),
            };
            match left {
                Ok(()) => format!("{why} {name} is where it was."),
                Err(error) => format!(
                    "{why} {name} is where it was, but the partial copy in {} couldn't be deleted: {error}",
                    target.display()
                ),
            }
        };
        let held = held.map_err(|why| discard(None, why))?;
        if let Err(error) = self.copy_disks(name, source, target, &old_disk, &*sink) {
            return Err(discard(Some(held), error));
        }
        // The record is written, and the place changed, in one step.
        let mut held = Some(held);
        let (was, old_held) = match store.switch(name, target, &mut held) {
            Ok(before) => before,
            Err(error) => return Err(discard(held, format!("The new place couldn't be kept: {error}"))),
        };
        let deleted = if own_folder(store, name, &was) {
            remove_disks(&was)
        } else {
            let emptied = empty(&was);
            drop(old_held);
            emptied.and_then(|()| fs::remove_dir(&was).map_err(|error| format!("{}: {error}", was.display())))
        };
        if let Err(error) = deleted {
            sink.problem(format!(
                "The disks are moved, but the old copy in {} couldn't be deleted, and is left: {error}",
                was.display()
            ));
        }
        Ok(format!("Moved its disks to {}.", target.display()))
    }

    /// Copies what's in `source` to `target`, checks every file is there at its size, and has the
    /// copy's disks read from each other where it is now.
    fn copy_disks(
        &self,
        name: &str,
        source: &Path,
        target: &Path,
        old_disk: &Path,
        sink: &dyn Sink,
    ) -> Result<(), String> {
        let before = tree(source)?;
        let total: u64 = before.values().sum();
        sink.note(format!("Copying {} files, {}, to {}.", before.len(), size_words(total), target.display()));
        let mut done = 0;
        copy_tree(source, target, true, &mut |file: &str, bytes: u64| {
            done += bytes;
            if bytes >= LARGE_FILE {
                sink.note(format!("Copied {file}: {} of {}.", size_words(done), size_words(total)));
            }
        })?;
        let after = tree(target)?;
        if let Some((file, size)) = before.iter().find(|(file, size)| after.get(*file) != Some(size)) {
            return Err(match after.get(file) {
                Some(copied) => format!("The copy of {file} is {copied} bytes, not the {size} it is."),
                None => format!("The copy of {file} is missing."),
            });
        }
        if after.len() != before.len() {
            return Err("The copy has files the original doesn't.".into());
        }
        sink.note("Checked the copy: every file is there, at the size it is.");
        snapshots::relocate(old_disk, &target.join(format!("{}.vhdx", name)), self.work.disks())
    }

    fn account(&self, asked: &str) -> Answer {
        let Some(name) = self.machines.find(asked) else { return no_machine(asked) };
        match self.machines.store.account(&name) {
            Ok((user, password)) => Answer::Account { user, password },
            Err(reason) => Answer::Refused { reason },
        }
    }

    /// Hands over the files a bug report needs from this PC: what the work reads, a list of the
    /// machines, and if one is named, a few of its own files. Nothing secret goes: see
    /// [`report_machine_files`].
    fn report_files(&self, machine: Option<String>, connection: &mut dyn Connection) {
        let named = match machine.as_deref() {
            Some(asked) => match self.machines.find(asked) {
                Some(name) => Some(name),
                None => {
                    let _ = connection.send(&no_machine(asked));
                    return;
                }
            },
            None => None,
        };
        let mut files = self.work.report_parts();
        files.push(("machines.txt".to_owned(), Ok(self.machines_text(&connection.asker()))));
        if let Some(name) = &named {
            for (shown, path) in report_machine_files(&self.machines.store, name) {
                files.push((shown, tail_file(&path, REPORT_TAIL)));
            }
        }
        for (name, text) in files {
            let sent = match text {
                Ok(text) => send_text(connection, &name, &text),
                Err(reason) => connection.send(&Answer::ReportMissing { name, reason }),
            };
            if sent.is_err() {
                return;
            }
        }
        let _ = connection.send(&Answer::Done { reason: None });
    }

    /// Each machine on a line: its name, system, state, and whether its first boot has set it up.
    fn machines_text(&self, asker: &Asker) -> String {
        let Answer::Status { machines, .. } = self.status(asker) else { return String::new() };
        machines
            .iter()
            .map(|machine| {
                let system = match machine.system {
                    System::Windows => "windows",
                    System::Linux => "linux",
                };
                let set_up = if machine.set_up { "set up" } else { "not set up" };
                format!("{}: {system}, {}, {set_up}\n", machine.name, words(machine.state))
            })
            .collect()
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

/// A report's files for a machine are these and nothing else: its folder also holds the guest's
/// account and password, the viewer's private key, and its disks. Each is optional. A file is
/// named in the report as it's shown here.
fn report_machine_files(store: &Store, name: &str) -> Vec<(String, PathBuf)> {
    let folder = store.folder(name);
    let place = store.place(name);
    let record = Record::path(&store.disk(name));
    let record_name = record.file_name().map(|file| file.to_string_lossy().into_owned()).unwrap_or_default();
    vec![
        ("machine.json".to_owned(), folder.join("machine.json")),
        (record_name, record),
        ("snapshots/index.json".to_owned(), place.join("snapshots").join("index.json")),
        ("events.log".to_owned(), store.events(name)),
        ("events.1.log".to_owned(), store.events_before(name)),
    ]
}

/// The port on this PC's loopback an Ubuntu guest's SSH is reached on, and what to say if it's
/// new. A machine keeps the one it has while it's free to listen on, and is given another when it
/// isn't, which no other machine keeps either: the lowest from `ssh::PORTS` up. It's kept in the
/// machine's settings. A Windows guest has none.
fn keep_port<W: Work>(machines: &Machines, work: &W, name: &str) -> (Option<u16>, Option<String>) {
    let linux = Record::load(&machines.store.disk(name)).unwrap_or_default().guest == Guest::Linux;
    if !linux {
        return (None, None);
    }
    let mut entries = machines.lock();
    let Some(before) = entries.get(name).map(|entry| entry.settings.ssh_port) else { return (None, None) };
    let others: Vec<u16> = entries
        .iter()
        .filter(|(other, _)| other.as_str() != name)
        .filter_map(|(_, entry)| entry.settings.ssh_port)
        .collect();
    let usable = |port: u16| !others.contains(&port) && work.port_free(port);
    let Some(port) = before.filter(|port| usable(*port)).or_else(|| ssh::ports().find(|port| usable(*port)))
    else {
        return (
            None,
            Some(
                "There is no free port on this PC's loopback for its SSH, so there's none this boot.".into(),
            ),
        );
    };
    if before == Some(port) {
        return (before, None);
    }
    let entry = entries.get_mut(name).expect("found just now");
    entry.settings.ssh_port = Some(port);
    let kept = machines.store.save(name, &entry.settings);
    let mut said = match before {
        Some(busy) => format!("Port {busy} on this PC's loopback was busy, so its SSH is on {port} now."),
        None => format!("Its SSH is on port {port} of this PC's loopback."),
    };
    if let Err(error) = kept {
        said.push_str(&format!(" The port couldn't be kept: {error}"));
    }
    (Some(port), Some(said))
}

fn gpu_choice(card: &Card) -> GpuChoice {
    match card {
        Card::Default => GpuChoice::Default,
        Card::Named(name) => GpuChoice::Matching(name.clone()),
        Card::None => GpuChoice::None,
    }
}

/// Why `share` can't be shared, if it can't: a folder on this PC's own drives, which the user asking
/// may read, write unless it's read-only, and give others into.
fn share_refusal(share: &Share, connection: &mut dyn Connection) -> Result<Held, String> {
    let folder = &share.folder;
    if !folder.is_absolute() || folder.to_string_lossy().starts_with(r"\\") {
        return Err(format!("{} isn't a folder on this PC's own drives.", folder.display()));
    }
    // Asked as the user first, as for an image.
    let held = connection.may_share(folder, share.read_only)?;
    if !folder.is_dir() {
        return Err(format!("{} isn't a folder.", folder.display()));
    }
    Ok(held)
}

/// The name of the card `card` means on this PC now, if any.
fn resolved<'a>(card: &'a Card, cards: &'a [CardInfo]) -> Option<&'a str> {
    match card {
        Card::Default => cards.iter().find(|known| known.default).map(|known| known.name.as_str()),
        Card::Named(name) => Some(name),
        Card::None => None,
    }
}

fn no_snapshot(name: &str, id: u64) -> Answer {
    Answer::Refused { reason: format!("{name} has no snapshot {id}.") }
}

/// Where a machine stands, in words, with what's being done to it if it's busy.
fn busy_words(entry: &Entry) -> String {
    match &entry.busy {
        Some(doing) => doing.to_lowercase(),
        None => words(entry.state),
    }
}

/// A file's text as `ReportPart` lines. An empty file is one empty part, so it's still there.
fn send_text(connection: &mut dyn Connection, name: &str, text: &str) -> io::Result<()> {
    let found = pieces(text);
    let found = if found.is_empty() { vec![""] } else { found };
    for piece in found {
        connection.send(&Answer::ReportPart { name: name.to_owned(), text: piece.to_owned() })?;
    }
    Ok(())
}

fn no_machine(name: &str) -> Answer {
    Answer::Refused { reason: format!("There is no machine called {name}.") }
}

/// A machine's state in the service's words: its common word, in lower case, as it's said mid-way.
fn words(state: State) -> String {
    state.name().to_lowercase()
}

#[cfg(test)]
#[path = "requests/state_words_tests.rs"]
mod state_words_tests;

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

/// Whether `folder` is a machine's own folder, where its settings are.
fn own_folder(store: &Store, name: &str, folder: &Path) -> bool {
    same_place(folder, &store.folder(name))
}

/// A path as it's compared: the same whatever its case or its slashes, and without one at its end.
fn plain(path: &Path) -> String {
    path.to_string_lossy().to_lowercase().replace('/', "\\").trim_end_matches('\\').to_owned()
}

fn same_place(a: &Path, b: &Path) -> bool {
    plain(a) == plain(b)
}

/// Whether `path` is `folder` or in it.
fn inside(path: &Path, folder: &Path) -> bool {
    let (path, folder) = (plain(path), plain(folder));
    path == folder || path.starts_with(&format!("{folder}\\"))
}

/// A size in words, in GB when it's large and MB when it isn't.
use guesthouse_basics::words::size_in_gb_or_mb_down as size_words;

#[cfg(test)]
#[path = "requests/size_words_tests.rs"]
mod size_words_tests;

mod disk_tree;
mod forwards;
mod guest_address;
mod hyper_v;
mod owners;
mod pairing;
mod remote_view;

#[cfg(test)]
mod tests;
