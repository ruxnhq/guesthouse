// SPDX-License-Identifier: GPL-2.0-or-later
//! A Linux machine on the guarded path with stand-ins: its QEMU line has the network and the
//! scale the start asked for, and the size the app wants reaches the graphics process as its
//! mode while the machine runs, through the C function the app calls. QEMU's own device and a
//! stopped machine are told nothing, and a refusal is said.
#![cfg(target_os = "macos")]

use guesthouse_core::agent::{Agent, Config};
use guesthouse_core::guesthouse_string_free;
use guesthouse_core::machines_ffi::guesthouse_machine_set_wanted_screen;
use guesthouse_core::protocol::Response;
use std::ffi::{CStr, CString};
use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

const FAKE_QEMU: &str = env!("CARGO_BIN_EXE_fake_qemu");
const FAKE_PVG_HOST: &str = env!("CARGO_BIN_EXE_fake_pvg_host");
/// The longest anything here waits for the agent or a stand-in. A start takes well under a
/// second; a fault fails after this, saying what it waited for.
const WAIT: Duration = Duration::from_secs(15);

struct Support(PathBuf);

impl Drop for Support {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

impl Support {
    fn new(name: &str) -> Support {
        let path = std::env::temp_dir().join(format!("ghz-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(path.join("Machines")).unwrap();
        Support(path)
    }

    fn machine(&self, id: &str, kind: &str, graphics: &str) {
        let folder = self.0.join("Machines").join(id);
        fs::create_dir_all(&folder).unwrap();
        for file in ["code.fd", "vars.fd", "disk.img"] {
            fs::write(folder.join(file), b"").unwrap();
        }
        let json = format!(
            r#"{{"name": "{id}", "kind": "{kind}", "cores": 2, "memory_mb": 512, "graphics": "{graphics}",
                "disk": "disk.img", "firmware_code": "code.fd", "firmware_vars": "vars.fd",
                "modes": ["1920x1080@60", "1280x800@60"]}}"#
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

    fn run_file(&self, id: &str, file: &str) -> String {
        fs::read_to_string(self.0.join("Run").join(id).join(file)).unwrap_or_default()
    }
}

/// Runs `work` on a thread of its own and waits `WAIT` for its answer, so a call that blocks
/// fails the test with a sentence instead of holding it for good. The thread is left behind.
fn within<T: Send + 'static>(what: &str, work: impl FnOnce() -> T + Send + 'static) -> T {
    let (sender, receiver) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(work());
    });
    receiver
        .recv_timeout(WAIT)
        .unwrap_or_else(|_| panic!("{what} did not answer in {} seconds", WAIT.as_secs()))
}

fn request(agent: &Agent, text: &str) -> Response {
    let (agent, line) = (agent.clone(), text.to_string());
    let answer = within(&format!("the agent, asked {text}"), move || agent.handle(&line));
    serde_json::from_str(&answer).unwrap()
}

fn wait_state(agent: &Agent, id: &str, state: &str) {
    let deadline = Instant::now() + WAIT;
    loop {
        let list = request(agent, r#"{"v":1,"id":1,"op":"list"}"#).machines.unwrap();
        if list.iter().any(|s| s.machine == id && s.state == state) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{id} was not {state} in {} seconds; it is {:?}",
            WAIT.as_secs(),
            list.iter().find(|s| s.machine == id).map(|s| &s.state)
        );
        thread::sleep(Duration::from_millis(50));
    }
}

/// The QEMU line's arguments, one a line, as the stand-in wrote them.
fn has_pair(args: &str, first: &str, second: &str) -> bool {
    let args: Vec<&str> = args.lines().collect();
    args.windows(2).any(|pair| pair == [first, second])
}

