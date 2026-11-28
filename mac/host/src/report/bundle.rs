// SPDX-License-Identifier: GPL-2.0-or-later
//! Makes the files of a report from what was gathered (each scrubbed, the end of a long one kept,
//! with the person's words, the summary and the manifest), writes them in a folder of the app's own
//! and packs them as the others do: a `tar.gz` made by the system's `tar`. The folder is kept
//! beside the archive so the app can show the person exactly what is in it before it goes.

use super::collect::{Collection, FIRST_PER_FILE, Places};
use super::{Request, extra};
use guesthouse_report_core::clock;
use guesthouse_report_core::contract::MOST_REPORT_BYTES;
use guesthouse_report_core::host::Host;
use guesthouse_report_core::manifest::{
    DESCRIPTION_FILE, MANIFEST_FILE, Missing, SUMMARY_FILE, manifest_for,
};
use guesthouse_report_core::scrub::Scrubber;
use guesthouse_report_core::summary::{self, Summary};
use guesthouse_report_core::trim::{MOST_REPACKS, halved, keep_end};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const REPORTS_FOLDER: &str = "Reports";
const NAME_START: &str = "guesthouse-report-";
/// Reports kept, newest first, so a person can save one after sending it.
const KEPT_REPORTS: usize = 3;
/// What `host` says in a report from a Mac, and how the manifest says it is from one.
pub const HOST: Host = Host::new("macos", "a Mac");
const MACHINE_ID_MARK: &str = "machine";

/// A report as the app shows it before it is sent.
#[derive(Debug)]
pub struct Prepared {
    /// The packed report, which is what is sent.
    pub archive: PathBuf,
    /// The same files unpacked, to read.
    pub folder: PathBuf,
    pub files: Vec<(String, usize)>,
    pub missing: Vec<(String, String)>,
    pub notes: Vec<String>,
    pub summary: Summary,
    /// The header the summary goes in when it is sent.
    pub summary_header: String,
    pub bytes: u64,
}

/// This Mac's names, found once: what takes out the user, the host and the machines.
struct Names {
    scrubber: Scrubber,
    /// Each machine's folder name and what stands for it, as `<machine>` for the one the report is
    /// about and `<machine-2>` and on for the rest.
    ids: Vec<(String, String)>,
}

fn names(request: &Request, places: &Places) -> Names {
    let user = std::env::var("USER").unwrap_or_default();
    let computer = host_name();
    let mut found: Vec<(String, String)> = Vec::new();
    if let Ok(entries) = fs::read_dir(places.support.join("Machines")) {
        for entry in entries.flatten() {
            let id = entry.file_name().to_string_lossy().into_owned();
            let name = fs::read_to_string(entry.path().join("machine.json"))
                .ok()
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .and_then(|value| value["name"].as_str().map(str::to_owned))
                .unwrap_or_default();
            found.push((id, name));
        }
    }
    found.sort();
    // The machine the report is about first, as the Scrubber marks it `machine`.
    if let Some(about) = request.machine.as_deref()
        && let Some(at) = found.iter().position(|(id, _)| id == about)
    {
        let first = found.remove(at);
        found.insert(0, first);
    }
    let about_first = request.machine.is_some();
    let machine_names: Vec<String> = found.iter().map(|(_, name)| name.clone()).collect();
    let ids = found
        .iter()
        .enumerate()
        .map(|(number, (id, _))| {
            let mark = if number == 0 && about_first {
                format!("<{MACHINE_ID_MARK}>")
            } else {
                format!("<{MACHINE_ID_MARK}-{}>", number + 1)
            };
            (id.clone(), mark)
        })
        .collect();
    Names {
        scrubber: Scrubber::new(&user, &places.home.to_string_lossy(), &computer, &machine_names)
            // The Mac's own pass runs inside the shared one, which runs again after it, so nothing
            // it writes can put back what the shared list takes out.
            .with_extra(extra::scrub),
        ids,
    }
}

fn host_name() -> String {
    Command::new("/bin/hostname")
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        .unwrap_or_default()
}

impl Names {
    /// Everything that is taken out of every file: the shared scrubber with the Mac's own pass
    /// inside it, then the machines' folder names where they stand as a part of a path. The folder
    /// names are this Mac's own data, which a pass (a plain function) cannot hold, so they go last,
    /// on text that is already scrubbed.
    fn scrub(&self, text: &str) -> String {
        let mut text = self.scrubber.scrub(text);
        for (id, mark) in &self.ids {
            text = replace_path_part(&text, id, mark);
        }
        text
    }
}

fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
}

/// `id` where it is a whole part of a path (after a `/`, and not followed by more of a name).
fn replace_path_part(text: &str, id: &str, mark: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    for (at, _) in text.match_indices(id) {
        let end = at + id.len();
        let before = text[..at].bytes().next_back();
        let after = text[end..].bytes().next();
        if at >= copied && before == Some(b'/') && !after.is_some_and(is_name_byte) {
            out.push_str(&text[copied..at]);
            out.push_str(mark);
            copied = end;
        }
    }
    out.push_str(&text[copied..]);
    out
}

