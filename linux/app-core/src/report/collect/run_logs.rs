//! A machine's logs from its last runs: from the run folder of each slot it last ran in, or else from
//! the copy the app keeps of its last run, as run folders go at each reboot.
//!
//! Which of a folder's files are a run's own is `run_files`' to say. A folder from before there were
//! notes gives only its task file, which names its machine.

use super::{Collection, Request};
use crate::context::Context;
use crate::kept_logs::{kept_folder, kept_names, kept_started};
use crate::machine::Machine;
use crate::report::scrub::scrub_emails;
use crate::run_files::{
    GUEST_LOGCAT, GUEST_SERIAL, HOST_LOGS, INSTALL_SERIAL, KEPT_HOST_LOGS, KEPT_SUFFIX, files_since,
    kept_run_of, local_time, run_of,
};
use crate::slots;
use crate::tasks::{read_task, task_path};
use std::time::UNIX_EPOCH;

const GONE_AT_REBOOT: &str = "Run folders go at each reboot, and the app keeps a copy of a machine's last run \
                              from when it ends or the slot is next used.";

pub fn collect(context: &Context, machine: &Machine, request: &Request, found: &mut Collection) {
    let wanted = |names: &[&'static str]| -> Vec<&'static str> {
        names
            .iter()
            .copied()
            .filter(|name| match *name {
                GUEST_SERIAL => request.guest_serial,
                GUEST_LOGCAT => request.guest_logs,
                INSTALL_SERIAL => {
                    request.guest_serial && (request.kind.wants_install_log() || machine.installing)
                }
                _ => true,
            })
            .collect()
    };
    let current: Vec<&str> =
        wanted(&[&HOST_LOGS[..], &[GUEST_SERIAL, INSTALL_SERIAL, GUEST_LOGCAT]].concat());
    for slot in slots::all() {
        let folder = context.run_folder(Some(slot));
        let place = format!("runs/slot-{}", slot.number());
        if read_task(context, Some(slot)).machine == machine.id {
            found
                .add_file_if_there(&format!("{place}/guesthouse-task.json"), &task_path(context, Some(slot)));
        }
        if let Some(since) = run_of(&folder, &machine.id) {
            found.has_run_logs = true;
            found.add_file_if_there(&format!("{place}/jail-note"), &crate::jailing::note_path(&folder));
            add_all(found, &place, files_since(&folder, &current, "", since));
            found.notes.push(format!(
                "The logs in {place} are from the run folder, of the run that started at {}.",
                local_time(since)
            ));
        }
        if let Some(since) = kept_run_of(&folder, &machine.id) {
            let kept: Vec<&str> = wanted(&[&KEPT_HOST_LOGS[..], &[GUEST_SERIAL]].concat());
            add_all(found, &place, files_since(&folder, &kept, KEPT_SUFFIX, since));
        }
    }
    if !found.has_run_logs {
        let kept = kept_folder(context, &machine.id);
        if let Some(started) = kept_started(context, &machine.id) {
            found.has_run_logs = true;
            let names: Vec<&str> = wanted(&kept_names()).into_iter().collect();
            add_all(found, "runs/kept", files_since(&kept, &names, "", UNIX_EPOCH));
            found.notes.push(format!(
                "No run folder holds this machine's last run, so runs/kept is the copy the app kept of it. \
                 That run started at {started}."
            ));
        } else {
            found.notes.push(format!("There are no logs of this machine running. {GONE_AT_REBOOT}"));
        }
    }
    if !request.guest_serial {
        found.notes.push("The guest's serial log was left out, as asked.".to_owned());
    }
}

fn add_all(found: &mut Collection, place: &str, files: Vec<(String, std::path::PathBuf)>) {
    for (name, path) in files {
        let before = found.items.len();
        found.add_run_file(&format!("{place}/{name}"), &path);
        // Apps write what they like to logcat, an account's address among it.
        if name == GUEST_LOGCAT
            && let Some(item) = found.items.get_mut(before)
        {
            item.text = scrub_emails(&item.text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kept_logs::keep_run;
    use crate::report::kind::ProblemKind;
    use crate::testing::{Scratch, scratch_context};
    use crate::vm_host::MACHINE_NOTE;
    use std::fs;
    use std::path::Path;
    use std::time::{Duration, SystemTime};

    fn request(guest_serial: bool) -> Request {
        Request {
            machine: None,
            kind: ProblemKind::SomethingElse,
            guest_serial,
            guest_logs: false,
            app_facts: Vec::new(),
        }
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
            if name == MACHINE_NOTE {
                crate::run_files::plant_note(&folder, name, "w11");
                set_age(&crate::paths::notes_beside(&folder).join(name), age);
                continue;
            }
            fs::write(folder.join(name), name).unwrap();
            set_age(&folder.join(name), age);
        }
        let mut found = Collection::default();
        collect(&context, &machine, &request(true), &mut found);
        let names: Vec<&str> = found.names().collect();
        assert_eq!(names, ["runs/slot-0/vm-gpupv.out", "runs/slot-0/qemu.log"], "an earlier run's stay out");
    }

    #[test]
    fn androids_logcat_follows_the_guest_logs_switch_and_loses_addresses() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let folder = context.run_folder(Some(slots::first()));
        fs::create_dir_all(&folder).unwrap();
        crate::run_files::plant_note(&folder, MACHINE_NOTE, "phone");
        fs::write(folder.join("logcat.log"), "E Auth: no token for quillon.marrow@example.com\n").unwrap();
        let machine = Machine::new("phone", "Phone", "/d", "/v");
        let mut found = Collection::default();
        collect(&context, &machine, &request(true), &mut found);
        assert_eq!(found.names().count(), 0, "left out with the guest's own logs");
        let mut found = Collection::default();
        collect(&context, &machine, &Request { guest_logs: true, ..request(true) }, &mut found);
        assert_eq!(found.names().collect::<Vec<_>>(), ["runs/slot-0/logcat.log"]);
        assert_eq!(found.items[0].text, "E Auth: no token for <email>\n");
    }

    #[test]
    fn a_link_or_a_pipe_in_the_run_folder_is_not_read() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let folder = context.run_folder(Some(slots::first()));
        fs::create_dir_all(&folder).unwrap();
        crate::run_files::plant_note(&folder, MACHINE_NOTE, "w11");
        let elsewhere = scratch.write("elsewhere/notes.txt", "not the run's");
        std::os::unix::fs::symlink(&elsewhere, folder.join("qemu.log")).unwrap();
        let pipe =
            std::ffi::CString::new(folder.join("swtpm.log").into_os_string().into_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(pipe.as_ptr(), 0o600) }, 0);
        fs::write(folder.join("daemon.out"), "the run's").unwrap();
        let mut found = Collection::default();
        collect(&context, &Machine::new("w11", "W11", "/d", "/v"), &request(true), &mut found);
        assert_eq!(found.names().collect::<Vec<_>>(), ["runs/slot-0/daemon.out"]);
        assert!(found.items.iter().all(|item| !item.text.contains("not the run's")));
    }

    #[test]
    fn another_machines_folder_gives_nothing() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let folder = context.run_folder(Some(slots::first()));
        fs::create_dir_all(&folder).unwrap();
        crate::run_files::plant_note(&folder, MACHINE_NOTE, "other");
        fs::write(folder.join("serial.log"), "another guest").unwrap();
        let mut found = Collection::default();
        collect(&context, &Machine::new("w11", "W11", "/d", "/v"), &request(true), &mut found);
        assert_eq!(found.names().count(), 0);
    }

    #[test]
    fn a_note_in_the_run_folder_itself_does_not_say_whose_run_it_is() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let folder = context.run_folder(Some(slots::first()));
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join(MACHINE_NOTE), "w11").unwrap();
        fs::write(folder.join("serial.log"), "a guest").unwrap();
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
        crate::run_files::plant_note(&folder, "run-machine.1", "w11");
        for name in ["daemon.out.1", "serial.log.1"] {
            fs::write(folder.join(name), "w11").unwrap();
        }
        let mut found = Collection::default();
        collect(&context, &Machine::new("w11", "W11", "/d", "/v"), &request(false), &mut found);
        let names: Vec<&str> = found.names().collect();
        assert_eq!(names, ["runs/slot-0/daemon.out.1"]);
    }

    #[test]
    fn with_no_run_folder_the_kept_copy_is_used_and_said() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let run = scratch.path("old-run");
        fs::create_dir_all(&run).unwrap();
        crate::run_files::plant_note(&run, MACHINE_NOTE, "w11");
        fs::write(run.join("daemon.out"), "daemon").unwrap();
        fs::write(run.join("serial.log"), "guest").unwrap();
        keep_run(&context, &run).unwrap();
        let mut found = Collection::default();
        collect(&context, &Machine::new("w11", "W11", "/d", "/v"), &request(false), &mut found);
        let names: Vec<&str> = found.names().collect();
        assert_eq!(names, ["runs/kept/daemon.out"], "the serial log follows the switch here too");
        assert!(found.has_run_logs);
        assert!(
            found
                .notes
                .iter()
                .any(|note| note.contains("the copy the app kept") && note.contains("started at"))
        );
    }

    #[test]
    fn with_no_logs_at_all_it_says_so() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let mut found = Collection::default();
        collect(&context, &Machine::new("w11", "W11", "/d", "/v"), &request(true), &mut found);
        assert!(!found.has_run_logs);
        assert!(found.notes.iter().any(|note| note.contains("no logs of this machine running")));
    }
}
