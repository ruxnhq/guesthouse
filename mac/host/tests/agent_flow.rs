// SPDX-License-Identifier: GPL-2.0-or-later
//! The agent starting, watching and stopping machines, with `fake_qemu` standing in for QEMU.
#![cfg(target_os = "macos")]

use guesthouse_core::agent::{Agent, Config};
use guesthouse_core::protocol::{MachineState, Response};
use guesthouse_core::server::{self, Connection};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

const FAKE_QEMU: &str = env!("CARGO_BIN_EXE_fake_qemu");
const WAIT: Duration = Duration::from_secs(15);

/// A support folder for one test, with a machines folder in it, removed afterwards.
struct Support(PathBuf);

impl Drop for Support {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

impl Support {
    fn new() -> Support {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "gha-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(path.join("Machines")).unwrap();
        Support(path)
    }

    /// A startable machine whose disk is called `{id}-{quirk}.img`.
    fn machine(&self, id: &str, quirk: &str) {
        let folder = self.0.join("Machines").join(id);
        fs::create_dir_all(&folder).unwrap();
        for file in ["code.fd", "vars.fd", &format!("{id}-{quirk}.img")] {
            fs::write(folder.join(file), b"").unwrap();
        }
        let json = format!(
            r#"{{"name": "{id}", "kind": "windows", "cores": 2, "memory_mb": 512,
                "disk": "{id}-{quirk}.img", "firmware_code": "code.fd", "firmware_vars": "vars.fd",
                "graphics": "qemu"}}"#
        );
        fs::write(folder.join("machine.json"), json).unwrap();
    }

    fn config(&self, qemu: Option<&str>) -> Config {
        Config {
            machines: self.0.join("Machines"),
            qemu: qemu.map(PathBuf::from),
            rmhost: None,
            pvg_host: None,
            install: Default::default(),
            signed: false,
            shutdown_timeout: Duration::from_millis(1500),
            start_timeout: Duration::from_secs(10),
        }
    }

    fn agent(&self) -> Agent {
        Agent::new(self.config(Some(FAKE_QEMU)))
    }

    fn run_folder(&self, id: &str) -> PathBuf {
        self.0.join("Run").join(id)
    }
}

fn request(agent: &Agent, text: &str) -> Response {
    serde_json::from_str(&agent.handle(text)).unwrap()
}