/// Gathers nothing: makes the report from what `collection` has, in `places`' Reports folder.
pub fn prepare(request: &Request, collection: &Collection, places: &Places) -> Result<Prepared, String> {
    let reports = places.support.join(REPORTS_FOLDER);
    fs::create_dir_all(&reports).map_err(|e| format!("Cannot make {}: {e}", reports.display()))?;
    let names = names(request, places);
    let stamp = clock::stamp(clock::now());
    let name = format!("{NAME_START}{stamp}");
    let folder = reports.join(&name);
    let archive = reports.join(format!("{name}.tar.gz"));
    let mut per_file = FIRST_PER_FILE;
    for _ in 0..MOST_REPACKS {
        let _ = fs::remove_dir_all(&folder);
        let (files, summary) = write_files(request, collection, &names, &folder, per_file)?;
        pack(&reports, &name, &archive)?;
        let bytes = fs::metadata(&archive).map_err(|e| e.to_string())?.len();
        if bytes <= MOST_REPORT_BYTES {
            keep_newest(&reports);
            return Ok(Prepared {
                archive,
                folder,
                files,
                missing: collection.missing.clone(),
                notes: collection.notes.clone(),
                summary_header: summary.header(),
                summary,
                bytes,
            });
        }
        per_file = halved(per_file);
    }
    let _ = fs::remove_file(&archive);
    Err("The report stayed too big to send, however much of each log was left out.".into())
}

type Written = (Vec<(String, usize)>, Summary);

fn write_files(
    request: &Request,
    collection: &Collection,
    names: &Names,
    folder: &Path,
    per_file: usize,
) -> Result<Written, String> {
    let mut files: Vec<(String, usize)> = Vec::new();
    for (name, text) in &collection.items {
        put(folder, &mut files, name, &keep_end(&names.scrub(text), per_file))?;
    }
    let description = names.scrub(request.description.trim());
    put(folder, &mut files, DESCRIPTION_FILE, &format!("{description}\n"))?;
    let summary = summary_of(request, collection, &description);
    put(folder, &mut files, SUMMARY_FILE, &summary.to_json())?;
    let manifest = manifest(&files, &collection.missing, &collection.notes);
    put(folder, &mut files, MANIFEST_FILE, &manifest)?;
    Ok((files, summary))
}

fn put(folder: &Path, files: &mut Vec<(String, usize)>, name: &str, text: &str) -> Result<(), String> {
    let path = folder.join(name);
    fs::create_dir_all(path.parent().unwrap_or(folder)).map_err(|e| e.to_string())?;
    fs::write(&path, text).map_err(|e| format!("Cannot write {name}: {e}"))?;
    files.push((name.to_owned(), text.len()));
    Ok(())
}

/// The few facts that sort a report: the Mac's, never the machine's name. The description is the
/// scrubbed one.
fn summary_of(request: &Request, collection: &Collection, description: &str) -> Summary {
    let mut summary = Summary::default();
    summary.set(summary::VERSION, &request.app_version);
    summary.set_host(HOST);
    let fact = |key: &str| {
        collection
            .items
            .iter()
            .find(|(name, _)| name == "host/facts.txt")
            .and_then(|(_, text)| text.lines().find_map(|line| line.strip_prefix(key)?.strip_prefix(": ")))
            .map(str::to_owned)
    };
    if let Some(os) = fact("macOS") {
        summary.set(summary::OS, &format!("macOS {os}"));
    }
    if let Some(chip) = fact("Chip") {
        summary.set(summary::CARD, &chip);
    }
    if let Some(driver) = &request.driver_version {
        summary.set(summary::DRIVER, driver);
    }
    summary.set(summary::KIND, request.problem_kind().word());
    summary.set(summary::DESCRIPTION, description);
    summary.set(summary::CONTACT, &request.contact);
    summary
}

/// What the Mac takes out beyond the shared scrubber's list, said in the manifest's notes.
const EXTRA_REDACTIONS: &str = "Beyond the user name, host name and home folder, passwords, pairing keys, device fingerprints, pairing codes and serial numbers were taken out where they were found.";

/// The shared manifest, for a report from a Mac.
fn manifest(files: &[(String, usize)], missing: &[(String, String)], notes: &[String]) -> String {
    let missing: Vec<Missing> = missing
        .iter()
        .map(|(name, reason)| Missing { name: name.clone(), reason: reason.clone(), how: None })
        .collect();
    let mut notes = notes.to_vec();
    notes.insert(0, EXTRA_REDACTIONS.to_owned());
    manifest_for(HOST, files, &missing, &notes)
}

fn pack(reports: &Path, name: &str, archive: &Path) -> Result<(), String> {
    let result = Command::new("tar")
        .arg("-czf")
        .arg(archive)
        .arg("-C")
        .arg(reports)
        .arg(name)
        .output()
        .map_err(|e| format!("tar could not run: {e}"))?;
    if result.status.success() {
        Ok(())
    } else {
        Err(format!("tar could not pack the report: {}", String::from_utf8_lossy(&result.stderr).trim()))
    }
}

/// Leaves the newest few reports (each a folder and its archive), by the time in their names.
fn keep_newest(reports: &Path) {
    let mut folders: Vec<PathBuf> = fs::read_dir(reports)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    folders.sort();
    let old = folders.len().saturating_sub(KEPT_REPORTS);
    for folder in &folders[..old] {
        let _ = fs::remove_dir_all(folder);
        let _ = fs::remove_file(folder.with_extension("tar.gz"));
    }
}
