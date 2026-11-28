// SPDX-License-Identifier: GPL-2.0-or-later
//! The agent's machines: starts QEMU for a machine, watches it, asks it to stop, and finds it
//! again if the agent itself restarts. What it says goes out as lines of `protocol`; how those
//! lines travel (a Unix socket, XPC) is up to `server` and the Swift shell around it.
//!
//! Each machine has a slot with its status. Everything that happens to it, whether the user
//! asked or a watcher noticed, becomes a `state::Event` applied under one lock, so the status
//! and the notices sent for it never disagree. Watchers carry the run they belong to and do
//! nothing once a newer run has started.

use crate::control;
use crate::launch::{self, Graphics, Launch};
use crate::process::{self, Pid};
use crate::protocol::{self, MachineState, Op, Response, StateChange};
use crate::state::{self, Event, Refused, Status};
use crate::store;
use guesthouse_remote_view_core::pairing::Event as PairEvent;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

mod guard;
mod install;
use install::{Sizes, Tools};
mod pairing;
mod share;

/// The pid file in a machine's run folder.
const PID_FILE: &str = "machine.pid";
/// QEMU's own output, and so the reason it did not start.
const LOG_FILE: &str = "qemu.log";
const POLL: Duration = Duration::from_millis(200);
/// How often a QEMU the agent did not start itself is looked at.
const ADOPTED_POLL: Duration = Duration::from_millis(500);
/// How long after `quit` QEMU has to go before it is sent SIGTERM, and then how long after.
const QUIT_WAIT: Duration = Duration::from_secs(3);
const TERM_WAIT: Duration = Duration::from_secs(5);
/// A reason is one line; longer ones are cut.
const REASON_LIMIT: usize = 200;

/// What installing a guest needs that a request does not name.
#[derive(Debug, Clone, Default)]
pub struct InstallConfig {
    /// Install with a stand-in network and guest, to try the screens and the command line
    /// (`GUESTHOUSE_INSTALL_FAKE`).
    pub fake: bool,
    /// The folder the driver's build gathers its files in (`GUESTHOUSE_DRIVER_DIR`).
    pub driver_dir: Option<PathBuf>,
    /// A folder holding QEMU's firmware (`edk2-aarch64-code.fd` and the vars file) and the option
    /// ROM, tried after the folders beside QEMU (`GUESTHOUSE_FIRMWARE_DIR`).
    pub firmware_dir: Option<PathBuf>,
}

/// What the agent is told when it starts, from the environment.
#[derive(Debug, Clone)]
pub struct Config {
    pub machines: PathBuf,
    /// The QEMU binary when a request names none (`GUESTHOUSE_QEMU`).
    pub qemu: Option<PathBuf>,
    /// The rmhost program when no request has named one (`GUESTHOUSE_RMHOST`).
    pub rmhost: Option<PathBuf>,
    /// The graphics process when a request names none (`GUESTHOUSE_PVG_HOST`).
    pub pvg_host: Option<PathBuf>,
    /// What installing a guest needs beyond the above.
    pub install: InstallConfig,
    /// How long a guest has to power off before Force Off is offered.
    pub shutdown_timeout: Duration,
    /// How long QEMU has to open its control socket.
    pub start_timeout: Duration,
}

impl Config {
    /// The machines folder is the app's; the timeouts can be shortened for tests with
    /// `GUESTHOUSE_SHUTDOWN_TIMEOUT_MS` and `GUESTHOUSE_START_TIMEOUT_MS`.
    pub fn from_env() -> Option<Config> {
        let millis = |name: &str, default: u64| {
            let value = std::env::var(name).ok().and_then(|text| text.parse().ok());
            Duration::from_millis(value.unwrap_or(default))
        };
        Some(Config {
            machines: store::default_folder()?,
            qemu: std::env::var_os("GUESTHOUSE_QEMU").filter(|path| !path.is_empty()).map(PathBuf::from),
            rmhost: std::env::var_os("GUESTHOUSE_RMHOST").filter(|path| !path.is_empty()).map(PathBuf::from),
            install: InstallConfig {
                fake: std::env::var_os("GUESTHOUSE_INSTALL_FAKE").is_some_and(|v| !v.is_empty()),
                driver_dir: std::env::var_os("GUESTHOUSE_DRIVER_DIR")
                    .filter(|p| !p.is_empty())
                    .map(PathBuf::from),
                firmware_dir: std::env::var_os("GUESTHOUSE_FIRMWARE_DIR")
                    .filter(|p| !p.is_empty())
                    .map(PathBuf::from),
            },
            pvg_host: std::env::var_os("GUESTHOUSE_PVG_HOST")
                .filter(|path| !path.is_empty())
                .map(PathBuf::from),
            shutdown_timeout: millis("GUESTHOUSE_SHUTDOWN_TIMEOUT_MS", 60_000),
            start_timeout: millis("GUESTHOUSE_START_TIMEOUT_MS", 30_000),
        })
    }
}

