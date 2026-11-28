// SPDX-License-Identifier: GPL-2.0-or-later
//! The graphics driver's package for a Windows guest, as a release publishes it, and how it is got.
//!
//! **What a release must produce** (written down for whoever builds releases). The signing happens where releases are built, with a key that never leaves there
//! and is not in git. The package carries only what the guest needs to trust and load the driver:
//!
//! - `install.ps1`: trusts the certificate, turns test signing on and installs the driver from its
//!   own folder (`pnputil /add-driver ruxnwddm-full.inf /install`), says how it went on the serial
//!   port, and exits with Windows' error code if it failed. It signs nothing.
//! - `ruxnwddm.sys`, `ruxnwddm-full.inf`, `ruxnwddm-full.cat`: the kernel driver, already signed.
//! - `ruxnumd.dll`, `winemetal.dll`, `ruxndxgi.dll`, `ruxnd3d11.dll`: the user-mode driver and what
//!   it draws with (the INF's `[SourceDisksFiles]`), already signed in the catalog.
//! - `ruxn-driver.cer`: the public certificate the driver is signed with, and nothing private.
//!
//! and a manifest, `guesthouse-windows-driver-latest.json`, published beside them:
//!
//! ```json
//! {"version": "0.3.36", "files": [
//!   {"name": "guesthouse-windows-driver-0.3.36-ruxnwddm.sys", "as": "ruxnwddm.sys", "sha256": "..."}
//! ]}
//! ```
//!
//! The Windows driver's files are at `files.ruxn.com/p/guesthouse-windows-driver/<name>`, the owner's
//! file site, which serves a project's published files flat at `/p/<project>/<file>` (the layout the
//! other hosts' releases use). The project is its own, not the releases', because a package is
//! published by `.github/workflows/windows-pvg.yml` on its own schedule and with its own rules, and
//! the files site takes each workflow for one project. `name` is the published file name (with the
//! version in it, so a file is written once and never changes); `as` is its name in the package.
//! Only the manifest is replaced, by the next version. The guest's install puts the certificate in the guest's trusted stores and turns test
//! signing on by itself, so the tester never handles a certificate.
//!
//! `GUESTHOUSE_DRIVER_DIR` names a folder of the same files instead, for the lab.
//!
//! **A Linux guest's package** (`LINUX`) is published the same way, in a project of its own,
//! `files.ruxn.com/p/guesthouse-linux-driver/`, by `.github/workflows/linux-package.yml`, under
//! `guesthouse-linux-driver-latest.json`: `install.sh`, the `.deb` files it installs, and the
//! manifest, which goes to the guest with them so the guest knows the version it installed. Its
//! files and their words on the serial port are in `guest/drivers/linux-pvg/README.md`; `ubuntu` has the
//! guest's side of it.

use super::fetch::{self, FetchError, Outcome, Request, Transport};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

/// Where the owner's file site serves the Linux driver's package: a project of its own, published by
/// `.github/workflows/linux-package.yml`.
pub const LINUX_BASE_URL: &str = "https://files.ruxn.com/p/guesthouse-linux-driver/";
/// Where it serves the Windows driver's package: a project of its own, published by
/// `.github/workflows/windows-pvg.yml`.
pub const WINDOWS_BASE_URL: &str = "https://files.ruxn.com/p/guesthouse-windows-driver/";
pub const MANIFEST_NAME: &str = "guesthouse-windows-driver-latest.json";

/// What the package must hold, by the name each has in it.
pub const SCRIPT: &str = "install.ps1";
pub const CERTIFICATE: &str = "ruxn-driver.cer";
pub const REQUIRED: [&str; 9] = [
    SCRIPT,
    CERTIFICATE,
    "ruxnwddm.sys",
    "ruxnwddm-full.inf",
    "ruxnwddm-full.cat",
    "ruxnumd.dll",
    "winemetal.dll",
    "ruxndxgi.dll",
    "ruxnd3d11.dll",
];

/// One guest's package: the manifest it is published under, the folder a fetched one is kept in,
/// and the files it must hold by their names in it.
#[derive(Debug)]
pub struct Package {
    /// Where the file site serves the package's files, with the slash at the end.
    pub base: &'static str,
    pub manifest: &'static str,
    pub folder: &'static str,
    pub required: &'static [&'static str],
    /// Whether the manifest goes to the guest with the files.
    pub manifest_in_guest: bool,
}

