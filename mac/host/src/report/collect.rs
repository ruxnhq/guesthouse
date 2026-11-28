// SPDX-License-Identifier: GPL-2.0-or-later
//! Gathers each part of a report from its own source, and never stops for one that cannot be read:
//! what is missing is said, with why. Nothing is scrubbed here (`bundle` does that for every part).
//!
//! **What is never gathered**, whatever the machine holds: its disk, its firmware variables, the
//! answer disc and its folder (they held the guest account's password while an install ran), the
//! `remote-view` folder (rmhost's key and the devices it trusts), the Keychain, and anything of the
//! guest's memory.

use super::Request;
use guesthouse_report_core::trim::FIRST_LIMIT;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

/// What a report has found so far.
#[derive(Debug, Default)]
pub struct Collection {
    /// Each part's name in the report and its text.
    pub items: Vec<(String, String)>,
    /// Each part that is not in it, and why.
    pub missing: Vec<(String, String)>,
    pub notes: Vec<String>,
    /// Whether any log of the machine running is in it.
    pub has_run_logs: bool,
}

impl Collection {
    pub fn add(&mut self, name: &str, text: String) {
        self.items.push((name.to_owned(), text));
    }

    pub fn miss(&mut self, name: &str, reason: &str) {
        self.missing.push((name.to_owned(), reason.to_owned()));
    }

    /// A file's text if it is there, and its absence said if it is not.
    fn add_file(&mut self, name: &str, path: &Path, reason: &str) -> bool {
        match fs::read(path) {
            Ok(bytes) => {
                self.add(name, String::from_utf8_lossy(&bytes).into_owned());
                true
            }
            Err(_) => {
                self.miss(name, reason);
                false
            }
        }
    }
}

/// Where the parts are.
pub struct Places {
    /// `~/Library/Application Support/Guesthouse`.
    pub support: PathBuf,
    /// The Mac user's home folder, for the crash reports.
    pub home: PathBuf,
}

/// The settings that are private, marked as left out so it is still clear whether they were set.
/// None is kept in `machine.json` today; this is for one that is added.
const PRIVATE_FIELDS: [&str; 5] = ["password", "pairing_key", "fingerprint", "devices", "key"];
const LEFT_OUT: &str = "(left out, private)";

const COMMAND_SECONDS: u64 = 20;
/// How far back the app's own log is read from the unified log.
const UNIFIED_LOG_WINDOW: &str = "2h";
const MOST_CRASH_REPORTS: usize = 3;
const CRASH_REPORT_DAYS: u64 = 14;
const CRASH_REPORT_LINES: usize = 200;
/// What a crash report's file name starts with: the process's own name, which for the graphics process is
/// `guesthouse-pvg-host` (and `guesthouse-daemon` once there is one).
const CRASH_NAMES: [&str; 4] = ["Guesthouse", "qemu", "guesthouse-pvg-host", "guesthouse-daemon"];

pub fn collect(request: &Request, places: &Places) -> Collection {
    let mut found = Collection::default();
    found.add_file(
        "app/agent.log",
        &places.support.join("agent.log"),
        "the agent has not been run from this app, or its log was removed",
    );
    unified_log(&mut found);
    found.add("host/facts.txt", facts(request));
    crashes(&mut found, &places.home);
    if let Some(id) = request.machine.as_deref().filter(|id| valid_id(id)) {
        machine(&mut found, request, places, id);
    }
    found.notes.push(
        "Never in it: the machine's disk, its firmware variables, the answer disc, its pairing keys, the \
         Keychain, and anything of the guest's memory."
            .to_owned(),
    );
    if request.driver_version.is_none() {
        found.notes.push(
            "The guest's graphics driver version is not reported yet, so it is not in the report.".to_owned(),
        );
    }
    found
}

/// A machine's id is a folder name: no way out of the machines folder.
fn valid_id(id: &str) -> bool {
    !id.is_empty() && !id.contains('/') && !id.starts_with('.')
}

fn machine(found: &mut Collection, request: &Request, places: &Places, id: &str) {
    let folder = places.support.join("Machines").join(id);
    let run = places.support.join("Run").join(id);
    match fs::read_to_string(folder.join("machine.json")) {
        Ok(text) => found.add("machine/machine.json", without_private_fields(&text)),
        Err(error) => found.miss("machine/machine.json", &format!("it could not be read: {error}")),
    }
    if folder.join("install.json").is_file() {
        found.add_file("machine/install.json", &folder.join("install.json"), "it could not be read");
    }
    let mut logs: Vec<(&str, &str)> = vec![
        ("machine/qemu.log", "qemu.log"),
        ("machine/guard.log", "host.out"),
        ("machine/rmhost.log", "rmhost.log"),
    ];
    if request.include_serial {
        logs.push(("machine/serial.log", "serial.log"));
    } else {
        found.miss("machine/serial.log", "left out at the person's choice");
    }
    for (name, file) in logs {
        if found.add_file(name, &run.join(file), "the machine has not run, or this log was not made") {
            found.has_run_logs = true;
        }
    }
}

