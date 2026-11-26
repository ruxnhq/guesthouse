//! Starting the program detached, finding it in `/proc`, and ending it, the way passt's are.

use super::settings::Settings;
use super::{PID_FILE, PROGRAM};
use crate::Result;
use crate::context::{Context, SLOT_VARIABLE};
use crate::daemon::detached;
use crate::failure::Failure;
use crate::log::open_for_appending;
use guesthouse_daemon::slot::Slot;
use guesthouse_jail::launch::start_time;
use rustix::process::{Pid, Signal, kill_process};
use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// How long it has to end after SIGTERM, which it answers within a moment, before it is killed.
const TERM_SECONDS: u64 = 5;
const KILL_SECONDS: u64 = 5;
const POLL: Duration = Duration::from_millis(50);

/// Starts the program for a machine, detached so that it outlives the caller, and returns at once.
///
/// It writes its pid file and makes its socket a moment later, so [`pid`] and [`super::ask`] may find
/// nothing for a few milliseconds. A caller that stays alive should `wait` on the child once the
/// program has ended, or the system keeps it as a zombie until the caller itself ends.
///
/// Nothing is started when one is already running in this slot. That is an error, as two could not
/// both have the socket.
pub fn start(context: &Context, slot: Slot, settings: &Settings) -> Result<Child> {
    if pid(context, Some(slot)).is_some() {
        return Err(Failure::new("The remote view is already running for this slot."));
    }
    context.make_run_folder(Some(slot))?;
    let log = open_for_appending(context)?;
    let child = detached(Command::new(context.paths.binary(PROGRAM)).args(settings.arguments()))
        .env(SLOT_VARIABLE, slot.number().to_string())
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .spawn()?;
    Ok(child)
}

/// The running program of a slot, by the pid and start time it noted itself, so that a pid the system
/// has given to something else since is not taken for it, and by its name.
pub fn pid(context: &Context, slot: Option<Slot>) -> Option<u32> {
    let noted = fs::read_to_string(context.run_folder(slot).join(PID_FILE)).ok()?;
    let (pid, started) = noted.trim().split_once(' ')?;
    let pid: u32 = pid.parse().ok()?;
    let process = context.processes.join(pid.to_string());
    let stat = fs::read_to_string(process.join("stat")).ok()?;
    let same = start_time(&stat).is_some_and(|time| time.to_string() == started);
    let alive = !crate::process_stat::has_ended(Some(&stat));
    (same && alive && is_the_program(&process)).then_some(pid)
}

fn is_the_program(process: &Path) -> bool {
    let words = fs::read(process.join("cmdline")).unwrap_or_default();
    let program = words.split(|byte| *byte == 0).next().unwrap_or_default();
    let name = Path::new(OsStr::new(&*String::from_utf8_lossy(program))).file_name().map(OsStr::to_owned);
    name.is_some_and(|name| name == PROGRAM)
}

/// Ends the slot's program and waits until it has gone, then clears what it left. It asks first, and
/// kills only the process it found by [`pid`]. Nothing running is not an error.
pub fn end(context: &Context, slot: Option<Slot>) {
    for (signal, seconds) in [(Signal::TERM, TERM_SECONDS), (Signal::KILL, KILL_SECONDS)] {
        let Some(pid) = pid(context, slot) else { break };
        if let Some(pid) = Pid::from_raw(pid as i32) {
            let _ = kill_process(pid, signal);
        }
        let end = context.clock.now() + Duration::from_secs(seconds);
        while pid_still(context, slot) && context.clock.now() < end {
            context.clock.sleep(POLL);
        }
    }
    if pid(context, slot).is_none() {
        let folder = context.run_folder(slot);
        let _ = fs::remove_file(folder.join(PID_FILE));
        let _ = fs::remove_file(super::socket_path(&folder));
    }
}

fn pid_still(context: &Context, slot: Option<Slot>) -> bool {
    pid(context, slot).is_some()
}