#[derive(Default)]
struct Slot {
    status: Option<Status>,
    /// Counts starts: a watcher of an older run does nothing.
    run: u64,
    /// Counts what the user asked since: a shutdown timer does nothing once a force off began.
    ticket: u64,
    pid: Option<Pid>,
    /// The graphics process of a guarded machine.
    host: Option<Pid>,
    /// The machine's screen sharing, when there is any.
    share: share::ShareSlot,
    /// The machine's install, when it has one.
    install: install::InstallSlot,
}

impl Slot {
    fn status(&self) -> Status {
        self.status.clone().unwrap_or(Status::Stopped)
    }
}

struct Shared {
    config: Config,
    slots: Mutex<BTreeMap<String, Slot>>,
    /// Each machine's pairing, the shared host-timed one (`pairing.rs`).
    pairing: guesthouse_remote_view_core::session::Sessions,
    subscribers: Mutex<Vec<(u64, Sender<String>)>>,
    next_subscriber: Mutex<u64>,
    /// Which build this agent is, told by the program that made it (`set_build`).
    build: std::sync::OnceLock<crate::agent_build::Identity>,
}

/// The agent. Cloning gives another handle on the same machines.
#[derive(Clone)]
pub struct Agent {
    shared: Arc<Shared>,
}

/// Who is told of state changes, until it is dropped.
pub struct Subscription {
    shared: Arc<Shared>,
    id: u64,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        lock(&self.shared.subscribers).retain(|(id, _)| *id != self.id);
    }
}

impl Agent {
    /// An agent for `config`, which first finds the machines still running from before.
    pub fn new(config: Config) -> Agent {
        let agent = Agent {
            shared: Arc::new(Shared {
                config,
                slots: Mutex::new(BTreeMap::new()),
                pairing: guesthouse_remote_view_core::session::Sessions::default(),
                subscribers: Mutex::new(Vec::new()),
                next_subscriber: Mutex::new(0),
                build: std::sync::OnceLock::new(),
            }),
        };
        agent.adopt_running();
        agent.resume_installs();
        agent
    }

    /// Tells the agent which build it is, from the bundle's version and build number: the
    /// identity (`agent_build`) of this program as it is now. Said once, when it starts.
    pub fn set_build(&self, version: &str, build: &str) {
        let identity = std::env::current_exe()
            .ok()
            .and_then(|program| crate::agent_build::identity_of(&program, version, build));
        if let Some(identity) = identity {
            let _ = self.shared.build.set(identity);
        }
    }