/// The settings with each private field marked as left out.
pub fn without_private_fields(text: &str) -> String {
    let Ok(serde_json::Value::Object(mut fields)) = serde_json::from_str::<serde_json::Value>(text) else {
        return "(machine.json could not be read as JSON, so it was left out)\n".to_owned();
    };
    for name in PRIVATE_FIELDS {
        if let Some(value) = fields.get_mut(name) {
            *value = match value {
                serde_json::Value::String(text) if text.is_empty() => continue,
                serde_json::Value::Array(list) => format!("{LEFT_OUT}: {} of them", list.len()).into(),
                _ => LEFT_OUT.into(),
            };
        }
    }
    serde_json::to_string_pretty(&serde_json::Value::Object(fields)).unwrap_or_default() + "\n"
}

/// The Mac, as the facts say it: its model, chip, system, memory and processors, and the app's and
/// the core's versions. No serial number, no hardware UUID and no names.
fn facts(request: &Request) -> String {
    let ask = |program: &str, args: &[&str]| {
        run_limited(program, args).map_or_else(|| "unknown".to_owned(), |text| text.trim().to_owned())
    };
    let memory: u64 = ask("/usr/sbin/sysctl", &["-n", "hw.memsize"]).parse().unwrap_or(0);
    let mut text = String::new();
    let mut line = |key: &str, value: String| text.push_str(&format!("{key}: {value}\n"));
    line("Guesthouse", request.app_version.clone());
    line("Core", env!("CARGO_PKG_VERSION").to_owned());
    line("Model", ask("/usr/sbin/sysctl", &["-n", "hw.model"]));
    line("Chip", ask("/usr/sbin/sysctl", &["-n", "machdep.cpu.brand_string"]));
    line("Processors", ask("/usr/sbin/sysctl", &["-n", "hw.ncpu"]));
    line("Memory", format!("{} GB", memory / (1 << 30)));
    line("macOS", ask("/usr/bin/sw_vers", &["-productVersion"]));
    line("macOS build", ask("/usr/bin/sw_vers", &["-buildVersion"]));
    line("Architecture", std::env::consts::ARCH.to_owned());
    if let Some(version) = &request.driver_version {
        line("Guest graphics driver", version.clone());
    }
    text
}

/// The app's own lines in the unified log, which is where its logger writes.
fn unified_log(found: &mut Collection) {
    let args = [
        "show",
        "--last",
        UNIFIED_LOG_WINDOW,
        "--style",
        "compact",
        "--predicate",
        "subsystem == \"com.ruxn.guesthouse\"",
    ];
    match run_limited("/usr/bin/log", &args) {
        Some(text) if !text.trim().is_empty() => found.add("app/unified-log.txt", text),
        _ => found
            .miss("app/unified-log.txt", "the system's log had nothing from the app, or could not be read"),
    }
}

/// The newest few crash reports of the app, QEMU and the graphics process, from the last weeks,
/// each cut to its first lines (the stack of the crashed thread is there).
fn crashes(found: &mut Collection, home: &Path) {
    let folder = home.join("Library/Logs/DiagnosticReports");
    let Ok(entries) = fs::read_dir(&folder) else {
        found.miss("host/crashes.txt", "the folder of crash reports could not be read");
        return;
    };
    let oldest = SystemTime::now() - Duration::from_secs(CRASH_REPORT_DAYS * 86_400);
    let mut recent: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            CRASH_NAMES.iter().any(|start| name.starts_with(start))
                && (name.ends_with(".ips") || name.ends_with(".crash"))
        })
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .filter(|(time, _)| *time >= oldest)
        .collect();
    recent.sort_by_key(|(time, _)| std::cmp::Reverse(*time));
    if recent.is_empty() {
        found.add(
            "host/crashes.txt",
            "No crash reports of Guesthouse, QEMU or the graphics process in the last two weeks.\n"
                .to_owned(),
        );
        return;
    }
    let mut text = String::new();
    for (_, path) in recent.into_iter().take(MOST_CRASH_REPORTS) {
        let body = fs::read_to_string(&path).unwrap_or_default();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        text.push_str(&format!("=== {name}\n"));
        for line in body.lines().take(CRASH_REPORT_LINES) {
            text.push_str(line);
            text.push('\n');
        }
    }
    found.add("host/crashes.txt", text);
}

/// Runs a program and returns what it wrote, or none if it could not run or took too long.
fn run_limited(program: &str, args: &[&str]) -> Option<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut text = Vec::new();
        let _ = std::io::Read::read_to_end(&mut stdout, &mut text);
        text
    });
    let deadline = Instant::now() + Duration::from_secs(COMMAND_SECONDS);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let text = reader.join().ok()?;
                return status.success().then(|| String::from_utf8_lossy(&text).into_owned());
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// How much of one part the first report keeps.
pub const FIRST_PER_FILE: usize = FIRST_LIMIT;
