// SPDX-License-Identifier: GPL-2.0-or-later
use super::*;
use crate::install::driver::{REQUIRED, fetch_package};
use crate::install::fake::FakeNetwork;
use crate::install::plan::Plan;
use crate::install::state::{InstallInfo, Source};

fn world(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!("gh-dv-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let machine = root.join("Machines/w");
    fs::create_dir_all(&machine).unwrap();
    (root, machine)
}

/// A support folder that has kept a released package (the stand-in's, version 0.0.0).
fn with_package(root: &Path) {
    fetch_package(&mut FakeNetwork, &root.join("Downloads")).unwrap();
}

#[test]
fn with_no_released_package_known_nothing_is_said() {
    let (root, machine) = world("none");
    record_reported(&machine, "0.1.0").unwrap();
    assert_eq!(state(&root, &machine), DriverState::Unknown);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn an_older_reported_version_is_old_and_a_same_or_newer_one_is_current() {
    let (root, machine) = world("old");
    with_package(&root);
    assert_eq!(released_version(&root).as_deref(), Some("0.0.0"));
    // The stand-in's version is older than the one reported.
    record_reported(&machine, "0.1.0").unwrap();
    assert_eq!(state(&root, &machine), DriverState::Current);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn the_comparison_is_by_number_not_by_text() {
    let (root, machine) = world("numbers");
    let package = root.join("Downloads/windows-driver/0.10.0");
    fs::create_dir_all(&package).unwrap();
    let mut files = Vec::new();
    for name in REQUIRED {
        fs::write(package.join(name), name).unwrap();
        files.push(format!(
            r#"{{"name":"p-{name}","as":"{name}","sha256":"{}"}}"#,
            crate::install::fetch::sha256_of(&package.join(name)).unwrap()
        ));
    }
    fs::write(
        package.join(MANIFEST_NAME),
        format!(r#"{{"version":"0.10.0","files":[{}]}}"#, files.join(",")),
    )
    .unwrap();
    record_reported(&machine, "0.9.0").unwrap();
    assert_eq!(
        state(&root, &machine),
        DriverState::Old { reported: "0.9.0".into(), released: "0.10.0".into() }
    );
    record_reported(&machine, "0.10.0").unwrap();
    assert_eq!(state(&root, &machine), DriverState::Current);
    // A machine whose install left the driver out is missing it, when nothing was reported.
    fs::remove_file(machine.join(REPORTED_FILE)).unwrap();
    assert_eq!(state(&root, &machine), DriverState::Unknown, "no install record, nothing to say");
    let mut info = InstallInfo::new("w", &Plan::for_guest(Guest::Windows), Source::Download);
    info.start_step(Step::InstallDriver, 0);
    info.fail(Step::InstallDriver, None, "turned away");
    state::save(&machine, &info).unwrap();
    assert_eq!(state(&root, &machine), DriverState::Missing { released: "0.10.0".into() });
    let _ = fs::remove_dir_all(root);
}

#[test]
fn only_a_version_is_kept() {
    let (root, machine) = world("keep");
    assert!(record_reported(&machine, "new").is_err());
    assert!(record_reported(&machine, "0.1").is_err());
    assert_eq!(reported_version(&machine), None);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn the_answer_for_the_app_says_the_versions() {
    assert_eq!(answer(&DriverState::Unknown), r#"{"state":"unknown"}"#);
    let old = answer(&DriverState::Old { reported: "0.9.0".into(), released: "0.10.0".into() });
    assert!(old.contains(r#""state":"old""#) && old.contains("0.10.0"));
}
