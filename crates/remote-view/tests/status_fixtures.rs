//! The files in `status/` are what `ctl status` printed, by hand from the host's source and,
//! beside them, real captures. Every file there is read, so a capture needs no change here.

use guesthouse_remote_view_core::ctl::check_fingerprint;
use guesthouse_remote_view_core::status::{Reply, Status, parse_reply};
use std::fs;
use std::path::PathBuf;

fn folder() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/status")
}

fn read(name: &str) -> Status {
    let text = fs::read_to_string(folder().join(name)).unwrap();
    match parse_reply(&text) {
        Reply::Status(status) => *status,
        other => panic!("{name}: {other:?}"),
    }
}

#[test]
fn every_file_in_the_folder_reads() {
    let mut seen = 0;
    for entry in fs::read_dir(folder()).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let text = fs::read_to_string(&path).unwrap();
        match path.extension().and_then(|extension| extension.to_str()) {
            Some("json") => {
                let Reply::Status(status) = parse_reply(&text) else { panic!("{name} is not a status") };
                for device in &status.devices {
                    assert!(check_fingerprint(&device.fingerprint).is_ok(), "{name}: {}", device.fingerprint);
                }
                if let Some(pending) = &status.pending {
                    assert!(check_fingerprint(&pending.fingerprint).is_ok(), "{name}");
                    assert!(pending.seconds <= 60, "{name}");
                }
                seen += 1;
            }
            Some("txt") if name != "README.txt" => {
                assert!(!matches!(parse_reply(&text), Reply::Status(_)), "{name}");
                seen += 1;
            }
            _ => {}
        }
    }
    assert!(seen >= 20, "the folder has lost files: {seen}");
}

#[test]
fn an_idle_host() {
    let status = read("from-source-idle.json");
    assert!(
        !status.pairing && status.pending.is_none() && status.client.is_none() && status.devices.is_empty()
    );
    assert_eq!(status.fingerprint.as_deref().map(str::len), Some(64));
    assert_eq!((status.clipboard, status.files), (Some(false), Some(false)));
    assert!(!status.paused);
}

#[test]
fn pairing_open() {
    let status = read("from-source-pairing-open.json");
    assert!(status.pairing && status.pending.is_none());
    assert_eq!(status.devices.len(), 1);
}

#[test]
fn a_device_waiting_with_its_code() {
    let status = read("from-source-device-pending.json");
    assert!(!status.pairing);
    let pending = status.pending.clone().unwrap();
    assert_eq!(
        (pending.name.as_str(), pending.code.as_str(), pending.seconds),
        ("Kitchen tablet", "482 913", 47)
    );
    assert!(status.device(&pending.fingerprint).is_none());
}

#[test]
fn a_device_connected() {
    let status = read("from-source-device-connected.json");
    assert_eq!(status.client.unwrap().name, "Kitchen tablet");
    let connected: Vec<_> = status.devices.iter().filter(|device| device.connected).collect();
    assert_eq!(connected.len(), 1);
    assert!(!connected[0].permissions.clipboard && !connected[0].permissions.files);
}

#[test]
fn several_devices_with_mixed_permissions() {
    let status = read("from-source-several-devices-mixed-permissions.json");
    let names: Vec<_> = status.devices.iter().map(|device| device.name.as_str()).collect();
    assert_eq!(names, ["Guesthouse", "Kitchen tablet", "Pat's phone", "Spare laptop"]);
    let phone = &status.devices[2].permissions;
    assert!(!phone.input && !phone.mic && !phone.camera && phone.clipboard && phone.sound);
    assert!(!status.devices[3].permissions.sound);
    assert_eq!((status.clipboard, status.files), (Some(true), Some(true)));
    assert_eq!(status.client.unwrap().name, "Pat's phone");
}

#[test]
fn the_host_stopping() {
    let text = fs::read_to_string(folder().join("from-source-host-stopping.txt")).unwrap();
    assert_eq!(parse_reply(&text), Reply::Stopping);
}

fn captured(name: &str) -> Status {
    read(&format!("captured-{name}.json"))
}

#[test]
fn a_real_host_has_one_name_for_every_device_so_devices_go_by_fingerprint() {
    let status = captured("connected");
    assert_eq!(status.devices.len(), 2);
    assert!(status.devices.iter().all(|device| device.name == "testclient"));
    assert_eq!(status.client.as_ref().unwrap().name, "testclient");
    let connected: Vec<_> = status.devices.iter().filter(|device| device.connected).collect();
    assert_eq!(connected.len(), 1);
    assert!(connected[0].fingerprint.starts_with("44444444"));
}

