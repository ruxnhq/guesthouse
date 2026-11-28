// SPDX-License-Identifier: GPL-2.0-or-later
//! Running `rmhost serve` for a machine, so another device can view its screen with
//! remoteman's viewer.
//!
//! rmhost is a separate program, started and ended like QEMU is: in its own process group,
//! named by its pid and start time in `rmhost.pid` in the machine's run folder, and found
//! again by a new agent. It runs only while the machine runs and the person has allowed it.
//! What it prints goes to `rmhost.log`, which a thread reads back to learn the fingerprint,
//! whether pairing is open and who is watching.
//!
//! Pairing is opened on purpose and closes by itself, with no restart and viewers staying
//! connected: the agent writes `pair open SECONDS` to rmhost's standard input, and rmhost
//! closes it at the end of the time or after one device pairs, and says so in its log. The
//! standard input is a named pipe in the run folder (`rmhost.in`), so a later agent can send
//! the same lines to an rmhost it found running.

use super::{Agent, Slot, lock};
use crate::process::{self, Pid};
use crate::protocol::{ShareInfo, ShareState, share_event_line};
use crate::remote::{self, LogEvent};
use crate::state::Status;
use crate::store::{self, Machine};
use guesthouse_remote_view_core::ctl::Words;
use guesthouse_remote_view_core::pairing::Event as PairEvent;
use guesthouse_remote_view_core::policy::Policy;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const LOG_POLL: Duration = Duration::from_millis(200);
/// How long rmhost has to go after SIGTERM before it is killed.
const STOP_WAIT: Duration = Duration::from_secs(3);
/// How long rmhost has to say it is listening before the share counts as failed to start.
const LISTEN_WAIT: Duration = Duration::from_secs(15);

/// A machine's share, kept in its slot.
#[derive(Default)]
pub(super) struct ShareSlot {
    info: Option<ShareInfo>,
    pid: Option<Pid>,
    /// Counts runs of rmhost: the threads of an older one do nothing.
    generation: u64,
    /// The program, as the app last named it, so a machine started later can use it.
    rmhost: Option<PathBuf>,
}

impl Agent {
    /// Remembers where the app says rmhost is, for when the machine is up.
    pub(super) fn remember_rmhost(&self, id: &str, path: Option<&str>) {
        if let Some(path) = path.filter(|path| !path.is_empty()) {
            lock(&self.shared.slots).entry(id.to_string()).or_default().share.rmhost =
                Some(PathBuf::from(path));
        }
    }

    /// The op `share`: starts or stops sharing, and with `pairing` (seconds) opens pairing. A
    /// share that is up is not restarted for that.
    pub(super) fn share(
        &self,
        id: &str,
        on: bool,
        pairing: Option<u64>,
        rmhost: Option<&str>,
    ) -> Result<(), String> {
        // The shared rule for how long a pairing may be opened for, with its sentence.
        if let Some(seconds) = pairing {
            guesthouse_remote_view_core::share::pairing_seconds(Some(seconds))?;
        }
        if !on {
            self.share_stop(id);
            return Ok(());
        }
        self.remember_rmhost(id, rmhost);
        if !matches!(self.share_state(id), Some(ShareState::Starting | ShareState::Listening)) {
            self.share_spawn(id)?;
        }
        match pairing {
            Some(seconds) => self.pairing_event(id, PairEvent::Open { seconds }),
            None => Ok(()),
        }
    }

    /// The ops `pair_close`, `pair_accept` and `pair_decline`: the person's event to the shared
    /// pairing, which sends rmhost what follows.
    pub(super) fn pair_event(&self, id: &str, event: PairEvent<'static>) -> Result<(), String> {
        if !matches!(self.share_state(id), Some(ShareState::Starting | ShareState::Listening)) {
            return Err("This machine's screen is not being shared.".into());
        }
        self.pairing_event(id, event)
    }

    /// The ops that name a device (`device_allow`, `device_revoke`, `device_drop`): the shared
    /// pairing's word for it, which checks the fingerprint, given to rmhost without an answer.
    pub(super) fn device_command(
        &self,
        id: &str,
        words: impl FnOnce(&Policy) -> Result<Words, String>,
    ) -> Result<(), String> {
        if !matches!(self.share_state(id), Some(ShareState::Starting | ShareState::Listening)) {
            return Err("This machine's screen is not being shared.".into());
        }
        self.pairing_device(id, words)
    }