/// The program's own note of itself: its pid and start time, written once it is up. It refuses with
/// an error when the file already names a running program, so two cannot share a run folder.
pub fn write_pid_file(run_folder: &Path, processes: &Path) -> std::io::Result<()> {
    let file = run_folder.join(PID_FILE);
    if let Some((pid, started)) =
        fs::read_to_string(&file).ok().as_deref().and_then(|text| text.trim().split_once(' '))
    {
        let stat = fs::read_to_string(processes.join(pid).join("stat")).ok();
        let running = stat.as_deref().and_then(start_time).is_some_and(|time| time.to_string() == started)
            && !crate::process_stat::has_ended(stat.as_deref());
        if running {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "the remote view is already running here",
            ));
        }
    }
    let own = std::process::id();
    let stat = fs::read_to_string(processes.join(own.to_string()).join("stat"))?;
    let started = start_time(&stat).ok_or_else(|| std::io::Error::other("own start time unreadable"))?;
    fs::write(file, format!("{own} {started}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process_stat::stand_in::write_stat;
    use crate::slots;
    use crate::testing::{Scratch, scratch_context};

    fn fake_program(context: &Context, pid: u32, started: u64, argv0: &str) {
        write_stat(context, pid, 'S', started);
        fs::write(context.processes.join(pid.to_string()).join("cmdline"), format!("{argv0}\0m\0")).unwrap();
    }

    fn note(context: &Context, text: &str) {
        context.make_run_folder(None).unwrap();
        fs::write(context.run_folder(None).join(PID_FILE), text).unwrap();
    }

    #[test]
    fn the_program_is_found_by_its_noted_pid_and_start_time() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        fake_program(&context, 4242, 987_654, "/usr/lib/guesthouse/guesthouse-remote-view");
        note(&context, "4242 987654\n");
        assert_eq!(pid(&context, None), Some(4242));
    }

    #[test]
    fn a_pid_the_system_gave_to_something_else_is_not_the_program() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        fake_program(&context, 4242, 987_654, "/usr/lib/guesthouse/guesthouse-remote-view");
        note(&context, "4242 111\n");
        assert_eq!(pid(&context, None), None, "it started at another time");
        fake_program(&context, 4242, 987_654, "/usr/bin/sleep");
        note(&context, "4242 987654\n");
        assert_eq!(pid(&context, None), None, "same pid and time, another program");
        fake_program(&context, 4242, 987_654, "/usr/lib/guesthouse/guesthouse-remote-view");
        write_stat(&context, 4242, 'Z', 987_654);
        assert_eq!(pid(&context, None), None, "a zombie has ended");
    }

    #[test]
    fn no_note_or_a_broken_one_finds_nothing() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        assert_eq!(pid(&context, None), None);
        for text in ["", "4242", "x y", "4242 \n"] {
            note(&context, text);
            assert_eq!(pid(&context, None), None, "{text:?}");
        }
    }

    #[test]
    fn a_slot_is_found_in_its_own_run_folder() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        fake_program(&context, 77, 5, "guesthouse-remote-view");
        context.make_run_folder(Some(slots::number(1))).unwrap();
        fs::write(context.run_folder(Some(slots::number(1))).join(PID_FILE), "77 5\n").unwrap();
        assert_eq!(pid(&context, None), None);
        assert_eq!(pid(&context, Some(slots::number(1))), Some(77));
    }

    #[test]
    fn a_second_program_is_refused_while_the_first_runs() {
        let scratch = Scratch::new();
        let folder = scratch.path("run");
        fs::create_dir_all(&folder).unwrap();
        // The first is this test's own process, as /proc has it.
        write_pid_file(&folder, Path::new("/proc")).unwrap();
        let error = write_pid_file(&folder, Path::new("/proc")).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        // One that has gone is replaced.
        fs::write(folder.join(PID_FILE), "999999 1\n").unwrap();
        write_pid_file(&folder, Path::new("/proc")).unwrap();
    }

    #[test]
    fn ending_with_nothing_running_is_quiet_and_clears_what_was_left() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        note(&context, "4242 987654\n");
        fs::write(super::super::socket_path(&context.run_folder(None)), "").unwrap();
        end(&context, None);
        assert!(!context.run_folder(None).join(PID_FILE).exists());
        assert!(!super::super::socket_path(&context.run_folder(None)).exists());
    }

    #[test]
    fn it_is_started_with_the_settings_as_arguments() {
        let scratch = Scratch::new();
        let mut context = scratch_context(&scratch);
        let marker = scratch.path("said");
        let program = scratch.program(PROGRAM, &format!("#!/bin/sh\necho \"$@\" > {}\n", marker.display()));
        context.paths.binaries = program.parent().unwrap().to_owned();
        let settings = Settings {
            machine: "win11".into(),
            run_folder: context.run_folder(None),
            loopback_port: 19065,
            listen: Some("127.0.0.1:7800".parse().unwrap()),
            qemu: None,
            idle_timeout: None,
            max_clients: None,
            name: None,
            fingerprint: None,
        };
        let mut child = start(&context, slots::first(), &settings).unwrap();
        assert!(child.wait().unwrap().success());
        let said = fs::read_to_string(marker).unwrap();
        assert_eq!(said.trim().split(' ').collect::<Vec<_>>()[..1], ["win11"]);
        assert!(said.contains("--loopback-port 19065") && said.contains("--listen 127.0.0.1:7800"));
    }
}
