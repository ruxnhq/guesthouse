// SPDX-License-Identifier: GPL-2.0-or-later
//! Stand-ins for the network and the guest, so an install can be run with neither: by the tests,
//! and by the agent when it is started with `GUESTHOUSE_INSTALL_FAKE` set, to try the install
//! screens or the command line (`--install`) with no download and no machine.
//!
//! Time here is the stand-in's own: waiting moves it on, so a guest that takes an hour of its time
//! takes a moment of ours.

use super::driver;
use super::fetch::{FetchError, Head, Transport};
use super::steps::MachineControl;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The Ubuntu disc the stand-in network serves.
pub const UBUNTU_BYTES: &[u8] = b"pretend this is the ubuntu server disc for arm64";

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// The version the stand-in driver package says it is.
pub const FAKE_DRIVER_VERSION: &str = "0.0.0";

/// What the stand-in serves for a driver file's published name.
fn driver_bytes(name: &str) -> Vec<u8> {
    format!("pretend this is {name}").into_bytes()
}

/// The published name of a stand-in driver file.
fn driver_name(package_name: &str) -> String {
    format!("guesthouse-windows-driver-{FAKE_DRIVER_VERSION}-{package_name}")
}

/// Ubuntu's list of checksums and its disc, and the driver's manifest and files, from memory.
pub struct FakeNetwork;

impl FakeNetwork {
    /// The bytes behind a URL: the Ubuntu disc, or a driver file by its published name.
    fn body(url: &str) -> Vec<u8> {
        match url.strip_prefix(driver::WINDOWS_BASE_URL) {
            Some(name) => driver_bytes(name),
            None => UBUNTU_BYTES.to_vec(),
        }
    }
}

impl Transport for FakeNetwork {
    fn head(&mut self, url: &str) -> Result<Head, FetchError> {
        Ok(Head { length: Some(Self::body(url).len() as u64), ranges: true })
    }

    fn get_from(
        &mut self,
        url: &str,
        from: u64,
        out: &mut dyn Write,
        progress: &mut dyn FnMut(u64) -> bool,
    ) -> Result<(), FetchError> {
        let body = Self::body(url);
        // In pieces, so progress is told more than once.
        for (index, piece) in body[from as usize..].chunks(8).enumerate() {
            out.write_all(piece).map_err(|e| FetchError::new(e.to_string()))?;
            if !progress((index * 8 + piece.len()) as u64) {
                return Ok(());
            }
        }
        Ok(())
    }