    /// Answers one request line with one answer line. Never waits on QEMU.
    pub fn handle(&self, line: &str) -> String {
        let request = match protocol::parse_request(line) {
            Ok(request) => request,
            Err((id, why)) => return Response::error(id, why).to_line(),
        };
        let id = request.id;
        let result = match request.op {
            Op::List => {
                let mut response = Response::ok(id);
                response.machines = Some(self.list());
                response.shares = Some(self.shares());
                response.installs = Some(self.installs());
                response.agent = self.shared.build.get().cloned();
                return response.to_line();
            }
            Op::Start { machine, qemu, rmhost, pvg_host, scale } => {
                self.start(&machine, qemu.as_deref(), rmhost.as_deref(), pvg_host.as_deref(), scale)
            }
            Op::Share { machine, on, pairing, pairing_seconds, rmhost } => self.share(
                &machine,
                on,
                pairing.then(|| pairing_seconds.unwrap_or(crate::remote::PAIRING_SECONDS)),
                rmhost.as_deref(),
            ),
            Op::Shutdown { machine } => self.shut_down(&machine),
            Op::ForceOff { machine } => self.force_off(&machine),
            Op::Install {
                name,
                guest,
                file,
                qemu,
                pvg_host,
                driver_dir,
                cores,
                memory_mb,
                disk_gb,
                desktop,
                password,
            } => match self.install_start(
                &name,
                &guest,
                file,
                Tools {
                    qemu,
                    pvg_host,
                    driver_dir: driver_dir.map(PathBuf::from),
                    password,
                    desktop,
                    sizes: Sizes { cores, memory_mb, disk_gb },
                },
            ) {
                Ok(machine) => {
                    let mut response = Response::ok(id);
                    response.machine = Some(machine);
                    return response.to_line();
                }
                Err(why) => Err(why),
            },
            Op::InstallChoose { machine, choice, path, password } => {
                self.install_choose(&machine, &choice, path, password)
            }
            Op::InstallStatus { machine } => {
                let mut response = Response::ok(id);
                response.installs =
                    Some(self.installs().into_iter().filter(|i| i.machine == machine).collect());
                return response.to_line();
            }
            Op::InstallCancel { machine } => self.install_cancel(&machine),
            Op::PairClose { machine } => self.pair_event(&machine, PairEvent::Stop),
            Op::PairAccept { machine } => self.pair_event(&machine, PairEvent::Accept),
            Op::PairDecline { machine } => self.pair_event(&machine, PairEvent::Decline),
            Op::DeviceAllow { machine, fingerprint, what, on } => {
                self.device_command(&machine, |policy| policy.device_allow(&fingerprint, &what, on))
            }
            Op::DeviceRevoke { machine, fingerprint } => {
                self.device_command(&machine, |policy| policy.device_revoke(&fingerprint))
            }
            Op::DeviceDrop { machine } => {
                self.device_command(&machine, |_| Ok(guesthouse_remote_view_core::ctl::device_drop()))
            }
        };
        match result {
            Ok(()) => Response::ok(id),
            Err(why) => Response::error(id, why),
        }
        .to_line()
    }

    /// Every machine in the machines folder, and any running one whose folder is gone.
    pub fn list(&self) -> Vec<MachineState> {
        let mut ids: Vec<String> = store::list(&self.shared.config.machines)
            .map(|listing| listing.machines.into_iter().map(|machine| machine.id).collect())
            .unwrap_or_default();
        let slots = lock(&self.shared.slots);
        for (id, slot) in slots.iter() {
            if slot.status().is_alive() && !ids.contains(id) {
                ids.push(id.clone());
            }
        }
        ids.sort();
        ids.iter()
            .map(|id| {
                let status = slots.get(id).map_or(Status::Stopped, Slot::status);
                MachineState::new(id, &status)
            })
            .collect()
    }

    /// Sends every state change, as a line, to `sender` until the subscription is dropped.
    pub fn subscribe(&self, sender: Sender<String>) -> Subscription {
        let mut next = lock(&self.shared.next_subscriber);
        *next += 1;
        let id = *next;
        lock(&self.shared.subscribers).push((id, sender));
        Subscription { shared: self.shared.clone(), id }
    }

    /// Nothing is running and nobody is listening: the agent may go.
    pub fn is_idle(&self) -> bool {
        lock(&self.shared.subscribers).is_empty()
            && !lock(&self.shared.slots).values().any(|slot| slot.status().is_alive())
    }

    fn start(
        &self,
        id: &str,
        qemu: Option<&str>,
        rmhost: Option<&str>,
        pvg_host: Option<&str>,
        scale: Option<u32>,
    ) -> Result<(), String> {
        let machine = store::find(&self.shared.config.machines, id)?;
        let mut launch = machine
            .launch
            .ok_or("this machine has no firmware files in machine.json, so it cannot be started")?;
        if let Some(scale) = scale {
            if !launch::SCALES.contains(&scale) {
                return Err(format!("{scale} is not a scale a guest takes (1 or 2)."));
            }
            launch.scale = scale;
        }
        if launch.kind == launch::LINUX {
            launch.keyboard = crate::keyboard_layout::mac_layout().map(|xkb| xkb.text());
        }
        let run = self.begin(id, Event::Start).map_err(|refused| refused.0)?;
        self.remember_rmhost(id, rmhost);
        if let Err(reason) = self.spawn(id, run, &launch, qemu, pvg_host) {
            let event = Event::StartRefused { reason: reason.clone() };
            self.apply(id, Some(run), event);
            return Err(reason);
        }
        Ok(())
    }

    fn shut_down(&self, id: &str) -> Result<(), String> {
        let ticket = self.ask(id, Event::ShutDown)?;
        let agent = self.clone();
        let id = id.to_string();
        thread::spawn(move || agent.power_off(&id, ticket));
        Ok(())
    }

