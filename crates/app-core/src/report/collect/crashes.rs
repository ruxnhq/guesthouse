//! What systemd-coredump kept of a recent crash of Guesthouse's programs: the record and its stack
//! trace, as text. Never the core file, which holds the program's memory.

use super::Collection;
use super::command::output_of;
use crate::report::kind::ProblemKind;
use serde::Deserialize;
use std::path::Path;

const NAME: &str = "host/crashes.txt";
/// The app's own programs.
const OWN_PROGRAMS: [&str; 5] =
    ["guesthouse", "guesthouse-task", "guesthouse-daemon", "guesthouse-vm", "guesthouse-share"];
/// What a crash report adds: the VM's programs, whose crashes are as often the system's as ours.
const VM_PROGRAMS: [&str; 3] = ["qemu-system-x86_64", "swtpm", "guesthouse-jail"];
const RECENT: &str = "--since=-3d";
const LONGER: &str = "--since=-14d";
/// The newest few, as a crash that repeats has the same trace each time.
const MOST_CRASHES: usize = 3;
const SECONDS: u32 = 30;

#[derive(Deserialize)]
struct Listed {
    pid: u32,
    exe: String,
}

pub fn collect(kind: ProblemKind, found: &mut Collection) {
    let every = kind.wants_every_crash();
    let since = if every { LONGER } else { RECENT };
    let listing = match output_of("coredumpctl", &["--no-pager", "--json=short", "list", since], SECONDS) {
        Ok(listing) => listing,
        // It says there's nothing by failing.
        Err(reason) if reason.contains("No coredumps found") => {
            found.notes.push("systemd-coredump has no recent crash of Guesthouse's programs.".to_owned());
            return;
        }
        Err(reason) => {
            if every {
                found.miss(
                    NAME,
                    &format!("the system's crash records could not be read: {reason}"),
                    Some("coredumpctl list"),
                );
            }
            return;
        }
    };
    let listed: Vec<Listed> = serde_json::from_str(&listing).unwrap_or_default();
    let ours = |crash: &&Listed| {
        let program = Path::new(&crash.exe).file_name().and_then(|name| name.to_str()).unwrap_or_default();
        OWN_PROGRAMS.contains(&program) || (every && VM_PROGRAMS.contains(&program))
    };
    let crashes: Vec<&Listed> = listed.iter().filter(ours).collect();
    if crashes.is_empty() {
        found.notes.push("systemd-coredump has no recent crash of Guesthouse's programs.".to_owned());
        return;
    }
    let mut text = String::new();
    for crash in crashes.iter().rev().take(MOST_CRASHES) {
        let pid = crash.pid.to_string();
        match output_of("coredumpctl", &["--no-pager", "info", &pid], SECONDS) {
            Ok(record) => text.push_str(&record),
            Err(reason) => {
                text.push_str(&format!("The record of process {pid} could not be read: {reason}\n"))
            }
        }
        text.push('\n');
    }
    found.add(NAME, text);
}