    fn text(&mut self, url: &str) -> Result<String, FetchError> {
        if url == format!("{}{}", driver::WINDOWS_BASE_URL, driver::MANIFEST_NAME) {
            let files: Vec<String> = driver::REQUIRED
                .iter()
                .map(|package_name| {
                    let name = driver_name(package_name);
                    format!(
                        r#"{{"name":"{name}","as":"{package_name}","sha256":"{}"}}"#,
                        sha256_hex(&driver_bytes(&name))
                    )
                })
                .collect();
            return Ok(format!(r#"{{"version":"{FAKE_DRIVER_VERSION}","files":[{}]}}"#, files.join(",")));
        }
        if !url.starts_with("https://cdimage.ubuntu.com/ubuntu/releases/26.04/release/") {
            return Err(FetchError::new(format!("{url} is not somewhere the stand-in knows.")));
        }
        Ok(format!("{} *ubuntu-26.04-live-server-arm64.iso\n", sha256_hex(UBUNTU_BYTES)))
    }
}

/// The file site as the real one answers: both driver packages, the Linux one as well as the
/// Windows one, and a HEAD that gives no length, as files.ruxn.com does, when `no_length`. The
/// Ubuntu disc and its list are `FakeNetwork`'s.
#[derive(Default)]
pub struct FakeSite {
    pub no_length: bool,
}

/// The Linux package's version the stand-in site publishes.
pub const FAKE_LINUX_VERSION: &str = "0.0.1";

fn linux_name(package_name: &str) -> String {
    format!("guesthouse-linux-driver-{FAKE_LINUX_VERSION}-{package_name}")
}

impl FakeSite {
    /// What the Linux package's `install.sh` is, the only file it must hold.
    fn linux_bytes(name: &str) -> Vec<u8> {
        format!("#!/bin/sh\n# pretend this is {name}\n").into_bytes()
    }
}

impl Transport for FakeSite {
    fn head(&mut self, url: &str) -> Result<Head, FetchError> {
        if self.no_length {
            return Ok(Head { length: None, ranges: true });
        }
        match url.strip_prefix(driver::LINUX_BASE_URL) {
            Some(name) => Ok(Head { length: Some(Self::linux_bytes(name).len() as u64), ranges: true }),
            None => FakeNetwork.head(url),
        }
    }

    fn get_from(
        &mut self,
        url: &str,
        from: u64,
        out: &mut dyn Write,
        progress: &mut dyn FnMut(u64) -> bool,
    ) -> Result<(), FetchError> {
        match url.strip_prefix(driver::LINUX_BASE_URL) {
            Some(name) => {
                out.write_all(&Self::linux_bytes(name)[from as usize..])
                    .map_err(|e| FetchError::new(e.to_string()))?;
                progress(0);
                Ok(())
            }
            None => FakeNetwork.get_from(url, from, out, progress),
        }
    }

    fn text(&mut self, url: &str) -> Result<String, FetchError> {
        if url == format!("{}{}", driver::LINUX_BASE_URL, driver::LINUX.manifest) {
            let name = linux_name(driver::LINUX_SCRIPT);
            return Ok(format!(
                r#"{{"version":"{FAKE_LINUX_VERSION}","files":[{{"name":"{name}","as":"{}","sha256":"{}"}}]}}"#,
                driver::LINUX_SCRIPT,
                sha256_hex(&Self::linux_bytes(&name))
            ));
        }
        FakeNetwork.text(url)
    }
}

/// The Mac's side, in memory: time that only moves when something waits, a serial port that says
/// things at set times, and a machine that starts and stops as the script says.
#[derive(Default)]
pub struct FakeGuest {
    pub now: u64,
    /// What the guest says on its serial port, and from when.
    pub says: Vec<(u64, String)>,
    pub started: Vec<String>,
    /// When the machine stops by itself, and the reason it fails with.
    pub stops_at: Option<u64>,
    pub fails: Option<String>,
    /// The label and contents of each disc made, and the files put on the stick.
    pub discs: Vec<(String, String)>,
    pub stick_files: Vec<String>,
    pub start_error: Option<String>,
    /// Real milliseconds to sleep for each wait, so a person watching can see it move.
    pub real_pause_ms: u64,
    /// What each disc image holds, by path, for `extract`; a path not named is a stand-in's.
    pub extracted: Vec<String>,
    /// The line a Linux driver package's `install.sh` says, when one is on the stick: the
    /// stand-in guest says it at its next start, as the driver's service would run it.
    pub stick_line: Option<String>,
    /// When the stand-in guest turns itself off after a driver that failed.
    pub off_at: Option<u64>,
}

/// After a start with the driver's disk, when the stand-in guest says what `install.sh` says,
/// and then that it came up again.
const DRIVER_SAYS_AFTER_MS: u64 = 15_000;
const FIRST_START_AFTER_MS: u64 = 30_000;
/// How long the stand-in guest takes to turn itself off after a driver that failed.
const POWER_OFF_AFTER_MS: u64 = 5_000;

/// The first line of the serial port's words in a stand-in `install.sh`, if it has one.
fn said_by_script(text: &str) -> Option<String> {
    use super::windows::marker;
    let start =
        [marker::DRIVER_FAILED, marker::DRIVER_INSTALLED].iter().filter_map(|m| text.find(m)).min()?;
    let line = &text[start..];
    let end = line.find(['"', '\'', '\n']).unwrap_or(line.len());
    Some(line[..end].trim().to_string())
}

impl FakeGuest {
    /// A guest that does everything right, quickly: for trying the screens and the command line.
    pub fn quick(guest: super::plan::Guest) -> FakeGuest {
        use super::{ubuntu, windows::marker};
        let says = match guest {
            super::plan::Guest::Windows => vec![
                (30_000, marker::SYSTEM_INSTALLED.to_string()),
                (60_000, marker::DRIVER_INSTALLED.to_string()),
                (90_000, marker::FIRST_START.to_string()),
            ],
            super::plan::Guest::Ubuntu => vec![(30_000, ubuntu::FINISHED.to_string())],
        };
        FakeGuest { says, stops_at: Some(45_000), real_pause_ms: 20, ..FakeGuest::default() }
    }
}

impl MachineControl for FakeGuest {
    fn make_disc(&mut self, folder: &Path, label: &str, out: &Path) -> Result<(), String> {
        let mut text = String::new();
        for entry in fs::read_dir(folder).map_err(|e| e.to_string())?.flatten() {
            text += &fs::read_to_string(entry.path()).unwrap_or_default();
        }
        self.discs.push((label.to_string(), text));
        fs::write(out, b"disc").map_err(|e| e.to_string())
    }

    fn make_stick(&mut self, image: &Path, files: &[PathBuf]) -> Result<(), String> {
        self.stick_line = files
            .iter()
            .find(|f| f.file_name().is_some_and(|n| n == driver::LINUX_SCRIPT))
            .and_then(|f| fs::read_to_string(f).ok())
            .and_then(|text| said_by_script(&text));
        self.stick_files =
            files.iter().filter_map(|f| f.file_name()).map(|n| n.to_string_lossy().into_owned()).collect();
        fs::write(image, b"stick").map_err(|e| e.to_string())
    }

    fn extract(&mut self, image: &Path, names: &[&str], folder: &Path) -> Result<(), String> {
        if !image.is_file() {
            return Err(format!("{} is not there.", image.display()));
        }
        for name in names {
            let path = folder.join(name);
            fs::create_dir_all(path.parent().unwrap_or(folder)).map_err(|e| e.to_string())?;
            fs::write(&path, format!("pretend this is {name}")).map_err(|e| e.to_string())?;
            self.extracted.push(name.to_string());
        }
        Ok(())
    }

    fn start(&mut self, id: &str) -> Result<(), String> {
        if let Some(error) = &self.start_error {
            return Err(error.clone());
        }
        self.started.push(id.to_string());
        self.off_at = None;
        // A start with a Linux driver's disk: the service runs its install.sh.
        if let Some(line) = self.stick_line.clone() {
            self.says.push((self.now + DRIVER_SAYS_AFTER_MS, line.clone()));
            if line.contains(super::windows::marker::DRIVER_INSTALLED) {
                let first = super::windows::marker::FIRST_START.to_string();
                self.says.push((self.now + FIRST_START_AFTER_MS, first));
            } else {
                self.off_at = Some(self.now + DRIVER_SAYS_AFTER_MS + POWER_OFF_AFTER_MS);
            }
        }
        Ok(())
    }

    fn machine_state(&mut self, _: &str) -> (String, Option<String>) {
        if let Some(why) = &self.fails {
            return ("failed".into(), Some(why.clone()));
        }
        if self.off_at.is_some_and(|at| self.now >= at) {
            return ("off".into(), None);
        }
        match self.stops_at {
            Some(at) if self.now >= at && self.started.len() == 1 => ("off".into(), None),
            _ if self.started.is_empty() => ("off".into(), None),
            _ => ("running".into(), None),
        }
    }

    fn serial(&mut self, _: &str) -> String {
        self.says.iter().filter(|(at, _)| *at <= self.now).map(|(_, t)| format!("{t}\n")).collect()
    }

    fn now_ms(&mut self) -> u64 {
        self.now
    }

    fn sleep_ms(&mut self, ms: u64) {
        self.now += ms;
        if self.real_pause_ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(self.real_pause_ms));
        }
    }
}
