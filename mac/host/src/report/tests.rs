// SPDX-License-Identifier: GPL-2.0-or-later
use super::*;
use crate::report::bundle::prepare;
use crate::report::collect::{Collection, Places};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("gh-report-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A support folder with one machine that has run, and a home folder.
fn world(name: &str) -> (PathBuf, Places) {
    let root = scratch(name);
    let support = root.join("Library/Application Support/Guesthouse");
    let machine = support.join("Machines/windows-11");
    let run = support.join("Run/windows-11");
    fs::create_dir_all(&machine).unwrap();
    fs::create_dir_all(&run).unwrap();
    fs::create_dir_all(machine.join("remote-view")).unwrap();
    fs::write(
        machine.join("machine.json"),
        r#"{"name":"Quill's Windows","kind":"windows","cores":4,"password":"hunter2"}"#,
    )
    .unwrap();
    fs::write(
        machine.join("install.json"),
        r#"{"machine":"windows-11","guest":"windows","state":"done","account":"quill"}"#,
    )
    .unwrap();
    fs::write(machine.join("remote-view/identity.key"), "PRIVATE KEY MATERIAL").unwrap();
    fs::write(machine.join("answers.iso"), "k7mn-p3xq-ab9d-w4ht").unwrap();
    fs::write(run.join("qemu.log"), format!("started {}/Library/x password=hunter2\n", root.display()))
        .unwrap();
    fs::write(run.join("serial.log"), "guesthouse: system installed\n").unwrap();
    fs::write(run.join("rmhost.log"), format!("rmhost: paired {} Phone\n", "9f2c41ab".repeat(8))).unwrap();
    fs::write(support.join("agent.log"), "agent up\n").unwrap();
    let home = root.clone();
    (root, Places { support, home })
}

fn request() -> Request {
    Request {
        machine: Some("windows-11".into()),
        kind: "wont-start".into(),
        description: "It went black. k7mn-p3xq-ab9d-w4ht".into(),
        contact: "on the Discord".into(),
        include_serial: true,
        machine_running: false,
        app_version: "0.1.0".into(),
        driver_version: None,
    }
}

fn collection_for(request: &Request, places: &Places) -> Collection {
    // Only what the files give: the system's own log and crash reports are not asked for here.
    let mut found = Collection::default();
    let run = places.support.join("Run/windows-11");
    for (name, file) in [
        ("machine/qemu.log", "qemu.log"),
        ("machine/serial.log", "serial.log"),
        ("machine/rmhost.log", "rmhost.log"),
    ] {
        found.add(name, fs::read_to_string(run.join(file)).unwrap());
        found.has_run_logs = true;
    }
    found.add(
        "machine/machine.json",
        collect::without_private_fields(
            &fs::read_to_string(places.support.join("Machines/windows-11/machine.json")).unwrap(),
        ),
    );
    found
        .add("host/facts.txt", format!("Guesthouse: {}\nChip: Apple M5\nmacOS: 27.2\n", request.app_version));
    found
}

#[test]
fn the_report_holds_the_parts_with_the_macs_names_and_secrets_taken_out() {
    let (root, places) = world("bundle");
    let user = std::env::var("USER").unwrap_or_default();
    let found = collection_for(&request(), &places);
    let prepared = prepare(&request(), &found, &places).unwrap();
    let read = |name: &str| fs::read_to_string(prepared.folder.join(name)).unwrap();
    let all: String = prepared.files.iter().map(|(name, _)| read(name)).collect();
    // Nothing that was to be taken out is left anywhere in it.
    for secret in ["hunter2", "k7mn-p3xq-ab9d-w4ht", &"9f2c41ab".repeat(8), root.to_str().unwrap(), "Quill"] {
        assert!(!all.contains(secret), "{secret} is still in the report:\n{all}");
    }
    if user.len() >= 3 {
        assert!(!all.contains(&user) || user == "root");
    }
    assert!(read("machine/qemu.log").contains("password=<hidden>"));
    assert!(read("machine/rmhost.log").contains("<fingerprint> Phone"));
    assert!(read("machine/machine.json").contains("<hidden>"), "the private field is not left in");
    // The manifest says who reads it and for how long, and what is not in it.
    let manifest = read("manifest.txt");
    assert!(manifest.contains("A Guesthouse bug report from a Mac."));
    assert!(manifest.contains("deleted after 30 days"));
    assert!(manifest.contains("Never in it") || prepared.notes.is_empty());
    // The sort facts are the Mac's, never the machine's name.
    assert_eq!(prepared.summary.get("host"), Some("macos"));
    assert_eq!(prepared.summary.get("kind"), Some("wont-start"));
    assert_eq!(prepared.summary.get("card"), Some("Apple M5"));
    assert_eq!(prepared.summary.get("os"), Some("macOS 27.2"));
    assert!(!prepared.summary.to_json().contains("Quill"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn the_archive_is_a_tar_gz_of_the_same_files() {
    let (root, places) = world("archive");
    let found = collection_for(&request(), &places);
    let prepared = prepare(&request(), &found, &places).unwrap();
    let listing = Command::new("tar").arg("-tzf").arg(&prepared.archive).output().unwrap();
    let listing = String::from_utf8_lossy(&listing.stdout).into_owned();
    for wanted in ["machine/qemu.log", "description.txt", "summary.json", "manifest.txt"] {
        assert!(listing.lines().any(|line| line.ends_with(wanted)), "{wanted} not in {listing}");
    }
    // What is never gathered is not in it, so cannot be: the keys, the answer disc.
    assert!(!listing.contains("identity.key") && !listing.contains("answers.iso"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn only_the_newest_three_reports_are_kept() {
    let (root, places) = world("kept");
    let found = collection_for(&request(), &places);
    let reports = places.support.join("Reports");
    fs::create_dir_all(reports.join("guesthouse-report-20200101-000001")).unwrap();
    fs::create_dir_all(reports.join("guesthouse-report-20200101-000002")).unwrap();
    fs::create_dir_all(reports.join("guesthouse-report-20200101-000003")).unwrap();
    prepare(&request(), &found, &places).unwrap();
    let folders: Vec<_> = fs::read_dir(&reports).unwrap().flatten().filter(|e| e.path().is_dir()).collect();
    assert_eq!(folders.len(), 3);
    assert!(!reports.join("guesthouse-report-20200101-000001").exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn what_a_machine_that_is_not_running_is_told_is_the_linux_apps_sentence() {
    let wont = ProblemKind::WontStart;
    assert!(before_sending(false, false, false, wont).is_none(), "a report about the app needs nothing");
    let none_running = before_sending(true, false, false, ProblemKind::Crashed).unwrap();
    assert!(
        none_running
            .starts_with("This report has no logs of the machine running, and logs from inside the guest")
    );
    assert!(
        before_sending(true, false, false, wont).unwrap().contains("If it won't start at all, send anyway.")
    );
    assert!(before_sending(true, true, true, wont).is_none());
}

#[test]
fn the_report_goes_where_the_others_go_in_the_others_words() {
    // The shared crate's own tests read the Linux sources; here only that this host uses them.
    assert_eq!(guesthouse_report_core::contract::report_address(), "https://files.ruxn.com/r/guesthouse");
}

#[test]
fn a_machine_folder_cannot_name_a_way_out() {
    let (root, places) = world("way-out");
    let mut asked = request();
    asked.machine = Some("../Machines/windows-11".into());
    let found = collect::collect(&asked, &places);
    assert!(found.items.iter().all(|(name, _)| !name.starts_with("machine/")));
    let _ = fs::remove_dir_all(root);
}
