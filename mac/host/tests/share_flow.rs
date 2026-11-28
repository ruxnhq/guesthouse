// SPDX-License-Identifier: GPL-2.0-or-later
//! Sharing a machine's screen with another device, with no virtual machine: `fake_qemu` stands in
//! for QEMU, `testsource` publishes a moving picture under the name the device would, the agent
//! starts the real `rmhost serve` against it, and `viewcheck` (a viewer with no screen) connects
//! over 127.0.0.1 and counts frames.
//!
//! It needs rmhost, testsource and viewcheck, found as the app finds rmhost (the environment
//! variable GUESTHOUSE_RMHOST, or the lab's build); without them the test says so and passes.
//! The publisher and rmhost must run in one login session, as they do here.
#![cfg(target_os = "macos")]

use guesthouse_core::agent::{Agent, Config};
use guesthouse_core::create::{self, NewMachine};
use guesthouse_core::process;
use guesthouse_core::protocol::{CloseReason, Response, ShareInfo};
use guesthouse_core::remote;
use guesthouse_core::tools::{self, Places, Tool};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const FAKE_QEMU: &str = env!("CARGO_BIN_EXE_fake_qemu");
const WAIT: Duration = Duration::from_secs(20);

/// A child that is ended when the test ends, however it ends.
struct Guard(Child);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Support(PathBuf);

