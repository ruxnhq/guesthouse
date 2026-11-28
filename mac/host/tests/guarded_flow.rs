// SPDX-License-Identifier: GPL-2.0-or-later
//! The guarded path with stand-ins: `fake_pvg_host` for the graphics process and `fake_qemu`,
//! which connects to its socket as QEMU's `pvg-fwd` does. Covers the start order, a graphics
//! process that ends before its socket or while the machine runs, one that outstays its QEMU,
//! a new agent finding both again, and a program that is refused before anything starts.
#![cfg(target_os = "macos")]

use guesthouse_core::agent::{Agent, Config};
use guesthouse_core::process;
use guesthouse_core::protocol::{MachineState, Response, ShareState};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const FAKE_QEMU: &str = env!("CARGO_BIN_EXE_fake_qemu");
const FAKE_PVG_HOST: &str = env!("CARGO_BIN_EXE_fake_pvg_host");
const WAIT: Duration = Duration::from_secs(15);

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
            "ghg-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(path.join("Machines")).unwrap();
        Support(path)
    }

    /// A machine that says nothing about graphics, so it is on the guarded path.
    fn machine(&self, id: &str) {
        let folder = self.0.join("Machines").join(id);
        fs::create_dir_all(&folder).unwrap();
        for file in ["code.fd", "vars.fd", "disk.img"] {
            fs::write(folder.join(file), b"").unwrap();
        }
        let json = format!(
            r#"{{"name": "{id}", "kind": "windows", "cores": 2, "memory_mb": 512,
                "disk": "disk.img", "firmware_code": "code.fd", "firmware_vars": "vars.fd"}}"#
        );
        fs::write(folder.join("machine.json"), json).unwrap();
    }

    fn agent(&self) -> Agent {
        Agent::new(Config {
            machines: self.0.join("Machines"),
            qemu: Some(PathBuf::from(FAKE_QEMU)),
            rmhost: None,
            pvg_host: Some(PathBuf::from(FAKE_PVG_HOST)),
            install: Default::default(),
            signed: false,
            shutdown_timeout: Duration::from_secs(5),
            start_timeout: Duration::from_secs(10),
        })
    }

    /// Turns sharing on in the machine's file by hand, the way a person editing it would: no
    /// port is written.
    fn allow_sharing_by_hand(&self, id: &str) {
        let file = self.0.join("Machines").join(id).join("machine.json");
        let mut settings: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        settings["allow_remote_view"] = true.into();
        assert!(settings.get("remote_port").is_none());
        fs::write(file, settings.to_string()).unwrap();
    }

    /// A stand-in for rmhost: says what the real one says when it is listening, then waits on
    /// its input until it is ended.
    fn fake_rmhost(&self) -> PathBuf {
        let path = self.0.join("fake-rmhost");
        let body = "#!/bin/sh\nprintf 'rmhost: remoteman host \"x\" listening: 0.0.0.0:7800\\tabcd1234abcd1234 (address, fingerprint), pairing off\\n'\ncat >/dev/null\n";
        fs::write(&path, body).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn run(&self, id: &str) -> PathBuf {
        self.0.join("Run").join(id)
    }
}

fn request(agent: &Agent, text: &str) -> Response {
    serde_json::from_str(&agent.handle(text)).unwrap()
}

