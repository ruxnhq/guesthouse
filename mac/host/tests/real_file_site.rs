// SPDX-License-Identifier: GPL-2.0-or-later
//! The real file site, with the real transport: fetches the Linux and the Windows driver
//! packages as published into a temporary folder, as the app does, and checks them. It needs the
//! network and only runs when `GUESTHOUSE_REAL_SITE=1`, so the suite stays offline otherwise.
//! `GUESTHOUSE_REAL_SITE=1 cargo test -p guesthouse-host-mac --test real_file_site -- --nocapture`
//! says what the site answered.
#![cfg(target_os = "macos")]

use guesthouse_core::install::driver::{self, LINUX, Package, WINDOWS};
use guesthouse_core::install::fetch::{CurlTransport, Transport};
use std::fs;
use std::time::Instant;

fn wanted() -> bool {
    std::env::var("GUESTHOUSE_REAL_SITE").is_ok_and(|v| v == "1")
}

fn fetch_for_real(package: &Package) {
    let downloads =
        std::env::temp_dir().join(format!("gh-real-site-{}-{}", package.folder, std::process::id()));
    let _ = fs::remove_dir_all(&downloads);
    let mut transport = CurlTransport;
    let manifest_url = format!("{}{}", package.base, package.manifest);
    let manifest = Manifest::of(&mut transport, &manifest_url);
    println!("{}: manifest {manifest_url} names version {}", package.folder, manifest);
    // What the site says to a HEAD of a file, as the app asks.
    let probe = format!("{}{}", package.base, first_file_name(&mut transport, &manifest_url));
    let head = transport.head(&probe).expect("the site answers a HEAD or the first byte");
    println!("{}: head of {probe}: length {:?}, ranges {}", package.folder, head.length, head.ranges);
    let began = Instant::now();
    let folder =
        driver::fetch_package_of(package, &mut transport, &downloads).expect("the package is fetched");
    let files: Vec<_> = fs::read_dir(&folder).unwrap().flatten().collect();
    let bytes: u64 = files.iter().filter_map(|f| f.metadata().ok()).map(|m| m.len()).sum();
    println!(
        "{}: {} files, {bytes} bytes, in {:?}, at {}",
        package.folder,
        files.len(),
        began.elapsed(),
        folder.display()
    );
    assert_eq!(
        driver::kept_package_of(package, &downloads),
        Some(folder),
        "the kept package is whole and checked"
    );
    let _ = fs::remove_dir_all(downloads);
}

/// The version a manifest names, for the report.
struct Manifest;

impl Manifest {
    fn of(transport: &mut CurlTransport, url: &str) -> String {
        let text = transport.text(url).expect("the manifest is there");
        let value: serde_json::Value = serde_json::from_str(&text).expect("the manifest is JSON");
        value["version"].as_str().unwrap_or("?").to_string()
    }
}

fn first_file_name(transport: &mut CurlTransport, url: &str) -> String {
    let text = transport.text(url).unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    value["files"][0]["name"].as_str().unwrap().to_string()
}

#[test]
fn the_real_linux_driver_package_is_fetched_though_the_site_gives_no_length_to_a_head() {
    if wanted() {
        fetch_for_real(&LINUX);
    }
}

#[test]
fn the_real_windows_driver_package_is_fetched_the_same_way() {
    if wanted() {
        fetch_for_real(&WINDOWS);
    }
}