pub const WINDOWS: Package = Package {
    base: WINDOWS_BASE_URL,
    manifest: MANIFEST_NAME,
    folder: "windows-driver",
    required: &REQUIRED,
    manifest_in_guest: false,
};

/// The script a Linux guest's package is installed with, run as root from its disk.
pub const LINUX_SCRIPT: &str = "install.sh";

pub const LINUX: Package = Package {
    base: LINUX_BASE_URL,
    manifest: "guesthouse-linux-driver-latest.json",
    folder: "linux-driver",
    required: &[LINUX_SCRIPT],
    manifest_in_guest: true,
};

/// Files that hold a private key. A package with one is refused, whoever made it.
const PRIVATE_KEY_ENDINGS: [&str; 5] = [".pfx", ".p12", ".pvk", ".key", ".snk"];

#[derive(Debug, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub version: String,
    pub files: Vec<Entry>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    #[serde(rename = "as")]
    pub package_name: String,
    pub sha256: String,
    /// The file's size in bytes, when the publisher says (a manifest may leave it out, as the
    /// first ones did): it lets a download be checked for room and shown progress when the file
    /// site will not say how big a file is.
    #[serde(default)]
    pub size: Option<u64>,
}

impl Manifest {
    /// Reads a manifest and checks it names every file the package needs, once each, with
    /// names that stay in their folder, checksums that are SHA-256 and no private key, under a
    /// version that is three numbers, as the version is the name of the folder it is kept in.
    pub fn parse(text: &str) -> Result<Manifest, String> {
        Manifest::parse_for(&WINDOWS, text)
    }

    /// The same for any guest's package.
    pub fn parse_for(package: &Package, text: &str) -> Result<Manifest, String> {
        let manifest: Manifest =
            serde_json::from_str(text).map_err(|e| format!("The driver's manifest is not readable: {e}"))?;
        if !guesthouse_update_check::is_version(&manifest.version)
            || manifest.version.trim() != manifest.version
        {
            return Err(format!(
                "The driver's manifest says its version is {:?}, which is not three numbers.",
                manifest.version
            ));
        }
        for entry in &manifest.files {
            for name in [&entry.name, &entry.package_name] {
                if !is_file_name(name) {
                    return Err(format!("The driver's manifest names {name:?}, which is not a file name."));
                }
            }
            if entry.sha256.len() != 64 || !entry.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(format!("The driver's manifest has no SHA-256 for {}.", entry.name));
            }
        }
        for &needed in package.required {
            let count = manifest.files.iter().filter(|e| e.package_name == needed).count();
            if count != 1 {
                return Err(format!(
                    "The driver's manifest must name {needed} once, and it names it {count} times."
                ));
            }
        }
        refuse_private_keys(manifest.files.iter().map(|e| e.package_name.as_str()))?;
        Ok(manifest)
    }
}

/// Whether `name` is a plain file name, kept in its folder and in the file site's address as it is:
/// letters, digits and `.`, `_`, `-`, `+`, `~`, not starting with a dot, and not too long for a Mac's
/// disk.
fn is_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !name.starts_with('.')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-+~".contains(&b))
}

fn refuse_private_keys<'a>(names: impl Iterator<Item = &'a str>) -> Result<(), String> {
    for name in names {
        let lower = name.to_ascii_lowercase();
        if PRIVATE_KEY_ENDINGS.iter().any(|ending| lower.ends_with(ending)) {
            return Err(format!(
                "The driver's package has {name}, which looks like a private key. Packages carry the public certificate only."
            ));
        }
    }
    Ok(())
}

/// Where a fetched package is kept: one folder for the version, with the manifest beside it.
fn folder_of(package: &Package, downloads: &Path, version: &str) -> PathBuf {
    downloads.join(package.folder).join(version)
}

/// Fetches the manifest and every file it names into `downloads`, each checked against its
/// SHA-256, and returns the folder holding the package under its own names.
pub fn fetch_package(transport: &mut dyn Transport, downloads: &Path) -> Result<PathBuf, FetchError> {
    fetch_package_of(&WINDOWS, transport, downloads)
}