    fn force_off(&self, id: &str) -> Result<(), String> {
        let ticket = self.ask(id, Event::ForceOff)?;
        let agent = self.clone();
        let id = id.to_string();
        thread::spawn(move || agent.quit(&id, ticket));
        Ok(())
    }

    /// Asks the guest to power off, then waits for it; marks the shutdown stalled if it does
    /// not go in time.
    fn power_off(&self, id: &str, ticket: u64) {
        let socket = self.control_socket(id);
        let mut result = Err(std::io::Error::other("not tried"));
        for _ in 0..3 {
            result = control::talk(&socket, Some("system_powerdown"));
            if result.is_ok() {
                break;
            }
            thread::sleep(Duration::from_millis(500));
        }
        if let Err(error) = result {
            eprintln!("guesthouse-agent: {id}: system_powerdown: {error}");
            self.stall(id, ticket);
            return;
        }
        if !self.wait_while(id, ticket, self.shared.config.shutdown_timeout) {
            self.stall(id, ticket);
        }
    }

    /// Force off: QMP `quit`, then SIGTERM if the process is still there.
    fn quit(&self, id: &str, ticket: u64) {
        if let Err(error) = control::talk(&self.control_socket(id), Some("quit")) {
            eprintln!("guesthouse-agent: {id}: quit: {error}");
        }
        if self.wait_while(id, ticket, QUIT_WAIT) {
            return;
        }
        let pid = lock(&self.shared.slots).get(id).and_then(|slot| slot.pid.clone());
        if let Some(pid) = pid {
            eprintln!("guesthouse-agent: {id}: still there after quit, sending SIGTERM to {}", pid.pid);
            process::terminate(&pid);
        }
        if !self.wait_while(id, ticket, TERM_WAIT) {
            self.stall(id, ticket);
        }
    }

