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
/// Lines of a record that name this PC, its boot or where the core file is, and nothing about the crash.
const PC_LINES: [&str; 4] = ["Machine ID:", "Boot ID:", "Hostname:", "Storage:"];

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
            found.miss(
                NAME,
                &format!("the system's crash records could not be read: {reason}"),
                Some("coredumpctl list"),
            );
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
            Ok(record) => text.push_str(&without_pc_lines(&record)),
            Err(reason) => {
                text.push_str(&format!("The record of process {pid} could not be read: {reason}\n"))
            }
        }
        text.push('\n');
    }
    found.add(NAME, text);
}

fn without_pc_lines(record: &str) -> String {
    let wanted = |line: &&str| !PC_LINES.iter().any(|start| line.trim_start().starts_with(start));
    record.lines().filter(wanted).map(|line| format!("{line}\n")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_keeps_the_crash_and_not_the_pcs_ids() {
        let record = "           PID: 4242 (guesthouse)\n\
                      \x20      Boot ID: 981906ea9dfc43e7ade3e79a95c6d2d9\n\
                      \x20   Machine ID: b0160524751c40ef9abb4386a64016fd\n\
                      \x20     Hostname: marrowbox\n\
                      \x20      Storage: /var/lib/systemd/coredump/core.guesthouse.1000.981906ea.zst (present)\n\
                      \x20      Message: Process 4242 (guesthouse) of user 1000 dumped core.\n\
                      \x20               #0  0x00007f1b07caf22b pthread_kill (libc.so.6 + 0xaf22b)\n";
        let kept = without_pc_lines(record);
        for gone in ["981906ea", "b0160524", "marrowbox", "coredump/core"] {
            assert!(!kept.contains(gone), "{gone} is still in:\n{kept}");
        }
        assert!(kept.contains("PID: 4242") && kept.contains("pthread_kill"));
    }
}
