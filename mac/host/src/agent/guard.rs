// SPDX-License-Identifier: GPL-2.0-or-later
//! The guarded path: the graphics device runs in `guesthouse-pvg-host`, a process of its own,
//! and QEMU forwards to it. The agent starts it first, waits for its socket, then starts QEMU;
//! the two live and die together.
//!
//! - pvg-host leaves by itself when QEMU goes. The agent gives it `HOST_EXIT_WAIT` and then
//!   ends it, so a graphics process is never left behind a machine that is off.
//! - If pvg-host dies first, the machine is failed with its last output line and QEMU is
//!   stopped, since a guest with no graphics device is of no use to anyone.
//! - It is tracked by pid and start time in `host.pid` in the run folder, like QEMU and rmhost,
//!   and found again by a new agent. Its output is `host.out`, as run.sh keeps it.

use super::{Agent, REASON_LIMIT, lock};
use crate::launch::{self, Launch};
use crate::process::{self, Pid};
use crate::state::{Event, Status};
use crate::tools::{self, Tool};
use std::fs::{self, File};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const HOST_PID_FILE: &str = "host.pid";
const HOST_LOG: &str = "host.out";
/// How long the graphics process has to open its socket (run.sh waits five seconds).
const SOCKET_WAIT: Duration = Duration::from_secs(5);
/// How long it has to go after QEMU, and then how long after being asked to.
const HOST_EXIT_WAIT: Duration = Duration::from_secs(3);
const HOST_TERM_WAIT: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_millis(50);

impl Agent {
    /// Checks the programs a guarded start needs: a QEMU with `pvg-fwd` and a graphics process
    /// that publishes frames. Returns the graphics process's path.
    pub(super) fn check_guarded(&self, qemu: &Path, pvg_host: Option<&str>) -> Result<PathBuf, String> {
        let host = match pvg_host.filter(|path| !path.is_empty()) {
            Some(path) => PathBuf::from(path),
            None => self
                .shared
                .config
                .pvg_host
                .clone()
                .ok_or("There is no graphics process (guesthouse-pvg-host) to start the machine with.")?,
        };
        if !host.is_file() {
            return Err(format!("The graphics process is not at {}", host.display()));
        }
        tools::check(Tool::PvgQemu, qemu)?;
        tools::check(Tool::PvgHost, &host)?;
        Ok(host)
    }

    /// Starts the graphics process and waits for its socket. If it ends first, the start
    /// fails with the last line it printed.
    pub(super) fn start_host(&self, id: &str, run: u64, launch: &Launch, host: &Path) -> Result<(), String> {
        let run_dir = &launch.run_dir;
        for stale in
            [&launch.host_socket, &launch.host_control, &run_dir.join(HOST_PID_FILE), &run_dir.join(HOST_LOG)]
        {
            let _ = fs::remove_file(stale);
        }
        let log =
            File::create(run_dir.join(HOST_LOG)).map_err(|e| format!("cannot write {HOST_LOG}: {e}"))?;
        let mut child = Command::new(host)
            .args(launch::host_args(launch))
            .current_dir(run_dir)
            .stdin(Stdio::null())
            .stdout(log.try_clone().map_err(|e| e.to_string())?)
            .stderr(log)
            // Its own process group, so a signal meant for the agent does not reach it.
            .process_group(0)
            .spawn()
            .map_err(|e| format!("cannot run {}: {e}", host.display()))?;
        let pid = process::identify(child.id() as i32)
            .unwrap_or(Pid { pid: child.id() as i32, started: "0.000000".into() });
        let _ = process::write_file(&run_dir.join(HOST_PID_FILE), &pid);
        if let Some(slot) = lock(&self.shared.slots).get_mut(id) {
            slot.host = Some(pid);
        }
        let deadline = Instant::now() + SOCKET_WAIT;
        loop {
            if is_socket(&launch.host_socket) {
                break;
            }
            if let Ok(Some(_)) = child.try_wait() {
                let _ = fs::remove_file(run_dir.join(HOST_PID_FILE));
                self.forget_host(id);
                let line = last_line(&run_dir.join(HOST_LOG)).unwrap_or_else(|| "it ended".into());
                return Err(format!("The graphics process stopped before it was ready: {line}"));
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                let _ = fs::remove_file(run_dir.join(HOST_PID_FILE));
                self.forget_host(id);
                return Err("The graphics process did not open its socket in time.".into());
            }
            thread::sleep(POLL);
        }
        // From here its end is news: watched for as long as the machine runs.
        let agent = self.clone();
        let (watch_id, watch_dir) = (id.to_string(), run_dir.clone());
        thread::spawn(move || {
            let _ = child.wait();
            agent.host_ended(&watch_id, run, &watch_dir);
        });
        Ok(())
    }