fn op(agent: &Agent, op: &str, id: &str) -> Response {
    request(agent, &format!(r#"{{"v":1,"id":1,"op":"{op}","machine":"{id}"}}"#))
}

fn state_of(agent: &Agent, id: &str) -> MachineState {
    let list = request(agent, r#"{"v":1,"id":1,"op":"list"}"#).machines.unwrap();
    list.into_iter().find(|s| s.machine == id).unwrap()
}

fn wait_for(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !check() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(50));
    }
}

fn wait_state(agent: &Agent, id: &str, state: &str) {
    wait_for(&format!("{id} to be {state}"), || state_of(agent, id).state == state);
}

fn pid_of(run: &Path, file: &str) -> process::Pid {
    process::read_file(&run.join(file)).unwrap_or_else(|| panic!("no {file} in {}", run.display()))
}

fn gone(pid: &process::Pid) -> bool {
    !process::is_running(pid)
}

#[test]
fn the_graphics_process_starts_first_and_both_end_when_the_machine_does() {
    let support = Support::new();
    support.machine("win");
    let agent = support.agent();
    assert!(op(&agent, "start", "win").ok);
    wait_state(&agent, "win", "running");

    let run = support.run("win");
    // QEMU connected to the process's socket, which is only possible if it was there first.
    let host_args = fs::read_to_string(run.join("fake-pvg-host.args")).unwrap();
    assert!(host_args.contains(&format!("--socket\n{}", run.join("host.sock").display())));
    assert!(host_args.contains("--frames\nguesthouse.win\n--modes\n"));
    assert!(!host_args.contains("--window"));
    let qemu_args = fs::read_to_string(run.join("fake-qemu.args")).unwrap();
    assert!(qemu_args.contains("virt,memory-backend=ram"));
    assert!(qemu_args.contains(&format!("pvg-fwd,id=pvg,socket={}", run.join("host.sock").display())));
    assert!(!qemu_args.contains("apple-gfx"));
    let (host, qemu) = (pid_of(&run, "host.pid"), pid_of(&run, "machine.pid"));
    assert!(host.started <= qemu.started || host.pid != qemu.pid);

    assert!(op(&agent, "shutdown", "win").ok);
    wait_state(&agent, "win", "off");
    wait_for("the graphics process to go", || gone(&host));
    assert!(!run.join("host.pid").exists());
}

#[test]
fn a_graphics_process_that_ends_before_its_socket_fails_the_start_with_its_last_line() {
    let support = Support::new();
    support.machine("hostfail");
    let agent = support.agent();
    assert!(op(&agent, "start", "hostfail").ok);
    wait_state(&agent, "hostfail", "failed");
    let reason = state_of(&agent, "hostfail").reason.unwrap();
    assert!(reason.contains("graphics process"), "{reason}");
    assert!(reason.contains("the frame name guesthouse.hostfail is taken"), "{reason}");
    // QEMU was never started.
    assert!(!support.run("hostfail").join("fake-qemu.args").exists());
    assert!(!support.run("hostfail").join("host.pid").exists());
    // And it can be tried again, which fails the same way.
    assert!(op(&agent, "start", "hostfail").ok);
    wait_state(&agent, "hostfail", "failed");
}

#[test]
fn a_graphics_process_that_dies_while_the_machine_runs_fails_it_and_stops_qemu() {
    let support = Support::new();
    support.machine("hostdie");
    let agent = support.agent();
    assert!(op(&agent, "start", "hostdie").ok);
    wait_state(&agent, "hostdie", "failed");
    let reason = state_of(&agent, "hostdie").reason.unwrap();
    assert!(reason.contains("The graphics process ended"), "{reason}");
    let qemu = process::read_file(&support.run("hostdie").join("machine.pid"));
    if let Some(qemu) = qemu {
        wait_for("QEMU to be stopped", || gone(&qemu));
    }
}

#[test]
fn a_graphics_process_that_outstays_its_qemu_is_ended() {
    let support = Support::new();
    support.machine("hoststuck");
    let agent = support.agent();
    assert!(op(&agent, "start", "hoststuck").ok);
    wait_state(&agent, "hoststuck", "running");
    let host = pid_of(&support.run("hoststuck"), "host.pid");
    assert!(op(&agent, "force_off", "hoststuck").ok);
    wait_state(&agent, "hoststuck", "off");
    // It stays when QEMU goes; after the moment it is given, it is asked and it leaves.
    wait_for("the stuck graphics process to be ended", || gone(&host));
}

#[test]
fn a_new_agent_finds_both_again_and_force_off_ends_both() {
    let support = Support::new();
    support.machine("win");
    let first = support.agent();
    assert!(op(&first, "start", "win").ok);
    wait_state(&first, "win", "running");
    let run = support.run("win");
    let (host, qemu) = (pid_of(&run, "host.pid"), pid_of(&run, "machine.pid"));

    let second = support.agent();
    assert_eq!(state_of(&second, "win").state, "running");
    assert!(op(&second, "force_off", "win").ok);
    wait_state(&second, "win", "off");
    wait_for("QEMU and the graphics process to go", || gone(&qemu) && gone(&host));
}

#[test]
fn nothing_is_left_behind_when_qemu_is_ended_from_outside() {
    let support = Support::new();
    support.machine("win");
    let agent = support.agent();
    assert!(op(&agent, "start", "win").ok);
    wait_state(&agent, "win", "running");
    let run = support.run("win");
    let (host, qemu) = (pid_of(&run, "host.pid"), pid_of(&run, "machine.pid"));
    // QEMU goes while no agent is looking: its pid file stays.
    let _ = fs::remove_file(run.join("fake-qemu.args"));
    process::terminate(&qemu);
    // (the first agent also notices; what matters is that nothing is left behind)
    wait_for("both gone", || gone(&qemu) && gone(&host));
    let fresh = support.agent();
    assert_eq!(state_of(&fresh, "win").state, "off");
}

#[test]
fn programs_that_cannot_do_the_job_are_refused_before_anything_starts() {
    let support = Support::new();
    support.machine("win");
    let script = |name: &str, body: &str| {
        let path = support.0.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    };
    let old_host = script("old-host", r#"echo "pvg-host: usage: x --socket P [--window]" >&2; exit 1"#);
    let plain_qemu = script("plain-qemu", r#"echo 'name "apple-gfx-pci"'"#);
    let agent = support.agent();
    let start = |qemu: &Path, host: &Path| {
        request(
            &agent,
            &format!(
                r#"{{"v":1,"id":1,"op":"start","machine":"win","qemu":"{}","pvg_host":"{}"}}"#,
                qemu.display(),
                host.display()
            ),
        )
    };
    let refused = start(Path::new(FAKE_QEMU), &old_host);
    assert!(!refused.ok);
    assert!(refused.error.unwrap().contains("cannot publish the screen"));
    let refused = start(&plain_qemu, Path::new(FAKE_PVG_HOST));
    assert!(!refused.ok);
    assert!(refused.error.unwrap().contains("pvg-fwd"));
    assert_eq!(state_of(&agent, "win").state, "failed");
    assert!(!support.run("win").join("host.pid").exists());
    // Naming the right ones starts it.
    let started = start(Path::new(FAKE_QEMU), Path::new(FAKE_PVG_HOST));
    assert!(started.ok, "{:?}", started.error);
    wait_state(&agent, "win", "running");
    assert!(op(&agent, "force_off", "win").ok);
    wait_state(&agent, "win", "off");
}

fn share_of(agent: &Agent, id: &str) -> Option<guesthouse_core::protocol::ShareInfo> {
    request(agent, r#"{"v":1,"id":1,"op":"list"}"#)
        .shares
        .unwrap()
        .into_iter()
        .find(|share| share.machine == id)
}

#[test]
fn a_machine_with_sharing_on_is_shared_when_it_runs_however_it_was_started_and_a_port_is_chosen() {
    let support = Support::new();
    support.machine("win");
    support.allow_sharing_by_hand("win");
    let rmhost = support.fake_rmhost();
    let agent = support.agent();
    // The start names rmhost, as the app's Start button and `--start` both do.
    let start = format!(r#"{{"v":1,"id":1,"op":"start","machine":"win","rmhost":"{}"}}"#, rmhost.display());
    assert!(request(&agent, &start).ok);
    wait_state(&agent, "win", "running");
    wait_for("the screen to be shared", || {
        share_of(&agent, "win").is_some_and(|s| s.state == ShareState::Listening)
    });
    let share = share_of(&agent, "win").unwrap();
    // The port was chosen then, and written to the machine's file (the stand-in says its own).
    assert_eq!(share.port, Some(7800));
    let file: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(support.0.join("Machines/win/machine.json")).unwrap())
            .unwrap();
    let chosen = file["remote_port"].as_u64().expect("a port written to machine.json");
    assert!((7792..=7899).contains(&chosen), "{chosen}");
    // rmhost has its own input pipe in the run folder, and goes with the machine.
    assert!(support.run("win").join("rmhost.in").exists());
    let pid = pid_of(&support.run("win"), "rmhost.pid");
    assert!(op(&agent, "force_off", "win").ok);
    wait_state(&agent, "win", "off");
    wait_for("rmhost to go", || gone(&pid));
}

#[test]
fn a_sharing_that_cannot_start_says_why_where_the_app_can_see_it() {
    let support = Support::new();
    support.machine("win");
    support.allow_sharing_by_hand("win");
    let agent = support.agent();
    // No rmhost named anywhere.
    assert!(op(&agent, "start", "win").ok);
    wait_state(&agent, "win", "running");
    wait_for("the failure to be shown", || {
        share_of(&agent, "win").is_some_and(|s| s.state == ShareState::Failed)
    });
    assert!(share_of(&agent, "win").unwrap().reason.unwrap().contains("rmhost"));
    assert!(op(&agent, "force_off", "win").ok);
    wait_state(&agent, "win", "off");
}

#[test]
fn a_new_agent_that_finds_a_machine_running_with_sharing_on_shares_it() {
    let support = Support::new();
    support.machine("win");
    let first = support.agent();
    assert!(op(&first, "start", "win").ok);
    wait_state(&first, "win", "running");
    assert!(share_of(&first, "win").is_none());
    // Sharing is turned on by hand while the machine runs, then another agent finds it.
    support.allow_sharing_by_hand("win");
    let rmhost = support.fake_rmhost();
    let second = Agent::new(Config {
        machines: support.0.join("Machines"),
        qemu: Some(PathBuf::from(FAKE_QEMU)),
        rmhost: Some(rmhost),
        pvg_host: Some(PathBuf::from(FAKE_PVG_HOST)),
        install: Default::default(),
        signed: false,
        shutdown_timeout: Duration::from_secs(5),
        start_timeout: Duration::from_secs(10),
    });
    wait_for("the found machine to be shared", || {
        share_of(&second, "win").is_some_and(|s| s.state == ShareState::Listening)
    });
    assert!(op(&second, "force_off", "win").ok);
    wait_state(&second, "win", "off");
}