    /// Waits up to `limit` while the machine stays in the shutdown the user asked for under
    /// `ticket`. True when it left that, because it stopped or because the user asked again.
    fn wait_while(&self, id: &str, ticket: u64, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        loop {
            {
                let slots = lock(&self.shared.slots);
                let slot = slots.get(id);
                let waiting = slot.is_some_and(|slot| {
                    slot.ticket == ticket && matches!(slot.status(), Status::ShuttingDown { .. })
                });
                if !waiting {
                    return true;
                }
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(POLL);
        }
    }

    /// The shutdown did not work in time. Does nothing if the user has asked something since.
    fn stall(&self, id: &str, ticket: u64) {
        let current = lock(&self.shared.slots).get(id).map(|slot| slot.ticket);
        if current == Some(ticket) {
            self.apply(id, None, Event::ShutdownStalled);
        }
    }

    fn control_socket(&self, id: &str) -> PathBuf {
        store::run_folder(&self.shared.config.machines, id).join("qmp.sock")
    }

    /// Checks what starting needs, and starts it. QEMU's process is started here for a machine
    /// on QEMU's own device; for a guarded one the graphics process is started first, and a
    /// thread waits for its socket and then starts QEMU, so the request does not wait for it.
    fn spawn(
        &self,
        id: &str,
        run: u64,
        launch: &Launch,
        qemu: Option<&str>,
        pvg_host: Option<&str>,
    ) -> Result<(), String> {
        if let Some(problem) = launch.problem() {
            return Err(problem);
        }
        let qemu = match qemu.filter(|path| !path.is_empty()) {
            Some(path) => PathBuf::from(path),
            None => {
                self.shared.config.qemu.clone().ok_or("No QEMU is set. Choose it in Settings, General.")?
            }
        };
        if !qemu.is_file() {
            return Err(format!("QEMU is not at {}", qemu.display()));
        }
        let host = match launch.graphics {
            Graphics::Qemu => None,
            Graphics::Guarded => Some(self.check_guarded(&qemu, pvg_host)?),
        };
        let run_dir = &launch.run_dir;
        store::make_run_folder_private(run_dir)
            .map_err(|error| format!("cannot make {}: {error}", run_dir.display()))?;
        for stale in
            [&launch.control_socket, &launch.input_socket, &launch.remote_socket, &run_dir.join(PID_FILE)]
        {
            let _ = fs::remove_file(stale);
        }
        let Some(host) = host else {
            return self.spawn_qemu(id, run, launch, &qemu);
        };
        let agent = self.clone();
        let (id, launch) = (id.to_string(), launch.clone());
        thread::spawn(move || {
            let started = agent.start_host(&id, run, &launch, &host).and_then(|()| {
                agent.spawn_qemu(&id, run, &launch, &qemu).inspect_err(|_| agent.stop_host(&id))
            });
            if let Err(reason) = started {
                agent.apply(&id, Some(run), Event::StartRefused { reason });
            }
        });
        Ok(())
    }

    /// Starts the QEMU process and the threads that watch it.
    fn spawn_qemu(&self, id: &str, run: u64, launch: &Launch, qemu: &Path) -> Result<(), String> {
        let run_dir = &launch.run_dir;
        let log = File::create(run_dir.join(LOG_FILE))
            .map_err(|error| format!("cannot write {}: {error}", run_dir.join(LOG_FILE).display()))?;
        let mut command = Command::new(qemu);
        command
            .args(launch::qemu_args(launch))
            .current_dir(run_dir)
            .stdin(Stdio::null())
            .stdout(log.try_clone().map_err(|error| error.to_string())?)
            .stderr(log)
            // Its own process group, so a signal meant for the agent does not reach it.
            .process_group(0);
        let child = command.spawn().map_err(|error| format!("cannot run {}: {error}", qemu.display()))?;
        let pid = process::identify(child.id() as i32).unwrap_or(Pid {
            pid: child.id() as i32,
            // It was gone before it could be looked at; the watcher is about to say so.
            started: "0.000000".into(),
        });
        let _ = process::write_file(&run_dir.join(PID_FILE), &pid);
        if let Some(slot) = lock(&self.shared.slots).get_mut(id) {
            slot.pid = Some(pid);
        }
        let agent = self.clone();
        let (watch_id, watch_dir) = (id.to_string(), run_dir.clone());
        thread::spawn(move || agent.watch_child(&watch_id, run, child, &watch_dir));
        let agent = self.clone();
        let (ready_id, socket) = (id.to_string(), launch.control_socket.clone());
        thread::spawn(move || agent.wait_until_ready(&ready_id, run, &socket));
        Ok(())
    }

    /// Waits for QEMU, our own child, and tells how it ended.
    fn watch_child(&self, id: &str, run: u64, mut child: std::process::Child, run_dir: &Path) {
        let status = child.wait();
        let _ = fs::remove_file(run_dir.join(PID_FILE));
        let event = match status {
            Ok(status) if status.success() => Event::Exited { clean: true, reason: String::new() },
            Ok(status) => Event::Exited { clean: false, reason: failure_reason(run_dir, status) },
            Err(error) => Event::Exited { clean: false, reason: format!("cannot wait for QEMU: {error}") },
        };
        self.apply(id, Some(run), event);
        self.share_stop(id);
        self.stop_host(id);
    }

    /// Calls the machine running once QEMU answers on its control socket.
    fn wait_until_ready(&self, id: &str, run: u64, socket: &Path) {
        let deadline = Instant::now() + self.shared.config.start_timeout;
        while Instant::now() < deadline {
            if !self.is_starting(id, run) {
                return;
            }
            if control::talk(socket, None).is_ok() {
                self.apply(id, Some(run), Event::Ready);
                self.share_when_ready(id);
                return;
            }
            thread::sleep(POLL);
        }
        if matches!(self.apply(id, Some(run), Event::StartTimedOut), Status::Failed { .. }) {
            let pid = lock(&self.shared.slots).get(id).and_then(|slot| slot.pid.clone());
            if let Some(pid) = pid {
                process::terminate(&pid);
            }
        }
    }

    fn is_starting(&self, id: &str, run: u64) -> bool {
        lock(&self.shared.slots)
            .get(id)
            .is_some_and(|slot| slot.run == run && slot.status() == Status::Starting)
    }

    /// Finds machines whose QEMU outlived the last agent: pid file and start time agree.
    fn adopt_running(&self) {
        let Some(run_root) = self.shared.config.machines.parent().map(|dir| dir.join("Run")) else {
            return;
        };
        let Ok(entries) = fs::read_dir(&run_root) else {
            return;
        };
        for entry in entries.flatten() {
            let file = entry.path().join(PID_FILE);
            let Some(pid) = process::read_file(&file) else {
                continue;
            };
            let id = entry.file_name().to_string_lossy().into_owned();
            if !process::is_running(&pid) {
                let _ = fs::remove_file(&file);
                // A graphics process left without its QEMU is of no use to anyone.
                self.end_leftover_host(&id_of(&entry.path()), &entry.path());
                continue;
            }
            eprintln!("guesthouse-agent: {id}: found running again, pid {}", pid.pid);
            let run = {
                let mut slots = lock(&self.shared.slots);
                let slot = slots.entry(id.clone()).or_default();
                slot.run += 1;
                slot.status = Some(Status::Running);
                slot.pid = Some(pid.clone());
                slot.run
            };
            self.adopt_host(&id, run, &entry.path());
            self.adopt_share(&id, &entry.path());
            // A machine found running with sharing allowed and no rmhost is shared now.
            if self.share_state(&id).is_none() && self.shared.config.rmhost.is_some() {
                let agent = self.clone();
                let id = id.clone();
                thread::spawn(move || agent.share_when_ready(&id));
            }
            let agent = self.clone();
            thread::spawn(move || agent.watch_found(&id, run, &pid, &file));
        }
    }

    /// Watches a QEMU that is not our child, by looking for it.
    fn watch_found(&self, id: &str, run: u64, pid: &Pid, pid_file: &Path) {
        while process::is_running(pid) {
            thread::sleep(ADOPTED_POLL);
        }
        let _ = fs::remove_file(pid_file);
        // The exit status of a process that is not our child is not ours to know.
        self.apply(id, Some(run), Event::Exited { clean: true, reason: String::new() });
        self.share_stop(id);
        self.stop_host(id);
    }

    /// A start: applies it and opens a new run.
    fn begin(&self, id: &str, event: Event) -> Result<u64, Refused> {
        let mut slots = lock(&self.shared.slots);
        let slot = slots.entry(id.to_string()).or_default();
        let new = state::next(&slot.status(), &event)?;
        slot.run += 1;
        slot.ticket += 1;
        slot.pid = None;
        self.set(id, slot, new);
        Ok(slot.run)
    }

    /// A shutdown or force off the user asked for: applies it and returns its ticket.
    fn ask(&self, id: &str, event: Event) -> Result<u64, String> {
        let mut slots = lock(&self.shared.slots);
        let slot = slots.entry(id.to_string()).or_default();
        let new = state::next(&slot.status(), &event).map_err(|refused| refused.0)?;
        slot.ticket += 1;
        self.set(id, slot, new);
        Ok(slot.ticket)
    }

    /// Applies something that happened, for the run it belongs to (or any run with `None`).
    fn apply(&self, id: &str, run: Option<u64>, event: Event) -> Status {
        let mut slots = lock(&self.shared.slots);
        let slot = slots.entry(id.to_string()).or_default();
        if run.is_some_and(|run| run != slot.run) {
            return slot.status();
        }
        // Only what the user asks can be refused; the rest changes nothing when it does not fit.
        let new = state::next(&slot.status(), &event).unwrap_or_else(|_| slot.status());
        self.set(id, slot, new.clone());
        new
    }

    /// Stores a new status and tells everyone if it is a change.
    fn set(&self, id: &str, slot: &mut Slot, new: Status) {
        if slot.status() == new {
            return;
        }
        eprintln!(
            "guesthouse-agent: {id}: {}{}",
            new.name(),
            new.reason().map(|r| format!(": {r}")).unwrap_or_default()
        );
        let line = StateChange::line(MachineState::new(id, &new));
        slot.status = Some(new);
        lock(&self.shared.subscribers).retain(|(_, sender)| sender.send(line.clone()).is_ok());
    }
}

/// The machine id a run folder belongs to: the folder's name.
fn id_of(run_dir: &Path) -> String {
    run_dir.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
}

/// A lock that a panicking thread did not poison for good: the data is plain state.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Why QEMU ended badly: the first line it printed, else how it ended.
fn failure_reason(run_dir: &Path, status: ExitStatus) -> String {
    if let Some(line) = first_line(&run_dir.join(LOG_FILE)) {
        return line;
    }
    match (status.code(), status.signal()) {
        (Some(code), _) => format!("QEMU exited with status {code}"),
        (None, Some(signal)) => format!("QEMU was stopped by signal {signal}"),
        (None, None) => "QEMU ended".into(),
    }
}

/// The first line of a file that is not blank, cut to a reasonable length.
fn first_line(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    Some(line.chars().take(REASON_LIMIT).collect())
}
