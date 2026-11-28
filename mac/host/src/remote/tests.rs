// SPDX-License-Identifier: GPL-2.0-or-later
use super::*;
use crate::create::{self, NewMachine};

fn launch() -> Launch {
    Launch {
        kind: "windows".into(),
        scale: crate::launch::DEFAULT_SCALE,
        keyboard: None,
        cores: 2,
        memory_mb: 4096,
        disk: "/m/w/disk.img".into(),
        firmware_code: "/m/w/code.fd".into(),
        firmware_vars: "/m/w/vars.fd".into(),
        stick: None,
        installer: None,
        answer_disc: None,
        installer_boot: None,
        option_rom: None,
        modes: vec![],
        frame_name: "guesthouse.w".into(),
        run_dir: "/s/Run/w".into(),
        control_socket: "/s/Run/w/qmp.sock".into(),
        input_socket: "/s/Run/w/input.sock".into(),
        remote_socket: "/s/Run/w/remote.sock".into(),
        graphics: crate::launch::Graphics::Qemu,
        host_socket: "/s/Run/w/host.sock".into(),
        host_control: "/s/Run/w/control.sock".into(),
    }
}

#[test]
fn serve_gets_its_own_socket_port_and_state_and_pairs_only_when_asked() {
    let closed = serve_args(&launch(), Path::new("/m/w"), "Windows 11", 7792);
    assert_eq!(
        closed,
        [
            "serve",
            "-q",
            "/s/Run/w/remote.sock",
            "-n",
            "guesthouse.w",
            "-l",
            "0.0.0.0:7792",
            "-s",
            "/m/w/remote-view",
            "-N",
            "Windows 11"
        ]
    );
    // Never the app's own input socket.
    assert!(!closed.iter().any(|arg| arg.contains("input.sock")));
    // Pairing is never on the command line: it is opened by a line on rmhost's input.
    assert!(!closed.contains(&"-p".to_string()));
    assert_eq!(pair_open_line(120), "pair open 120\n");
}