impl Drop for Support {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn request(agent: &Agent, line: &str) -> Value {
    serde_json::from_str(&agent.handle(line)).unwrap()
}

fn shares(agent: &Agent) -> Vec<ShareInfo> {
    let list: Response = serde_json::from_str(&agent.handle(r#"{"v":1,"id":1,"op":"list"}"#)).unwrap();
    list.shares.unwrap()
}

/// A share's state in the word it has on the wire.
fn state_name(state: guesthouse_core::protocol::ShareState) -> String {
    serde_json::to_string(&state).unwrap().trim_matches('"').to_string()
}

fn wait_for<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(found) = check() {
            return found;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(100));
    }
}

/// The share of machine `id` once it is in `state` (and, if given, with pairing so).
fn share_in(agent: &Agent, state: &str, pairing: Option<bool>) -> ShareInfo {
    wait_for(&format!("the share to be {state}"), || {
        shares(agent).into_iter().find(|s| {
            // Pairing false with a device waiting is not closed.
            state_name(s.state) == state
                && pairing.is_none_or(|p| p == s.pairing && (p || s.pending.is_none()))
        })
    })
}

/// Runs viewcheck for `seconds` against the share and returns how many frames it saw, or the
/// error it printed.
fn view(viewcheck: &Path, port: u16, state: &Path, seconds: u32) -> Result<u32, String> {
    let output = Command::new(viewcheck)
        .args([&format!("127.0.0.1:{port}"), &state.display().to_string(), &seconds.to_string()])
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    // The summary line: "viewcheck: 240 frames (1 keyframes) in 4.0 s, ...".
    let frames = out
        .lines()
        .filter_map(|line| line.strip_prefix("viewcheck: "))
        .filter(|line| line.contains(" frames ("))
        .find_map(|line| line.split_whitespace().next()?.parse().ok());
    frames.ok_or(format!("no frame count in: {out}"))
}

/// Runs a viewer in the background, for a device that has to wait for its answer.
fn view_later(
    viewcheck: &Path,
    port: u16,
    state: &Path,
    seconds: u32,
) -> thread::JoinHandle<Result<u32, String>> {
    let (viewcheck, state) = (viewcheck.to_path_buf(), state.to_path_buf());
    thread::spawn(move || view(&viewcheck, port, &state, seconds))
}

/// The share once a device waits to be answered.
fn pending_in(agent: &Agent) -> ShareInfo {
    wait_for("a device to wait", || shares(agent).into_iter().find(|s| s.pending.is_some()))
}

fn ok(agent: &Agent, line: &str) {
    assert_eq!(request(agent, line)["ok"], true, "{line}");
}

#[test]
fn the_agent_shares_a_machine_with_a_viewer_and_stops_when_told() {
    let places = Places::on_this_mac(Tool::Rmhost, None, None);
    let Ok(found) = tools::locate(Tool::Rmhost, &places) else {
        eprintln!("\n*** share_flow SKIPPED: rmhost is not built here (set GUESTHOUSE_RMHOST) ***\n");
        return;
    };
    let rmhost = found.path;
    // An rmhost built before pairing could be opened while running has "nobody is waiting" (its answer to pair accept) nowhere in it.
    let bytes = fs::read(&rmhost).unwrap();
    if !bytes.windows(17).any(|window| window == b"nobody is waiting") {
        eprintln!(
            "\n*** share_flow SKIPPED: this rmhost has no pair accept command; build win/rmhost again ***\n"
        );
        return;
    }
    let beside = |name: &str| rmhost.with_file_name(name);
    let (testsource, viewcheck) = (beside("testsource"), beside("viewcheck"));
    if !testsource.is_file() || !viewcheck.is_file() {
        eprintln!("\n*** share_flow SKIPPED: testsource or viewcheck is not built beside rmhost ***\n");
        return;
    }

    // A machine called sh, with the screen sharing allowed.
    let root = std::env::temp_dir().join(format!("gh-share-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let support = Support(root.clone());
    let machines = root.join("Machines");
    fs::create_dir_all(&machines).unwrap();
    let file = |name: &str| {
        let path = root.join(name);
        fs::write(&path, name).unwrap();
        path
    };
    let new = NewMachine {
        name: "sh".into(),
        cores: 2,
        memory_mb: 4096,
        disk: file("d.img"),
        firmware_code: file("c.fd"),
        firmware_vars: file("v.fd"),
        option_rom: None,
        host_memory_mb: 16384,
        host_cores: 8,
    };
    assert_eq!(create::create(&machines, &new).unwrap(), "sh");
    // This test is about sharing, not about where the device lives.
    create::set_graphics(&machines, "sh", "qemu").unwrap();
    let port = remote::set_enabled(&machines, "sh", true).unwrap().unwrap();

    // The device's frames, under the name the machine's command line gives them.
    let _source = Guard(
        Command::new(&testsource)
            .args(["-n", "guesthouse.sh"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    thread::sleep(Duration::from_millis(500));

    let agent = Agent::new(Config {
        machines: machines.clone(),
        qemu: Some(PathBuf::from(FAKE_QEMU)),
        rmhost: None,
        pvg_host: None,
        install: Default::default(),
        shutdown_timeout: Duration::from_secs(5),
        start_timeout: Duration::from_secs(10),
    });
    // Nothing listens before a machine runs, whatever the setting.
    assert!(shares(&agent).is_empty());

    // Starting the machine starts the share, since it is allowed.
    let start = format!(r#"{{"v":1,"id":2,"op":"start","machine":"sh","rmhost":"{}"}}"#, rmhost.display());
    assert_eq!(request(&agent, &start)["ok"], true);
    let listening = share_in(&agent, "listening", Some(false));
    assert_eq!(listening.port, Some(port));
    assert!(listening.fingerprint.as_deref().is_some_and(|f| f.len() >= 16));
    let run = root.join("Run/sh");
    let pid = process::read_file(&run.join(remote::PID_FILE)).expect("a pid file for rmhost");
    assert!(process::is_running(&pid));
    // The viewer's input has a QMP socket of its own, not the app's.
    let args = fs::read_to_string(run.join("fake-qemu.args")).unwrap();
    assert!(args.contains("remote.sock") && args.contains("input.sock"));

    // A device nobody knows is turned away while pairing is closed.
    let state = |name: &str| root.join(name);
    assert!(view(&viewcheck, port, &state("v1"), 2).is_err(), "an unpaired device got in");

    // The lines the agent sends, as the app does.
    let open = |seconds: u64| {
        format!(
            r#"{{"v":1,"id":3,"op":"share","machine":"sh","on":true,"pairing":true,"pairing_seconds":{seconds},"rmhost":"{}"}}"#,
            rmhost.display()
        )
    };
    let say = |op: &str| format!(r#"{{"v":1,"id":3,"op":"{op}","machine":"sh"}}"#);
    let same_rmhost = || process::read_file(&run.join(remote::PID_FILE)) == Some(pid.clone());

    // Pairing asks first. The device is held and gets no picture until it is accepted.
    ok(&agent, &open(30));
    let opened = share_in(&agent, "listening", Some(true));
    assert!(opened.pairing_until_ms.is_some() && opened.pending.is_none());
    assert_eq!(opened.fingerprint, listening.fingerprint, "the host keeps its identity");
    let first = view_later(&viewcheck, port, &state("v1"), 3);
    let waiting = pending_in(&agent);
    let device = waiting.pending.clone().unwrap();
    assert_eq!(device.name, "viewcheck");
    // "123 456": two groups of three digits, as the device shows them.
    let digits: Vec<&str> = device.code.split(' ').collect();
    assert!(
        digits.len() == 2 && digits.iter().all(|d| d.len() == 3 && d.bytes().all(|b| b.is_ascii_digit()))
    );
    assert!(device.seconds > 50 && device.seconds <= 60);
    assert!(!waiting.pairing, "waiting is not open, and not closed either");
    assert!(waiting.closed_reason.is_none() && waiting.viewer.is_none(), "it was let in unasked");
    ok(&agent, &say("pair_accept"));
    let closed = share_in(&agent, "listening", Some(false));
    assert_eq!(closed.paired.as_deref(), Some("viewcheck"));
    assert_eq!(closed.closed_reason, Some(CloseReason::Paired));
    assert!(closed.pending.is_none());
    let frames = first.join().unwrap().expect("the accepted device connects");
    assert!(frames > 30, "only {frames} frames after accepting");
    assert!(same_rmhost(), "pairing restarted rmhost");
    // The accepted device is listed, with clipboard and files off until the person turns them on.
    let listed = wait_for("the device list", || shares(&agent).into_iter().find(|s| !s.devices.is_empty()));
    let me = &listed.devices[0];
    assert_eq!(me.name, "viewcheck");
    assert!(me.permissions.input && me.permissions.sound);
    assert!(!me.permissions.clipboard && !me.permissions.files, "clipboard and files start on");
    let fingerprint = me.fingerprint.clone();

    // Declined: refused, and not trusted afterwards.
    ok(&agent, &open(30));
    let second = view_later(&viewcheck, port, &state("v2"), 2);
    pending_in(&agent);
    ok(&agent, &say("pair_decline"));
    let declined = share_in(&agent, "listening", Some(false));
    assert_eq!(declined.closed_reason, Some(CloseReason::Declined));
    assert!(declined.paired.is_none(), "paired is cleared when pairing opens");
    let error = second.join().unwrap().unwrap_err();
    assert!(error.contains("declined"), "{error}");
    assert!(view(&viewcheck, port, &state("v2"), 2).is_err(), "a declined device got in later");

    // The pairing time does not run out under a waiting device.
    ok(&agent, &open(2));
    let third = view_later(&viewcheck, port, &state("v3"), 3);
    pending_in(&agent);
    thread::sleep(Duration::from_secs(4));
    let still = pending_in(&agent);
    assert!(still.closed_reason.is_none(), "pairing closed under a waiting device");
    ok(&agent, &say("pair_accept"));
    share_in(&agent, "listening", Some(false));
    assert!(third.join().unwrap().expect("accepted late") > 20);

    // A device already paired comes straight in while another waits; the core serves one
    // viewer at a time, so the one that waited is cut off and its pairing closes as "other".
    ok(&agent, &open(30));
    let fourth = view_later(&viewcheck, port, &state("v4"), 4);
    pending_in(&agent);
    let known = view(&viewcheck, port, &state("v1"), 2).expect("a paired device is let straight in");
    assert!(known > 20, "only {known} frames for the paired device");
    assert!(fourth.join().unwrap().is_err(), "the waiting device was not cut off");
    assert_eq!(share_in(&agent, "listening", Some(false)).closed_reason, Some(CloseReason::Other));

    // Pairing closes at the end of its time with nobody coming, and when asked.
    ok(&agent, &open(2));
    share_in(&agent, "listening", Some(true));
    assert_eq!(share_in(&agent, "listening", Some(false)).closed_reason, Some(CloseReason::Timeout));
    ok(&agent, &open(30));
    share_in(&agent, "listening", Some(true));
    ok(&agent, &say("pair_close"));
    assert_eq!(share_in(&agent, "listening", Some(false)).closed_reason, Some(CloseReason::Asked));
    assert!(view(&viewcheck, port, &state("v5"), 2).is_err(), "a device got in with pairing closed");

    // A time outside what rmhost takes is refused with a sentence, not passed on.
    for seconds in [0, 3601] {
        let refused = request(&agent, &open(seconds));
        assert_eq!(refused["ok"], false);
        assert!(refused["error"].as_str().unwrap().contains("1 to 3600"));
    }

    // Per-device controls: view-only, a permission, disconnect, forget.
    let device = |op: &str, extra: &str| {
        format!(r#"{{"v":1,"id":4,"op":"{op}","machine":"sh","fingerprint":"{fingerprint}"{extra}}}"#)
    };
    let find =
        |agent: &Agent| shares(agent)[0].devices.iter().find(|d| d.fingerprint == fingerprint).cloned();
    ok(&agent, &device("device_allow", r#","what":"input","on":false"#));
    wait_for("view-only", || find(&agent).filter(|d| !d.permissions.input));
    ok(&agent, &device("device_allow", r#","what":"clipboard","on":true"#));
    wait_for("clipboard on", || find(&agent).filter(|d| d.permissions.clipboard));
    let refused = request(&agent, &device("device_allow", r#","what":"root","on":true"#));
    assert_eq!(refused["ok"], false);
    let refused = request(
        &agent,
        &format!(
            r#"{{"v":1,"id":4,"op":"device_revoke","machine":"sh","fingerprint":"{fingerprint}\npair open 5"}}"#
        ),
    );
    assert_eq!(refused["ok"], false, "a second command got through on the fingerprint");

    let watching = view_later(&viewcheck, port, &state("v1"), 9);
    wait_for("the viewer to connect", || find(&agent).filter(|d| d.connected));
    ok(&agent, &say("device_drop"));
    wait_for("the viewer to be dropped", || find(&agent).filter(|d| !d.connected));
    // viewcheck waits out its nine seconds either way; a dropped one got only the frames
    // before the drop, far fewer than nine seconds of 60 a second.
    if let Ok(frames) = watching.join().unwrap() {
        assert!(frames < 400, "the viewer was not disconnected: {frames} frames");
    }
    assert!(find(&agent).is_some(), "disconnecting made the device forgotten");

    ok(&agent, &device("device_revoke", ""));
    wait_for("the device to be forgotten", || find(&agent).is_none().then_some(()));
    assert!(view(&viewcheck, port, &state("v1"), 2).is_err(), "a removed device got in");
    assert!(same_rmhost(), "rmhost was restarted along the way");

    // Turning the share off ends rmhost, and nothing is left listening.
    let pid = process::read_file(&run.join(remote::PID_FILE)).unwrap();
    assert_eq!(request(&agent, r#"{"v":1,"id":4,"op":"share","machine":"sh","on":false}"#)["ok"], true);
    assert!(shares(&agent).is_empty());
    wait_for("rmhost to end", || (!process::is_running(&pid)).then_some(()));
    assert!(!run.join(remote::PID_FILE).exists());
    assert!(view(&viewcheck, port, &state("v1"), 2).is_err(), "something still listens");

    // On again, then the machine is forced off: rmhost goes with it.
    let plain =
        format!(r#"{{"v":1,"id":9,"op":"share","machine":"sh","on":true,"rmhost":"{}"}}"#, rmhost.display());
    assert_eq!(request(&agent, &plain)["ok"], true);
    share_in(&agent, "listening", Some(false));
    let pid = process::read_file(&run.join(remote::PID_FILE)).unwrap();
    assert_eq!(request(&agent, r#"{"v":1,"id":5,"op":"force_off","machine":"sh"}"#)["ok"], true);
    wait_for("rmhost to end with the machine", || (!process::is_running(&pid)).then_some(()));
    assert!(shares(&agent).is_empty());

    // Switched off in the machine's settings: starting the machine shares nothing.
    remote::set_enabled(&machines, "sh", false).unwrap();
    wait_for("the machine to stop", || {
        let list = request(&agent, r#"{"v":1,"id":6,"op":"list"}"#);
        (list["machines"][0]["state"] == "off").then_some(())
    });
    assert_eq!(request(&agent, &start)["ok"], true);
    wait_for("the machine to run", || {
        let list = request(&agent, r#"{"v":1,"id":7,"op":"list"}"#);
        (list["machines"][0]["state"] == "running").then_some(())
    });
    thread::sleep(Duration::from_secs(1));
    assert!(shares(&agent).is_empty());
    let refused = request(&agent, &plain);
    assert_eq!(refused["ok"], false);
    assert!(refused["error"].as_str().unwrap().contains("off"));
    request(&agent, r#"{"v":1,"id":8,"op":"force_off","machine":"sh"}"#);
    drop(support);
    eprintln!("share_flow: RAN against {}", rmhost.display());
}
