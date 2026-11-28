//! A host's own scrubbing pass and its host word, checked against the Windows host's golden copies in
//! `tests/report-golden/`, which this file only reads: a scrubber without a pass makes them byte for
//! byte, one with a pass that changes nothing makes them too, and a pass reaches every file, name,
//! reason and the description the Windows host's staging scrubs.

use guesthouse_control::System;
use guesthouse_control::report::host::Host;
use guesthouse_control::report::manifest::{Missing, manifest_for};
use guesthouse_control::report::stage::{self, FIRST_LIMIT, Facts};
use guesthouse_control::report::{Parts, ProblemKind, Scrubber};
use std::fs;
use std::path::{Path, PathBuf};

fn folder() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/report-golden")
}

fn kept(name: &str) -> String {
    fs::read_to_string(folder().join(name)).unwrap()
}

/// The golden test's three scrubbers, each with the copy it makes.
fn scrubbers() -> [(Scrubber, &'static str); 3] {
    let machines = ["quillon-games", "w11", "tv", "windows", "work-box"].map(str::to_owned);
    [
        (Scrubber::new("quillon", r"C:\Users\quillon", "marrowbox", &machines), "windows-scrub.txt"),
        (Scrubber::new("quillon", "/home/quillon", "marrowbox", &[]), "windows-scrub-unix-home.txt"),
        (Scrubber::new("", "", "", &[String::new()]), "windows-scrub-nothing-known.txt"),
    ]
}

#[test]
fn without_a_pass_the_scrub_is_todays() {
    for (scrubber, copy) in scrubbers() {
        assert!(scrubber.scrub(&kept("sample.txt")) == kept(copy), "{copy}");
    }
}

#[test]
fn the_shared_list_changes_nothing_it_has_already_scrubbed() {
    // So the closing pass after a host's own takes out only what that pass put in.
    for (scrubber, copy) in scrubbers() {
        assert!(scrubber.scrub(&kept(copy)) == kept(copy), "{copy}");
        let unchanged = scrubber.with_extra(|text| text.to_owned());
        assert!(unchanged.scrub(&kept("sample.txt")) == kept(copy), "{copy} with a pass that does nothing");
    }
}

fn parts() -> Parts {
    let mut parts = Parts::default();
    parts.piece("service.log", &format!("{}badge=opal-42\n", kept("sample.txt")));
    parts.piece("windows.txt", "ProductName: Windows 11 Pro\nDisplayVersion: 24H2\nCurrentBuild: 26100\n");
    parts.piece("quillon-games.guesthouse.json", r#"{"name": "quillon-games", "badge=opal-42": 1}"#);
    parts.miss("crashes.txt", "wevtutil failed for quillon with badge=opal-42");
    parts
}

fn facts() -> Facts<'static> {
    Facts {
        version: "0.13.0",
        windows: None,
        card: Some("NVIDIA GeForce RTX 4080"),
        machine: Some(System::Windows),
        kind: ProblemKind::Crashed,
        description: "quillon-games went black, badge=opal-42",
        contact: "quillon on the Discord",
    }
}

#[test]
fn a_pass_reaches_everything_the_staging_scrubs() {
    let (scrubber, _) = scrubbers().into_iter().next().unwrap();
    let hooked = scrubber.clone().with_extra(|text| text.replace("opal-42", "[gone]"));
    let plain = stage::stage(&parts(), &scrubber, &facts(), FIRST_LIMIT);
    let staged = stage::stage(&parts(), &hooked, &facts(), FIRST_LIMIT);
    let all = |staged: &stage::Staged| {
        let mut all: Vec<String> =
            staged.files.iter().map(|(name, text)| format!("{name}\n{text}")).collect();
        all.extend(staged.missing.iter().map(|(name, reason)| format!("{name}: {reason}")));
        all.push(staged.summary.to_json());
        all.join("\n")
    };
    assert!(all(&plain).contains("opal-42"), "the shared list alone leaves it");
    assert!(!all(&staged).contains("opal-42"), "{}", all(&staged));
    let text = |name: &str| &staged.files.iter().find(|(file, _)| file == name).unwrap().1;
    assert!(text("service.log").ends_with("badge=[gone]\n"));
    assert!(text("description.txt").contains("badge=[gone]"));
    assert!(staged.summary.to_json().contains("badge=[gone]"));
    assert!(staged.missing.iter().any(|(_, reason)| reason.ends_with("badge=[gone]")));
    let names =
        |staged: &stage::Staged| staged.files.iter().map(|(name, _)| name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&staged), names(&plain), "the files' names are the shared list's");
}

#[test]
fn the_windows_host_word_makes_todays_manifest() {
    let files = [("service.log".to_owned(), 1200), ("summary.json".to_owned(), 80)];
    let missing = [Missing { name: "crash dumps".into(), reason: "there were none".into(), how: None }];
    assert!(manifest_for(Host::WINDOWS, &files, &missing, &[]) == kept("windows-manifest.txt"));
}
