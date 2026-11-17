//! A machine's logs from its last runs, from the run folder of each slot it last ran in.
//!
//! Slots are shared between machines, and a start doesn't clear every file an earlier run left. So a
//! folder's files only go in when its note says the run is this machine's, and only the ones that
//! changed after the note was written. The logs kept from the run before go by their own note the same
//! way. A folder from before there were notes gives only its task file, which names its machine.

use super::{Collection, Request};
use crate::context::Context;
use crate::machine::Machine;
use crate::tasks::{read_task, task_path};
use crate::vm_args::INSTALL_LOG;
use crate::vm_host::MACHINE_NOTE;
use crate::{daemon, slots, vm_args};
use std::fs;
use std::path::Path;
use std::time::SystemTime;

/// What the host's side of a run leaves: the daemon's output and QEMU's, QEMU's own log, the TPM's,
/// the Linux GPU device's calls, and the small notes the app and the daemon leave for the machine's
/// row (written in jailing.rs, daemon/src/display_note.rs, daemon/src/gpu_note.rs, actions/ubuntu.rs
/// and actions/desktop.rs).
const HOST_LOGS: [&str; 10] = [
    daemon::OUTPUT_FILE,
    vm_args::OUTPUT_FILE,
    "qemu.log",
    "swtpm.log",
    "calls.log",
    "jail-note",
    "display-ring",
    "gpu-stopped",
    "gpu-off-reason",
    "guesthouse-boots",
];
/// The logs a start keeps from the run before, in vm_host/local.rs.
const KEPT_HOST_LOGS: [&str; 2] = [daemon::OUTPUT_FILE, "qemu.log"];
/// What the guest wrote to its serial port.
const GUEST_SERIAL: &str = "serial.log";
const KEPT_SUFFIX: &str = ".1";
const GONE_AT_REBOOT: &str = "A machine's logs are in the runtime folder, which goes at each reboot, so runs from \
                              before the last boot have no logs here.";

pub fn collect(context: &Context, machine: &Machine, request: &Request, found: &mut Collection) {
    let mut any = false;
    for slot in slots::all() {
        let folder = context.run_folder(Some(slot));
        let place = format!("runs/slot-{}", slot.number());
        if read_task(context, Some(slot)).machine == machine.id {
            any = true;
            found
                .add_file_if_there(&format!("{place}/guesthouse-task.json"), &task_path(context, Some(slot)));
        }
        if let Some(since) = noted_at(&folder, MACHINE_NOTE, machine) {
            any = true;
            let mut names: Vec<&str> = HOST_LOGS.to_vec();
            if request.guest_serial {
                names.push(GUEST_SERIAL);
                if request.kind.wants_install_log() || machine.installing {
                    names.push(INSTALL_LOG);
                }
            }
            add_newer(found, &place, &folder, &names, "", since);
        }
        let kept_note = format!("{MACHINE_NOTE}{KEPT_SUFFIX}");
        if let Some(since) = noted_at(&folder, &kept_note, machine) {
            let mut names: Vec<&str> = KEPT_HOST_LOGS.to_vec();
            if request.guest_serial {
                names.push(GUEST_SERIAL);
            }
            add_newer(found, &place, &folder, &names, KEPT_SUFFIX, since);
        }
    }
    if !request.guest_serial {
        found.notes.push("The guest's serial log was left out, as asked.".to_owned());
    }
    if !any {
        found.notes.push(format!("No run folder holds this machine's last run. {GONE_AT_REBOOT}"));
    }
}

/// When the note was written, if it names this machine.
fn noted_at(folder: &Path, note: &str, machine: &Machine) -> Option<SystemTime> {
    let path = folder.join(note);
    let named = fs::read_to_string(&path).ok()?;
    (named.trim() == machine.id).then(|| modified(&path)).flatten()
}

fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

/// Each file that changed at or after `since`, and so is from the run the note is for.
fn add_newer(
    found: &mut Collection,
    place: &str,
    folder: &Path,
    names: &[&str],
    suffix: &str,
    since: SystemTime,
) {
    for name in names {
        let name = format!("{name}{suffix}");
        let path = folder.join(&name);
        if modified(&path).is_some_and(|changed| changed >= since) {
            found.add_file_if_there(&format!("{place}/{name}"), &path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::kind::ProblemKind;
    use crate::testing::{Scratch, scratch_context};
    use std::time::Duration;

    fn request(guest_serial: bool) -> Request {
        Request { machine: None, kind: ProblemKind::SomethingElse, guest_serial, app_facts: Vec::new() }
    }

    fn set_age(path: &Path, seconds_ago: u64) {
        let file = fs::File::options().append(true).open(path).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(seconds_ago)).unwrap();
    }

    #[test]
    fn only_files_newer_than_this_machines_note_go_in() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let folder = context.run_folder(Some(slots::first()));
        fs::create_dir_all(&folder).unwrap();
        let machine = Machine::new("w11", "W11", "/d", "/v");
        let ages = [
            ("daemon.out", 600),
            ("serial.log", 600),
            (MACHINE_NOTE, 60),
            ("qemu.log", 30),
            ("vm-gpupv.out", 30),
        ];
        for (name, age) in ages {
            fs::write(folder.join(name), if name == MACHINE_NOTE { "w11" } else { name }).unwrap();
            set_age(&folder.join(name), age);
        }
        let mut found = Collection::default();
        collect(&context, &machine, &request(true), &mut found);
        let names: Vec<&str> = found.names().collect();
        assert_eq!(names, ["runs/slot-0/vm-gpupv.out", "runs/slot-0/qemu.log"], "an earlier run's stay out");
    }

    #[test]
    fn another_machines_folder_gives_nothing() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let folder = context.run_folder(Some(slots::first()));
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join(MACHINE_NOTE), "other").unwrap();
        fs::write(folder.join("serial.log"), "another guest").unwrap();
        let mut found = Collection::default();
        collect(&context, &Machine::new("w11", "W11", "/d", "/v"), &request(true), &mut found);
        assert_eq!(found.names().count(), 0);
    }

    #[test]
    fn the_kept_run_goes_by_its_own_note_and_the_serial_choice() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let folder = context.run_folder(Some(slots::first()));
        fs::create_dir_all(&folder).unwrap();
        for name in ["run-machine.1", "daemon.out.1", "serial.log.1"] {
            fs::write(folder.join(name), "w11").unwrap();
        }
        let mut found = Collection::default();
        collect(&context, &Machine::new("w11", "W11", "/d", "/v"), &request(false), &mut found);
        let names: Vec<&str> = found.names().collect();
        assert_eq!(names, ["runs/slot-0/daemon.out.1"]);
    }
}