    /// Writes a line to rmhost's input. The pipe is opened for a moment each time, so it works
    /// as well for an rmhost that an earlier agent started.
    pub(super) fn send_to_rmhost(&self, id: &str, line: &str) -> Result<(), String> {
        let pipe = self.run_dir(id).join(remote::INPUT_FILE);
        // Non-blocking: with no reader the open fails at once instead of waiting.
        let mut file = OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&pipe)
            .map_err(|e| format!("cannot reach rmhost: {e}"))?;
        file.write_all(line.as_bytes()).map_err(|e| format!("cannot reach rmhost: {e}"))
    }

    /// A machine is running (it has just come up, or an agent found it running again): shares
    /// its screen if the person allowed it, however it was started.
    pub(super) fn share_when_ready(&self, id: &str) {
        let allowed = store::find(&self.shared.config.machines, id).is_ok_and(|m| m.remote_view);
        if allowed && let Err(reason) = self.share_spawn(id) {
            eprintln!("guesthouse-agent: {id}: not sharing the screen: {reason}");
            // Say so where the app can see it, rather than only in the agent's own log.
            let generation = self.share_generation(id);
            self.fail_share(id, generation, reason);
        }
    }

    /// Every share that is on, or failed, for a listing.
    pub(super) fn shares(&self) -> Vec<ShareInfo> {
        lock(&self.shared.slots)
            .values()
            .filter_map(|slot| slot.share.info.clone())
            .filter(|info| info.state != ShareState::Off)
            .collect()
    }

    /// Ends the share, and waits a little for rmhost to go.
    pub(super) fn share_stop(&self, id: &str) {
        let pid = {
            let mut slots = lock(&self.shared.slots);
            let Some(slot) = slots.get_mut(id) else {
                return;
            };
            if slot.share.info.is_none() && slot.share.pid.is_none() {
                return;
            }
            slot.share.generation += 1;
            self.pairing_ended(id);
            let pid = slot.share.pid.take();
            self.publish_share(id, slot, ShareInfo::off(id));
            pid
        };
        let _ = fs::remove_file(self.run_dir(id).join(remote::PID_FILE));
        let Some(pid) = pid else { return };
        process::terminate(&pid);
        let deadline = Instant::now() + STOP_WAIT;
        while process::is_running(&pid) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
        if process::is_running(&pid) {
            // SAFETY: kill has no memory effects; the pid was checked just above.
            unsafe { libc::kill(pid.pid, libc::SIGKILL) };
        }
    }

    fn run_dir(&self, id: &str) -> PathBuf {
        store::run_folder(&self.shared.config.machines, id)
    }

    /// Starts rmhost for the running machine `id`, replacing one that is already there.
    fn share_spawn(&self, id: &str) -> Result<(), String> {
        let mut machine = store::find(&self.shared.config.machines, id)?;
        if !machine.remote_view {
            return Err("Allow viewing from other devices is off for this machine.".into());
        }
        // A file turned on by hand has no port yet: one is chosen now, as when the switch is
        // flipped in the window.
        if machine.remote_port.is_none() {
            remote::set_enabled(&self.shared.config.machines, id, true)?;
            machine = store::find(&self.shared.config.machines, id)?;
        }
        let launch = machine.launch.clone().ok_or("this machine cannot be started by the app")?;
        let port = machine.remote_port.ok_or("no port could be chosen for this machine")?;
        let (running, rmhost) = {
            let slots = lock(&self.shared.slots);
            let slot = slots.get(id);
            (
                slot.is_some_and(|slot| slot.status() == Status::Running),
                // The program the app named, else the agent's own (`GUESTHOUSE_RMHOST`).
                slot.and_then(|slot| slot.share.rmhost.clone()).or_else(|| self.shared.config.rmhost.clone()),
            )
        };
        if !running {
            return Err("The machine is not running.".into());
        }
        let rmhost = rmhost
            .filter(|path| path.is_file())
            .ok_or("rmhost, the program that shares the screen, was not found.")?;

        self.share_stop(id);
        let generation = self.begin_share(id, &machine)?;
        let run_dir = launch.run_dir.clone();
        let spawned = self.start_rmhost(&rmhost, &machine, &launch, port, &run_dir);
        let child = match spawned {
            Ok(child) => child,
            Err(reason) => {
                self.fail_share(id, generation, reason.clone());
                return Err(reason);
            }
        };
        let pid = process::identify(child.id() as i32)
            .unwrap_or(Pid { pid: child.id() as i32, started: "0.000000".into() });
        let _ = process::write_file(&run_dir.join(remote::PID_FILE), &pid);
        if let Some(slot) = lock(&self.shared.slots).get_mut(id) {
            slot.share.pid = Some(pid);
        }
        let agent = self.clone();
        let (watch_id, watch_dir) = (id.to_string(), run_dir.clone());
        thread::spawn(move || agent.watch_rmhost_child(&watch_id, generation, child, &watch_dir));
        self.spawn_log_reader(id, generation, &run_dir);
        Ok(())
    }

    /// Opens a new generation with the share in `starting`.
    fn begin_share(&self, id: &str, machine: &Machine) -> Result<u64, String> {
        let mut slots = lock(&self.shared.slots);
        let slot = slots.get_mut(id).ok_or("The machine is not running.")?;
        slot.share.generation += 1;
        let info = ShareInfo {
            machine: id.to_string(),
            state: ShareState::Starting,
            port: machine.remote_port,
            ..ShareInfo::default()
        };
        let generation = slot.share.generation;
        // A new rmhost: what the last one had open went with it.
        self.pairing_restart(id);
        self.publish_share(id, slot, info);
        Ok(generation)
    }

    fn start_rmhost(
        &self,
        rmhost: &Path,
        machine: &Machine,
        launch: &crate::launch::Launch,
        port: u16,
        run_dir: &Path,
    ) -> Result<std::process::Child, String> {
        let folder = Path::new(&machine.folder);
        fs::create_dir_all(remote::state_folder(folder))
            .map_err(|e| format!("cannot make the sharing folder: {e}"))?;
        let log = File::create(run_dir.join(remote::LOG_FILE))
            .map_err(|e| format!("cannot write {}: {e}", remote::LOG_FILE))?;
        let input = open_input_pipe(&run_dir.join(remote::INPUT_FILE))?;
        Command::new(rmhost)
            .args(remote::serve_args(launch, folder, &machine.name, port))
            .current_dir(run_dir)
            // Opened for reading and writing, so rmhost holds a writer itself and never sees
            // the end of its input, however the agent comes and goes.
            .stdin(Stdio::from(input))
            .stdout(log.try_clone().map_err(|e| e.to_string())?)
            .stderr(log)
            // Its own process group, so a signal meant for the agent does not reach it.
            .process_group(0)
            .spawn()
            .map_err(|e| format!("cannot run {}: {e}", rmhost.display()))
    }

    /// Waits for rmhost, our own child, and tells how it ended.
    fn watch_rmhost_child(&self, id: &str, generation: u64, mut child: std::process::Child, run_dir: &Path) {
        let status = child.wait();
        self.share_ended(id, generation, run_dir, status.ok());
    }

    /// Reads the log as it grows, and follows what it says.
    fn spawn_log_reader(&self, id: &str, generation: u64, run_dir: &Path) {
        let agent = self.clone();
        let (id, log) = (id.to_string(), run_dir.join(remote::LOG_FILE));
        thread::spawn(move || {
            let started = Instant::now();
            let mut seen = 0;
            while agent.share_generation(&id) == generation {
                let text = fs::read_to_string(&log).unwrap_or_default();
                for line in text.lines().skip(seen) {
                    if let Some(event) = remote::parse_line(line) {
                        agent.follow(&id, generation, event);
                    }
                }
                seen = text.lines().count();
                if started.elapsed() > LISTEN_WAIT && agent.share_state(&id) == Some(ShareState::Starting) {
                    agent.fail_share(&id, generation, "rmhost did not start listening.".into());
                    return;
                }
                thread::sleep(LOG_POLL);
            }
        });
    }

    /// What a line from rmhost changes in the share.
    fn follow(&self, id: &str, generation: u64, event: LogEvent) {
        let mut slots = lock(&self.shared.slots);
        let Some(slot) = slots.get_mut(id) else {
            return;
        };
        if slot.share.generation != generation {
            return;
        }
        let Some(mut info) = slot.share.info.clone() else {
            return;
        };
        if let Some(said) = remote::pairing_event(&event) {
            self.pairing_says(id, &said);
        }
        apply(&mut info, event);
        let info = self.with_pairing_shown(id, info);
        self.publish_share(id, slot, info);
    }

    /// rmhost went. If the agent did not end it, that is a failure, and its last words say why.
    fn share_ended(&self, id: &str, generation: u64, run_dir: &Path, status: Option<ExitStatus>) {
        if self.share_generation(id) != generation {
            return;
        }
        let _ = fs::remove_file(run_dir.join(remote::PID_FILE));
        let reason = last_words(&run_dir.join(remote::LOG_FILE)).unwrap_or_else(|| match status {
            Some(status) => match (status.code(), status.signal()) {
                (Some(code), _) => format!("rmhost exited with status {code}"),
                (None, Some(signal)) => format!("rmhost was stopped by signal {signal}"),
                (None, None) => "rmhost ended".into(),
            },
            None => "rmhost ended".into(),
        });
        self.fail_share(id, generation, reason);
    }

    fn fail_share(&self, id: &str, generation: u64, reason: String) {
        let mut slots = lock(&self.shared.slots);
        let Some(slot) = slots.get_mut(id) else {
            return;
        };
        if slot.share.generation != generation {
            return;
        }
        slot.share.generation += 1;
        self.pairing_ended(id);
        let port = slot.share.info.as_ref().and_then(|info| info.port);
        let info = ShareInfo {
            machine: id.to_string(),
            state: ShareState::Failed,
            port,
            reason: Some(reason),
            ..ShareInfo::default()
        };
        self.publish_share(id, slot, info);
    }

    fn share_generation(&self, id: &str) -> u64 {
        lock(&self.shared.slots).get(id).map_or(0, |slot| slot.share.generation)
    }

    pub(super) fn share_state(&self, id: &str) -> Option<ShareState> {
        lock(&self.shared.slots).get(id).and_then(|slot| slot.share.info.as_ref().map(|info| info.state))
    }

    /// Stores the share's new state and tells everyone connected if it is a change.
    fn publish_share(&self, id: &str, slot: &mut Slot, info: ShareInfo) {
        if slot.share.info.as_ref() == Some(&info)
            || (slot.share.info.is_none() && info.state == ShareState::Off)
        {
            return;
        }
        eprintln!("guesthouse-agent: {id}: sharing the screen: {:?}", info.state);
        let line = share_event_line(&info);
        slot.share.info = Some(info);
        lock(&self.shared.subscribers).retain(|(_, sender)| sender.send(line.clone()).is_ok());
    }

    /// Finds an rmhost that outlived the last agent, for a machine found running again.
    pub(super) fn adopt_share(&self, id: &str, run_dir: &Path) {
        let Some(pid) = process::read_file(&run_dir.join(remote::PID_FILE)) else {
            return;
        };
        if !process::is_running(&pid) {
            let _ = fs::remove_file(run_dir.join(remote::PID_FILE));
            return;
        }
        let mut info =
            ShareInfo { machine: id.to_string(), state: ShareState::Starting, ..ShareInfo::default() };
        // What it said earlier says where it stands now. Pairing may have been open: it is
        // reported as it was said, and not given a new time.
        let log = fs::read_to_string(run_dir.join(remote::LOG_FILE)).unwrap_or_default();
        self.pairing_restart(id);
        for event in log.lines().filter_map(remote::parse_line) {
            if let Some(said) = remote::pairing_event(&event) {
                self.pairing_says(id, &said);
            }
            apply(&mut info, event);
        }
        let info = self.with_pairing_shown(id, info);
        let generation = {
            let mut slots = lock(&self.shared.slots);
            let slot = slots.entry(id.to_string()).or_default();
            slot.share.generation += 1;
            slot.share.pid = Some(pid.clone());
            let generation = slot.share.generation;
            self.publish_share(id, slot, info);
            generation
        };
        eprintln!("guesthouse-agent: {id}: found rmhost running again, pid {}", pid.pid);
        let agent = self.clone();
        let (watch_id, dir) = (id.to_string(), run_dir.to_path_buf());
        thread::spawn(move || {
            while process::is_running(&pid) {
                thread::sleep(Duration::from_millis(500));
            }
            agent.share_ended(&watch_id, generation, &dir, None);
        });
        self.spawn_log_reader(id, generation, run_dir);
    }
}