#[test]
fn reads_the_lines_serve_prints() {
    let line = "rmhost: remoteman host \"Windows 11\" listening: 0.0.0.0:7792\tabc123def (address, fingerprint), pairing on";
    assert_eq!(
        parse_line(line),
        Some(LogEvent::Listening { port: 7792, fingerprint: "abc123def".into(), pairing: true })
    );
    let off = line.replace("pairing on", "pairing off");
    assert!(matches!(parse_line(&off), Some(LogEvent::Listening { pairing: false, .. })));
    assert_eq!(
        parse_line("rmhost: viewer \"Pat's iPad\" 2388x1668 at 60 Hz, decoders hevc"),
        Some(LogEvent::Viewer("Pat's iPad".into()))
    );
    assert_eq!(parse_line("rmhost: viewer left"), Some(LogEvent::ViewerLeft));
    assert_eq!(parse_line("rmhost: input goes to /s/remote.sock"), None);
    assert_eq!(parse_line("rmhost: refused: this host sends HEVC only"), None);
    assert_eq!(parse_line(""), None);
}

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn machines_with(names: &[&str]) -> (Scratch, PathBuf) {
    // One folder for each call, so tests running side by side never share one.
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let number = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("gh-remote-{}-{number}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let machines = root.join("Machines");
    fs::create_dir_all(&machines).unwrap();
    for name in names {
        let source = |file: &str| {
            let path = root.join(format!("{name}-{file}"));
            fs::write(&path, file).unwrap();
            path
        };
        let request = NewMachine {
            name: name.to_string(),
            cores: 2,
            memory_mb: 4096,
            disk: source("disk"),
            firmware_code: source("code"),
            firmware_vars: source("vars"),
            option_rom: None,
            host_memory_mb: 16384,
            host_cores: 8,
        };
        create::create(&machines, &request).unwrap();
    }
    (Scratch(root), machines)
}

#[test]
fn a_new_machine_does_not_listen_and_has_no_port() {
    let (_scratch, machines) = machines_with(&["a"]);
    let machine = store::find(&machines, "a").unwrap();
    assert!(!machine.remote_view);
    assert_eq!(machine.remote_port, None);
}

#[test]
fn turning_it_on_picks_a_port_and_keeps_it_and_turning_it_off_keeps_the_port() {
    let (_scratch, machines) = machines_with(&["a", "b"]);
    let first = set_enabled(&machines, "a", true).unwrap().unwrap();
    assert!((FIRST_PORT..=LAST_PORT).contains(&first));
    let second = set_enabled(&machines, "b", true).unwrap().unwrap();
    assert_ne!(first, second, "each machine has a port of its own");
    let machine = store::find(&machines, "a").unwrap();
    assert!(machine.remote_view);
    assert_eq!(machine.remote_port, Some(first));
    assert_eq!(set_enabled(&machines, "a", false).unwrap(), Some(first));
    let machine = store::find(&machines, "a").unwrap();
    assert!(!machine.remote_view);
    assert_eq!(machine.remote_port, Some(first));
    // On again: the same port.
    assert_eq!(set_enabled(&machines, "a", true).unwrap(), Some(first));
    // The machine's other settings are as they were.
    assert_eq!(machine.cores, 2);
    assert!(machine.startable);
}

#[test]
fn ports_in_use_are_skipped_and_running_out_is_said() {
    let (_scratch, machines) = machines_with(&["a"]);
    assert_eq!(choose_port(&machines, |port| port != FIRST_PORT), Some(FIRST_PORT + 1));
    assert_eq!(choose_port(&machines, |_| false), None);
}

/// A device's fingerprint, as long as a real one.
const FP: &str = "abababababababababababababababababababababababababababababababab";

#[test]
fn device_lines_are_the_crates_words_and_only_from_a_fingerprint_and_a_known_permission() {
    use guesthouse_remote_view_core::ctl;
    use guesthouse_remote_view_core::policy::Policy;
    let policy = Policy::default();
    let line = |words: Result<Vec<String>, String>| rmhost_line(&words.unwrap()).unwrap();
    assert_eq!(line(policy.device_allow(FP, "clipboard", true)), format!("device {FP} allow clipboard on\n"));
    // Control is the opposite of view-only.
    assert_eq!(line(policy.device_allow(FP, "input", false)), format!("device {FP} view-only on\n"));
    assert_eq!(line(policy.device_allow(FP, "input", true)), format!("device {FP} view-only off\n"));
    assert!(policy.device_allow(FP, "root", true).is_err());
    assert!(policy.device_allow(&format!("{FP}\npair open 5"), "files", true).is_err());
    assert!(policy.device_allow("", "files", true).is_err());
    assert_eq!(line(policy.device_revoke(FP)), format!("revoke {FP}\n"));
    assert_eq!(rmhost_line(&ctl::device_drop()).unwrap(), "drop\n");
    assert_eq!(rmhost_line(&ctl::pair_accept(FP).unwrap()).unwrap(), format!("pair accept {FP}\n"));
    assert_eq!(rmhost_line(&ctl::pair_decline(FP).unwrap()).unwrap(), format!("pair decline {FP}\n"));
    // A short one is not a fingerprint: the real ones are 64 hex digits.
    assert!(policy.device_revoke("ab12").is_err());
    assert!(policy.device_revoke("zz").is_err());
}

fn words(line: &str) -> Vec<String> {
    line.split(' ').map(str::to_owned).collect()
}

#[test]
fn the_pairings_words_become_the_lines_rmhost_reads() {
    assert_eq!(rmhost_line(&words("pair on 120")).unwrap(), "pair open 120\n");
    assert_eq!(rmhost_line(&words("pair off")).unwrap(), "pair close\n");
    let fingerprint = "ab".repeat(32);
    assert_eq!(
        rmhost_line(&words(&format!("pair accept {fingerprint}"))).unwrap(),
        format!("pair accept {fingerprint}\n")
    );
    assert_eq!(
        rmhost_line(&words(&format!("pair decline {fingerprint}"))).unwrap(),
        format!("pair decline {fingerprint}\n")
    );
    // A device is named for an answer: rmhost is never asked to answer whoever waits.
    assert!(rmhost_line(&words("pair accept")).is_err());
    assert!(rmhost_line(&words("pair decline")).is_err());
    assert!(rmhost_line(&words("pair accept Tablet")).is_err());
    assert_eq!(rmhost_line(&words("drop")).unwrap(), "drop\n");
    assert_eq!(
        rmhost_line(&words(&format!("device {fingerprint} view-only on"))).unwrap(),
        format!("device {fingerprint} view-only on\n")
    );
}

#[test]
fn a_word_rmhost_has_no_line_for_is_not_delivered_with_a_reason() {
    for line in ["sharing on", "allow clipboard off", "set updates off", "status", "pair on soon"] {
        assert!(rmhost_line(&words(line)).is_err(), "{line}");
    }
}

#[test]
fn what_rmhost_says_of_pairing_is_the_shared_pairings_event_in_lower_case() {
    use guesthouse_remote_view_core::events::PairingEvent;
    let device =
        Pending { name: "Phone".into(), fingerprint: "AB:CD".into(), code: "123 456".into(), seconds: 60 };
    assert_eq!(
        pairing_event(&LogEvent::PairingPending(device)),
        Some(PairingEvent::DeviceWaits {
            name: "Phone".into(),
            fingerprint: "ab:cd".into(),
            code: "123 456".into(),
            seconds: 60
        })
    );
    assert_eq!(
        pairing_event(&LogEvent::Paired { fingerprint: "AB".into(), name: "Phone".into() }),
        Some(PairingEvent::Paired { fingerprint: "ab".into(), name: "Phone".into() })
    );
    assert_eq!(
        pairing_event(&LogEvent::PairingOpen { until_secs: 1_800_000_000 }),
        Some(PairingEvent::PairingOpen { until_unix_seconds: 1_800_000_000 })
    );
    // No time limit is rmhost's lab flag, and the shared pairing takes 0 as ended: not fed.
    assert_eq!(pairing_event(&LogEvent::PairingOpen { until_secs: 0 }), None);
    assert_eq!(pairing_event(&LogEvent::ViewerLeft), None);
}