#[test]
fn a_linux_machine_starts_with_a_network_and_the_scale_it_was_given() {
    let support = Support::new("start");
    support.machine("deb", "linux", "guarded");
    support.machine("std", "linux", "guarded");
    support.machine("win", "windows", "guarded");
    let agent = support.agent();
    let start = |id: &str, scale: &str| {
        request(&agent, &format!(r#"{{"v":1,"id":1,"op":"start","machine":"{id}"{scale}}}"#))
    };
    assert!(start("deb", "").ok);
    assert!(start("std", r#","scale":1"#).ok);
    assert!(start("win", r#","scale":1"#).ok);
    for id in ["deb", "std", "win"] {
        wait_state(&agent, id, "running");
    }
    let deb = support.run_file("deb", "fake-qemu.args");
    assert!(has_pair(&deb, "-netdev", "user,id=net,host-loopback=off"));
    assert!(has_pair(&deb, "-device", "virtio-net-pci,netdev=net,romfile="));
    assert!(has_pair(&deb, "-fw_cfg", "name=opt/com.ruxn.guesthouse/scale,string=2"));
    let std = support.run_file("std", "fake-qemu.args");
    assert!(has_pair(&std, "-fw_cfg", "name=opt/com.ruxn.guesthouse/scale,string=1"));
    let win = support.run_file("win", "fake-qemu.args");
    assert!(has_pair(&win, "-nic", "none"));
    assert!(!win.contains("-fw_cfg"));
    // A scale no guest takes is refused before anything starts.
    support.machine("odd", "linux", "guarded");
    let odd = start("odd", r#","scale":3"#);
    assert!(!odd.ok && odd.error.unwrap().contains("not a scale"));
    for id in ["deb", "std", "win"] {
        let off = format!(r#"{{"v":1,"id":2,"op":"force_off","machine":"{id}"}}"#);
        assert!(request(&agent, &off).ok);
        wait_state(&agent, id, "off");
    }
}

/// What the app's C function answers for machine `id` wanting `width` by `height`.
fn want(id: &str, width: u32, height: u32) -> serde_json::Value {
    let name = id.to_string();
    let id = CString::new(id).unwrap();
    within(&format!("the screen size wanted for {name}"), move || {
        // SAFETY: a nul-terminated string; the answer is the core's and is freed by it.
        unsafe {
            let text = guesthouse_machine_set_wanted_screen(id.as_ptr(), width, height, 200);
            let value = serde_json::from_str(CStr::from_ptr(text).to_str().unwrap()).unwrap();
            guesthouse_string_free(text);
            value
        }
    })
}

#[test]
fn the_wanted_size_reaches_a_running_guarded_machine_and_nothing_else() {
    let support = Support::new("size");
    // The only test in this program that reads the environment.
    unsafe { std::env::set_var("GUESTHOUSE_SUPPORT_DIR", &support.0) };
    support.machine("deb", "linux", "guarded");
    support.machine("q", "linux", "qemu");
    support.machine("hostnomodes", "linux", "guarded");
    let agent = support.agent();

    let stopped = want("deb", 2560, 1440);
    assert_eq!(stopped["applied"], false, "{stopped}");
    assert!(stopped["why"].as_str().unwrap().contains("not running"));

    for id in ["deb", "q", "hostnomodes"] {
        let start = format!(r#"{{"v":1,"id":1,"op":"start","machine":"{id}"}}"#);
        assert!(request(&agent, &start).ok);
        wait_state(&agent, id, "running");
    }
    let applied = want("deb", 2560, 1440);
    assert_eq!(applied["applied"], true, "{applied}");
    assert_eq!(applied["mode"], "2560x1440@60");
    assert_eq!(support.run_file("deb", "fake-pvg-host.modes"), "2560x1440@60\n");

    let qemu = want("q", 2560, 1440);
    assert_eq!(qemu["applied"], false, "{qemu}");
    assert!(qemu["why"].as_str().unwrap().contains("QEMU's own device"));

    let refused = want("hostnomodes", 1280, 800);
    assert!(refused["error"].as_str().unwrap().contains("refused"), "{refused}");

    for id in ["deb", "q", "hostnomodes"] {
        let off = format!(r#"{{"v":1,"id":2,"op":"force_off","machine":"{id}"}}"#);
        assert!(request(&agent, &off).ok);
        wait_state(&agent, id, "off");
    }
    // A second after the last one, and the machine is off again.
    let after = want("deb", 1280, 800);
    assert_eq!(after["applied"], false, "{after}");
}
