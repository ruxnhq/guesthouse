//! A machine's logs from its last runs, from the run folder of each slot it last ran in.
//!
//! Slots are shared between machines, so a folder's logs only go in when its note says they're this
//! machine's, and the logs kept from the run before only when their own note says so too.

use super::machine_files::machine_of_folder;
use super::{Collection, Request};
use crate::context::Context;
use crate::machine::Machine;
use crate::vm_args::INSTALL_LOG;
use crate::vm_host::MACHINE_NOTE;
use crate::{daemon, slots, vm_args};
use std::fs;
use std::path::Path;

/// What the host's side of a run leaves: the daemon's output and QEMU's, QEMU's own log, the TPM's,
/// the Linux GPU device's calls, and the small notes the app and the daemon leave for the machine's
/// row (written in jailing.rs, daemon/src/display_note.rs, daemon/src/gpu_note.rs, actions/ubuntu.rs,
/// actions/desktop.rs and tasks.rs).
const HOST_LOGS: [&str; 11] = [
    daemon::OUTPUT_FILE,
    vm_args::OUTPUT_FILE,
    "qemu.log",
    "swtpm.log",
    "calls.log",
    "guesthouse-task.json",
    "jail-note",
    "display-ring",
    "gpu-stopped",
    "gpu-off-reason",
    "guesthouse-boots",
];
/// The logs a start keeps from the run before, in vm_host/local.rs.
const KEPT_HOST_LOGS: [&str; 2] = [daemon::OUTPUT_FILE, "qemu.log"];
/// The guest's serial console, the one thing in a report from inside the guest.
const GUEST_SERIAL: &str = "serial.log";
const KEPT_SUFFIX: &str = ".1";
const GONE_AT_REBOOT: &str = "A machine's logs are in the runtime folder, which goes at each reboot, so runs from \
                              before the last boot have no logs here.";

pub fn collect(context: &Context, machine: &Machine, request: &Request, found: &mut Collection) {
    let mut any = false;
    for slot in slots::all() {
        let folder = context.run_folder(Some(slot));
        if machine_of_folder(context, &folder, slot).as_deref() != Some(machine.id.as_str()) {
            continue;
        }
        any = true;
        let place = format!("runs/slot-{}", slot.number());
        for name in HOST_LOGS {
            found.add_file_if_there(&format!("{place}/{name}"), &folder.join(name));
        }
        let wants_install_log = request.kind.wants_install_log() || machine.installing;
        if request.guest_serial {
            found.add_file_if_there(&format!("{place}/{GUEST_SERIAL}"), &folder.join(GUEST_SERIAL));
            if wants_install_log {
                found.add_file_if_there(&format!("{place}/{INSTALL_LOG}"), &folder.join(INSTALL_LOG));
            }
        }
        if kept_logs_are_this_machines(&folder, machine) {
            let kept: Vec<&str> =
                KEPT_HOST_LOGS.iter().copied().chain(request.guest_serial.then_some(GUEST_SERIAL)).collect();
            for name in kept {
                let name = format!("{name}{KEPT_SUFFIX}");
                found.add_file_if_there(&format!("{place}/{name}"), &folder.join(&name));
            }
        } else if folder.join(format!("{}{KEPT_SUFFIX}", daemon::OUTPUT_FILE)).exists() {
            found.notes.push(format!(
                "The logs kept from the run before in slot {} are left out, as nothing says which machine \
                 they're from.",
                slot.number()
            ));
        }
    }
    if !request.guest_serial {
        found.notes.push("The guest's serial log was left out, as asked.".to_owned());
    }
    if !any {
        found.notes.push(format!("No run folder holds this machine's last run. {GONE_AT_REBOOT}"));
    }
}

fn kept_logs_are_this_machines(folder: &Path, machine: &Machine) -> bool {
    let note = fs::read_to_string(folder.join(format!("{MACHINE_NOTE}{KEPT_SUFFIX}"))).unwrap_or_default();
    note.trim() == machine.id
}