/// What one line from rmhost changes in the share's state.
fn apply(info: &mut ShareInfo, event: LogEvent) {
    match event {
        LogEvent::Listening { port, fingerprint, pairing } => {
            info.state = ShareState::Listening;
            info.port = Some(port);
            info.fingerprint = Some(fingerprint);
            let _ = pairing;
        }
        LogEvent::Viewer(name) => info.viewer = Some(name),
        LogEvent::ViewerLeft => info.viewer = None,
        LogEvent::Devices(devices) => info.devices = devices,
        // What a screen shows of pairing comes from the shared pairing (`pairing.rs`).
        LogEvent::PairingOpen { .. }
        | LogEvent::PairingPending(_)
        | LogEvent::Paired { .. }
        | LogEvent::PairingClosed { .. } => {}
    }
}

/// Makes the named pipe that is rmhost's input, if it is not there, and opens it for reading
/// and writing.
fn open_input_pipe(path: &Path) -> Result<File, String> {
    if !path.exists() {
        let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).map_err(|e| e.to_string())?;
        // SAFETY: `name` is a nul-terminated path that outlives the call.
        if unsafe { libc::mkfifo(name.as_ptr(), 0o600) } != 0 {
            return Err(format!("cannot make {}: {}", path.display(), std::io::Error::last_os_error()));
        }
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| format!("cannot open {}: {e}", path.display()))
}

/// The last line rmhost printed that is a message of its own, which says why it ended.
fn last_words(log: &Path) -> Option<String> {
    let text = fs::read_to_string(log).ok()?;
    let line = text.lines().rev().map(str::trim).find(|line| !line.is_empty())?;
    Some(line.trim_start_matches("rmhost: ").chars().take(200).collect())
}