#[test]
fn a_real_pending_device_has_its_code_and_a_count_down_from_sixty() {
    for (name, code) in [("pending", "585 320"), ("pending-C", "808 279")] {
        let status = captured(name);
        assert!(!status.pairing);
        let pending = status.pending.clone().unwrap();
        assert_eq!((pending.code.as_str(), pending.seconds), (code, 56));
        assert!(status.device(&pending.fingerprint).is_none() || name == "pending-C");
    }
}

#[test]
fn a_real_host_in_each_step_of_a_trial_run() {
    assert!(captured("pairing-open").pairing);
    assert!(!captured("idle").pairing && captured("idle").pending.is_none());
    assert_eq!(captured("after-decline").devices.len(), 2);
    assert!(!captured("after-decline").pairing);
    // A permission change shows in the list, and a revoke takes the device off it.
    let before = captured("connected-A-clipboard-off");
    assert!(!before.devices[0].permissions.clipboard && before.devices[0].connected);
    assert_eq!(captured("after-revoke").devices.len(), 1);
    // Running the installer with one identity left one device, under the name it was given.
    let reinstalled = captured("after-reinstall");
    assert_eq!(reinstalled.devices.len(), 1);
    assert_eq!(reinstalled.devices[0].name, "Guesthouse");
    assert!(!reinstalled.devices[0].permissions.clipboard);
    for name in ["idle", "after-pair-off", "after-drop"] {
        assert_eq!(captured(name).clipboard, Some(false), "{name}");
    }
}

#[test]
fn the_raw_output_with_the_exit_line_after_it_reads_the_same() {
    for name in ["idle", "pending", "connected", "after-reinstall"] {
        assert_eq!(read(&format!("captured-raw-{name}.json")), captured(name), "{name}");
    }
}

#[test]
fn a_device_change_is_answered_with_a_summary_line_and_that_is_done() {
    use guesthouse_remote_view_core::ctl::{Ack, parse_ack};
    let text = fs::read_to_string(folder().join("captured-device-clipboard-off.txt")).unwrap();
    // The capture has the exit code of the program under the answer; the answer is the first line.
    let answer = text.lines().next().unwrap();
    assert!(matches!(parse_ack(answer), Ack::Done(_)), "{answer}");
}

/// Through the relay, a device connected: the host names it as the client. A moment earlier, in the
/// reading where a newly paired device first appears, the host has it connected and no client yet
/// (seen live: the app showed nobody viewing while the list said "Viewing now"). The device's own
/// name stands in then.
#[test]
fn a_connected_device_is_the_viewer_even_before_the_host_names_the_client() {
    use guesthouse_remote_view_core::policy::Policy;
    use guesthouse_remote_view_core::view::viewer;
    let mut status = read("captured-connected-through-relay.json");
    assert_eq!(viewer(&status, &Policy::linux()).as_deref(), Some("Studio laptop"));
    status.client = None;
    assert!(status.devices.iter().any(|device| device.connected));
    assert_eq!(viewer(&status, &Policy::linux()).as_deref(), Some("Studio laptop"));
    for device in &mut status.devices {
        device.connected = false;
    }
    assert_eq!(viewer(&status, &Policy::linux()), None);
}

/// A device at `only` (the spike's capture): the machine's own window then has no picture, and the
/// state says who took it. Through the relay at `extend` it does not.
#[test]
fn a_device_at_only_is_named_as_having_the_screen_to_itself() {
    use guesthouse_remote_view_core::pairing::Pairing;
    use guesthouse_remote_view_core::policy::Policy;
    use guesthouse_remote_view_core::share::ShareState;
    use guesthouse_remote_view_core::view::{Facts, share_info};
    let facts = Facts {
        machine: "m",
        state: ShareState::Listening,
        port: Some(7800),
        fingerprint: None,
        reason: None,
    };
    let pairing = Pairing::new(Policy::linux());
    let mut status = read("captured-connected-through-relay.json");
    let extend = share_info(&facts, Some(&status), &pairing);
    assert_eq!(extend.viewer_placement.as_deref(), Some("extend"));
    assert_eq!(extend.viewing_only(), None);
    let only = read("captured-connected-only.json");
    let name = only.client.as_ref().unwrap().name.clone();
    assert_eq!(share_info(&facts, Some(&only), &pairing).viewing_only(), Some(name.as_str()));
    status.client = None;
    for device in &mut status.devices {
        device.connected = false;
    }
    assert_eq!(share_info(&facts, Some(&status), &pairing).viewing_only(), None);
}