/// The same for any guest's package.
pub fn fetch_package_of(
    package: &Package,
    transport: &mut dyn Transport,
    downloads: &Path,
) -> Result<PathBuf, FetchError> {
    let text = transport.text(&format!("{}{}", package.base, package.manifest))?;
    let manifest = Manifest::parse_for(package, &text).map_err(FetchError::new)?;
    let folder = folder_of(package, downloads, &manifest.version);
    let parts = folder.join("parts");
    for entry in &manifest.files {
        let url = format!("{}{}", package.base, entry.name);
        let request =
            Request { url: &url, folder: &parts, name: &entry.name, sha256: &entry.sha256, size: entry.size };
        match fetch::download(transport, &request, &mut |_, _| true)? {
            Outcome::Done(path) => {
                fs::copy(&path, folder.join(&entry.package_name))
                    .map_err(|e| FetchError::new(format!("Cannot keep {}: {e}", entry.package_name)))?;
            }
            Outcome::Stopped => return Err(FetchError::new("The driver's download was stopped.")),
        }
    }
    fs::write(folder.join(package.manifest), text)
        .map_err(|e| FetchError::new(format!("Cannot keep the driver's manifest: {e}")))?;
    Ok(folder)
}

/// The package a fetch kept, found again and checked against its manifest, newest first. None
/// when there is none whole.
pub fn kept_package(downloads: &Path) -> Option<PathBuf> {
    kept_package_of(&WINDOWS, downloads)
}

/// The same for any guest's package.
pub fn kept_package_of(package: &Package, downloads: &Path) -> Option<PathBuf> {
    let versions = fs::read_dir(downloads.join(package.folder)).ok()?;
    let mut found: Vec<PathBuf> =
        versions.flatten().map(|e| e.path()).filter(|p| p.join(package.manifest).is_file()).collect();
    // By version number (0.3.36 is newer than 0.3.9), not by the folder's text. A folder whose
    // name is not three numbers sorts before every version, so it is only used when it is all there is.
    found.sort_by_key(|folder| folder.file_name().and_then(|n| n.to_str()).and_then(version_numbers));
    found.into_iter().rev().find(|folder| {
        let Ok(text) = fs::read_to_string(folder.join(package.manifest)) else {
            return false;
        };
        Manifest::parse_for(package, &text).is_ok_and(|m| {
            m.files.iter().all(|e| {
                fetch::sha256_of(&folder.join(&e.package_name))
                    .is_ok_and(|sum| sum.eq_ignore_ascii_case(&e.sha256))
            })
        })
    })
}

/// A version's three numbers, for ordering. The shared crate's `newer` compares two; this gives
/// the numbers so a list can be sorted with them.
fn version_numbers(version: &str) -> Option<[u64; 3]> {
    if !guesthouse_update_check::is_version(version) {
        return None;
    }
    let mut parts = version.trim().split('.').map(|p| p.parse::<u64>().unwrap_or(0));
    Some([parts.next()?, parts.next()?, parts.next()?])
}

/// The files of a package folder, for the stick: every file in it, which must hold those the
/// guest needs, and none that is a private key. A folder from `GUESTHOUSE_DRIVER_DIR` is checked
/// the same way a fetched one is.
pub fn package_files(folder: &Path) -> Result<Vec<PathBuf>, String> {
    package_files_of(&WINDOWS, folder)
}

/// The same for any guest's package; the manifest is one of them when the guest is given it.
pub fn package_files_of(package: &Package, folder: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = fs::read_dir(folder)
        .map_err(|_| format!("The driver's files are not at {}. Build them first.", folder.display()))?;
    let files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| package.manifest_in_guest || p.file_name().is_some_and(|n| n != package.manifest))
        .collect();
    let names: Vec<String> =
        files.iter().filter_map(|f| f.file_name().map(|n| n.to_string_lossy().into_owned())).collect();
    let manifest = package.manifest_in_guest.then_some(package.manifest);
    for &needed in package.required.iter().chain(manifest.iter()) {
        if !names.iter().any(|n| n == needed) {
            return Err(format!("The driver's files at {} have no {needed}.", folder.display()));
        }
    }
    refuse_private_keys(names.iter().map(String::as_str))?;
    Ok(files)
}

#[cfg(test)]
mod tests;