    fn forget_host(&self, id: &str) {
        if let Some(slot) = lock(&self.shared.slots).get_mut(id) {
            slot.host = None;
        }
    }

    /// The graphics process ended. If QEMU is still there it ended first, which fails the
    /// machine, with the process's last words, and stops QEMU. If QEMU is gone it was only
    /// following it.
    fn host_ended(&self, id: &str, run: u64, run_dir: &Path) {
        let qemu = {
            let slots = lock(&self.shared.slots);
            let Some(slot) = slots.get(id).filter(|slot| slot.run == run) else {
                return;
            };
            // None: QEMU is not started yet, and the start in progress is the one to say so.
            slot.pid.clone()
        };
        let _ = fs::remove_file(run_dir.join(HOST_PID_FILE));
        self.forget_host(id);
        let Some(qemu) = qemu.filter(process::is_running) else {
            return;
        };
        let line = last_line(&run_dir.join(HOST_LOG)).unwrap_or_else(|| "it ended".into());
        eprintln!("guesthouse-agent: {id}: the graphics process ended first: {line}");
        let reason = format!("The graphics process ended: {line}");
        let status = self.apply(id, Some(run), Event::Exited { clean: false, reason });
        if matches!(status, Status::Failed { .. }) {
            process::terminate(&qemu);
        }
    }

    /// Ends the graphics process of a machine that has stopped: it leaves by itself when QEMU
    /// does, so it is given a moment, then asked, then killed.
    pub(super) fn stop_host(&self, id: &str) {
        let pid = lock(&self.shared.slots).get_mut(id).and_then(|slot| slot.host.take());
        let Some(pid) = pid else { return };
        end(&pid);
        let _ = fs::remove_file(self.run_dir_of(id).join(HOST_PID_FILE));
    }

    fn run_dir_of(&self, id: &str) -> PathBuf {
        crate::store::run_folder(&self.shared.config.machines, id)
    }

    /// A graphics process whose QEMU is gone, found at start-up.
    pub(super) fn end_leftover_host(&self, id: &str, run_dir: &Path) {
        let file = run_dir.join(HOST_PID_FILE);
        if let Some(pid) = process::read_file(&file)
            && process::is_running(&pid)
        {
            eprintln!("guesthouse-agent: {id}: ending a graphics process left without its QEMU");
            end(&pid);
        }
        let _ = fs::remove_file(file);
    }

    /// Finds the graphics process of a machine found running again, and watches it.
    pub(super) fn adopt_host(&self, id: &str, run: u64, run_dir: &Path) {
        let Some(pid) = process::read_file(&run_dir.join(HOST_PID_FILE)) else {
            return;
        };
        if !process::is_running(&pid) {
            let _ = fs::remove_file(run_dir.join(HOST_PID_FILE));
            return;
        }
        if let Some(slot) = lock(&self.shared.slots).get_mut(id) {
            slot.host = Some(pid.clone());
        }
        eprintln!("guesthouse-agent: {id}: found the graphics process again, pid {}", pid.pid);
        let agent = self.clone();
        let (id, dir) = (id.to_string(), run_dir.to_path_buf());
        thread::spawn(move || {
            while process::is_running(&pid) {
                thread::sleep(Duration::from_millis(500));
            }
            agent.host_ended(&id, run, &dir);
        });
    }
}

/// Gives a process that should be leaving a moment, then asks, then kills.
fn end(pid: &Pid) {
    wait_gone(pid, HOST_EXIT_WAIT);
    if process::is_running(pid) {
        process::terminate(pid);
        wait_gone(pid, HOST_TERM_WAIT);
    }
    if process::is_running(pid) {
        // SAFETY: kill has no memory effects; the pid was checked just above.
        unsafe { libc::kill(pid.pid, libc::SIGKILL) };
    }
}

fn wait_gone(pid: &Pid, limit: Duration) {
    let deadline = Instant::now() + limit;
    while process::is_running(pid) && Instant::now() < deadline {
        thread::sleep(POLL);
    }
}

fn is_socket(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|meta| meta.file_type().is_socket())
}

/// The last line of a file that is not blank, cut to a reasonable length.
fn last_line(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let line = text.lines().rev().map(str::trim).find(|line| !line.is_empty())?;
    Some(line.chars().take(REASON_LIMIT).collect())
}
