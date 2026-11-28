//! Which VMs and daemons are running, and what the kernel says each is doing. All of it is read from
//! `/proc`, and nothing is asked of a running VM.

use super::Collection;
use crate::context::Context;
use crate::daemon::is_own_daemon;
use crate::machine::Machine;
use crate::process_stat::{read, state_of};
use crate::processes::running_vms;
use crate::slots;
use std::fs;

fn state_words(letter: char) -> &'static str {
    match letter {
        'R' => "running",
        'S' => "asleep",
        'D' => "waiting on a device",
        'T' | 't' => "stopped",
        'Z' => "a zombie",
        'I' => "idle",
        _ => "in another state",
    }
}

fn described(context: &Context, pid: u32) -> String {
    let state = read(context, pid).as_deref().and_then(state_of);
    state.map_or_else(
        || format!("process {pid}, gone"),
        |letter| format!("process {pid}, {}", state_words(letter)),
    )
}

pub fn collect(context: &Context, machine: Option<&Machine>, found: &mut Collection) {
    let mut text = String::new();
    let vms = running_vms(context);
    for slot in slots::all() {
        let number = slot.number();
        let qemu = match vms.get(&number) {
            Some(qemu) => {
                let whose = match machine {
                    Some(machine) if qemu.runs(machine) => ", this report's machine",
                    Some(_) => ", another machine",
                    None => "",
                };
                format!("{}{whose}", described(context, qemu.seen.pid))
            }
            None => "none".to_owned(),
        };
        let daemons: Vec<String> = fs::read_dir(&context.processes)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|pid| is_own_daemon(context, pid, slot))
            .filter_map(|pid| pid.parse().ok())
            .map(|pid| described(context, pid))
            .collect();
        let daemon = if daemons.is_empty() { "none".to_owned() } else { daemons.join("; ") };
        text.push_str(&format!("Slot {number}: QEMU {qemu}. Daemon {daemon}.\n"));
    }
    found.add("host/processes.txt", text);
}