fn op(agent: &Agent, op: &str, machine: &str) -> Response {
    request(agent, &format!(r#"{{"v":1,"id":1,"op":"{op}","machine":"{machine}"}}"#))
}

fn states_of(agent: &Agent) -> Vec<MachineState> {
    request(agent, r#"{"v":1,"id":1,"op":"list"}"#).machines.unwrap()
}

fn state_of(agent: &Agent, id: &str) -> MachineState {
    states_of(agent).into_iter().find(|state| state.machine == id).unwrap()
}

/// Reads events until `machine` reaches `state`, and returns the states it went through.
fn expect(events: &Receiver<String>, machine: &str, state: &str) -> Vec<String> {
    let deadline = Instant::now() + WAIT;
    let mut seen = Vec::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        let Ok(line) = events.recv_timeout(left) else {
            break;
        };
        let event: serde_json::Value = serde_json::from_str(&line).unwrap();
        if event["event"] != "state" || event["machine"] != machine {
            continue;
        }
        let name = event["state"].as_str().unwrap().to_string();
        seen.push(name.clone());
        if name == state {
            return seen;
        }
    }
    panic!("{machine} never reached {state}; saw {seen:?}");
}

fn listen(agent: &Agent) -> (Receiver<String>, guesthouse_core::agent::Subscription) {
    let (sender, receiver) = mpsc::channel();
    let subscription = agent.subscribe(sender);
    (receiver, subscription)
}

fn args_of(support: &Support, id: &str) -> Vec<String> {
    let file = support.run_folder(id).join("fake-qemu.args");
    fs::read_to_string(file).unwrap().lines().map(String::from).collect()
}

#[test]
fn start_run_and_shut_down() {
    let support = Support::new();
    support.machine("win", "plain");
    let agent = support.agent();
    let (events, _keep) = listen(&agent);
    assert_eq!(state_of(&agent, "win").state, "off");

    assert!(op(&agent, "start", "win").ok);
    assert_eq!(expect(&events, "win", "running"), ["starting", "running"]);
    assert_eq!(state_of(&agent, "win").state, "running");

    // QEMU was given the sockets in the run folder and the frame name for this machine.
    let args = args_of(&support, "win");
    let run = support.run_folder("win");
    assert!(args.contains(&format!("unix:{},server,nowait", run.join("qmp.sock").display())));
    assert!(args.contains(&format!("unix:{},server,nowait", run.join("input.sock").display())));
    assert!(args.last().unwrap().contains(r#""frame-export":"guesthouse.win""#));
    assert!(run.join("machine.pid").exists());

    assert!(op(&agent, "shutdown", "win").ok);
    assert_eq!(expect(&events, "win", "off"), ["stopping", "off"]);
    assert!(!run.join("machine.pid").exists());
}

#[test]
fn starting_twice_and_shutting_down_a_stopped_machine_are_refused() {
    let support = Support::new();
    support.machine("win", "plain");
    let agent = support.agent();
    let (events, _keep) = listen(&agent);
    let refused = op(&agent, "shutdown", "win");
    assert!(!refused.ok);
    assert_eq!(refused.error.unwrap(), "the machine is not running");
    assert!(op(&agent, "start", "win").ok);
    assert!(!op(&agent, "start", "win").ok);
    expect(&events, "win", "running");
    assert!(!op(&agent, "start", "win").ok);
    assert!(!op(&agent, "start", "nobody").ok);
    assert!(op(&agent, "force_off", "win").ok);
    expect(&events, "win", "off");
}

#[test]
fn a_start_that_fails_says_why_from_the_first_line() {
    let support = Support::new();
    support.machine("bad", "failstart");
    let agent = support.agent();
    let (events, _keep) = listen(&agent);
    assert!(op(&agent, "start", "bad").ok);
    assert_eq!(expect(&events, "bad", "failed"), ["starting", "failed"]);
    let state = state_of(&agent, "bad");
    assert_eq!(state.reason.unwrap(), "fake-qemu: could not open the disk image: Operation not permitted");
    // A failed machine can be started again.
    assert!(op(&agent, "start", "bad").ok);
    expect(&events, "bad", "failed");
}

#[test]
fn no_qemu_or_a_wrong_path_is_a_failure_with_a_reason() {
    let support = Support::new();
    support.machine("win", "plain");
    let agent = Agent::new(support.config(None));
    let answer = op(&agent, "start", "win");
    assert!(!answer.ok);
    assert!(answer.error.unwrap().contains("Settings"));
    assert_eq!(state_of(&agent, "win").state, "failed");
    let wrong = request(&agent, r#"{"v":1,"id":2,"op":"start","machine":"win","qemu":"/nowhere/qemu"}"#);
    assert_eq!(wrong.error.unwrap(), "QEMU is not at /nowhere/qemu");
    // A machine whose files are gone is reported before anything runs.
    fs::remove_file(support.0.join("Machines/win/code.fd")).unwrap();
    let missing = op(&agent, "start", "win");
    assert!(missing.error.unwrap().contains("firmware code"));
}

#[test]
fn a_machine_without_firmware_files_cannot_be_started() {
    let support = Support::new();
    let folder = support.0.join("Machines/hand");
    fs::create_dir_all(&folder).unwrap();
    fs::write(
        folder.join("machine.json"),
        r#"{"name":"Hand","kind":"windows","cores":1,"memory_mb":1,"disk":"/d"}"#,
    )
    .unwrap();
    let agent = support.agent();
    let answer = op(&agent, "start", "hand");
    assert!(answer.error.unwrap().contains("cannot be started"));
    assert_eq!(state_of(&agent, "hand").state, "off");
}

#[test]
fn a_guest_that_ignores_the_power_button_stalls_and_force_off_ends_it() {
    let support = Support::new();
    support.machine("slow", "stubborn");
    let agent = support.agent();
    let (events, _keep) = listen(&agent);
    assert!(op(&agent, "start", "slow").ok);
    expect(&events, "slow", "running");
    assert!(op(&agent, "shutdown", "slow").ok);
    expect(&events, "slow", "stopping");
    // After the timeout the shared state for a stalled shutdown is announced.
    expect(&events, "slow", "stopping_stalled");
    assert!(op(&agent, "force_off", "slow").ok);
    expect(&events, "slow", "off");
}

#[test]
fn force_off_falls_back_to_sigterm_when_quit_is_ignored() {
    let support = Support::new();
    support.machine("deaf", "deaf");
    let agent = support.agent();
    let (events, _keep) = listen(&agent);
    assert!(op(&agent, "start", "deaf").ok);
    expect(&events, "deaf", "running");
    assert!(op(&agent, "force_off", "deaf").ok);
    expect(&events, "deaf", "off");
}

#[test]
fn a_machine_that_exits_by_itself_is_stopped_and_a_crash_is_failed() {
    let support = Support::new();
    support.machine("self", "selfexit");
    support.machine("boom", "crash");
    let agent = support.agent();
    let (events, _keep) = listen(&agent);
    assert!(op(&agent, "start", "self").ok);
    assert_eq!(expect(&events, "self", "off"), ["starting", "running", "off"]);
    assert!(op(&agent, "start", "boom").ok);
    expect(&events, "boom", "failed");
    assert_eq!(state_of(&agent, "boom").reason.unwrap(), "QEMU exited with status 3");
}

#[test]
fn a_new_agent_finds_a_running_machine_and_can_stop_it() {
    let support = Support::new();
    support.machine("win", "plain");
    let first = support.agent();
    let (events, _keep) = listen(&first);
    assert!(op(&first, "start", "win").ok);
    expect(&events, "win", "running");

    // Another agent over the same folders, as after the first one died.
    let second = support.agent();
    assert_eq!(state_of(&second, "win").state, "running");
    let (found, _keep_found) = listen(&second);
    assert!(op(&second, "shutdown", "win").ok);
    expect(&found, "win", "off");
}

#[test]
fn a_pid_file_of_something_else_is_not_a_machine() {
    let support = Support::new();
    support.machine("win", "plain");
    let run = support.run_folder("win");
    fs::create_dir_all(&run).unwrap();
    // This very test process's pid with the wrong start time: a recycled pid.
    fs::write(run.join("machine.pid"), format!("{} 1.000000\n", std::process::id())).unwrap();
    let agent = support.agent();
    assert_eq!(state_of(&agent, "win").state, "off");
    assert!(!run.join("machine.pid").exists());
}

#[test]
fn a_signed_agent_runs_the_programs_of_its_bundle_whatever_a_request_names() {
    let support = Support::new();
    support.machine("win", "plain");
    let mut config = support.config(Some("/Bundle.app/Contents/Helpers/qemu-system-aarch64"));
    config.signed = true;
    let agent = Agent::new(config);
    let line = format!(r#"{{"v":1,"id":2,"op":"start","machine":"win","qemu":"{FAKE_QEMU}"}}"#);
    let answer = request(&agent, &line);
    assert_eq!(answer.error.unwrap(), "QEMU is not at /Bundle.app/Contents/Helpers/qemu-system-aarch64");
}

#[test]
fn a_signed_agent_does_not_answer_on_a_socket() {
    let support = Support::new();
    let mut config = support.config(Some(FAKE_QEMU));
    config.signed = true;
    let agent = Agent::new(config);
    let socket = std::ffi::CString::new(support.0.join("agent.sock").to_str().unwrap()).unwrap();
    // SAFETY: the agent outlives the call and the path is a nul-terminated string.
    let ended = unsafe { guesthouse_core::agent_api::guesthouse_agent_serve(&agent, socket.as_ptr()) };
    assert_eq!(ended, -1);
    assert!(!support.0.join("agent.sock").exists());
}

#[test]
fn the_socket_carries_answers_and_events_and_the_agent_leaves_when_idle() {
    let support = Support::new();
    support.machine("win", "plain");
    let agent = support.agent();
    let socket = support.0.join("agent.sock");
    let serving = {
        let (agent, socket) = (agent.clone(), socket.clone());
        thread::spawn(move || server::serve(&agent, &socket))
    };
    let deadline = Instant::now() + WAIT;
    let connection = loop {
        match Connection::open(&socket) {
            Ok(connection) => break connection,
            Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            Err(error) => panic!("no socket: {error}"),
        }
    };
    let wait_for = |needle: &str| -> String {
        let deadline = Instant::now() + WAIT;
        while Instant::now() < deadline {
            if let Some(line) = connection.receive(Duration::from_millis(200)).unwrap()
                && line.contains(needle)
            {
                return line;
            }
        }
        panic!("never saw {needle}");
    };
    connection.send(r#"{"v":1,"id":11,"op":"list"}"#).unwrap();
    assert!(wait_for(r#""id":11"#).contains(r#""state":"off""#));
    connection.send(r#"{"v":1,"id":12,"op":"start","machine":"win"}"#).unwrap();
    wait_for(r#""id":12,"ok":true"#);
    wait_for(r#""event":"state","machine":"win","state":"running""#);
    connection.send(r#"{"v":1,"id":13,"op":"shutdown","machine":"win"}"#).unwrap();
    wait_for(r#""state":"off""#);
    drop(connection);

    // A second server on the same socket is refused while the first holds the lock.
    assert!(matches!(server::serve(&agent, &socket), Err(server::ServeError::AlreadyRunning)));
    // With nobody connected and nothing running, the server ends by itself.
    let started = Instant::now();
    serving.join().unwrap().unwrap();
    assert!(started.elapsed() < Duration::from_secs(20));
    assert!(!Path::new(&socket).exists());
}

#[test]
fn a_listing_says_which_build_the_agent_is_once_it_was_told() {
    let support = Support::new();
    let agent = support.agent();
    let listing = || request(&agent, r#"{"v":1,"id":1,"op":"list"}"#);
    assert!(listing().agent.is_none(), "an agent not told its build says none");
    agent.set_build("0.1.2", "42");
    let said = listing().agent.expect("the build is in the listing");
    assert_eq!((said.version.as_str(), said.build.as_str()), ("0.1.2", "42"));
    // It is the program running the test, which is what the agent says it is.
    let own = std::env::current_exe().unwrap().canonicalize().unwrap();
    assert_eq!(said.path, own.to_str().unwrap());
}
