use super::*;
use crate::agent::make_token;
use crate::agent::stand_in::ScriptedAgent;
use crate::clock::Clock;
use crate::disk_image::backing::BackingFile;
use crate::machine::Kind;
use crate::slots;
use crate::testing::{Scratch, scratch_context};
use crate::vm_args::{Boot, OUTPUT_FILE};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// How long a test's pretend programs last if nothing stops them.
const PRETEND_LIFE_SECONDS: u32 = 60;
const NOTICE_WITHIN: Duration = Duration::from_secs(1);

/// Time that moves when something sleeps, so a test doesn't sit out the wait for an agent. A real
/// process still needs a moment, so each sleep gives it a little of the real thing.
#[derive(Default)]
struct QuickClock(AtomicU64);

impl Clock for QuickClock {
    fn now(&self) -> Duration {
        Duration::from_millis(self.0.load(Ordering::Relaxed))
    }

    fn sleep(&self, time: Duration) {
        self.0.fetch_add(time.as_millis() as u64, Ordering::Relaxed);
        thread::sleep(Duration::from_millis(1));
    }
}

/// A daemon that does nothing but make its sockets, and a QEMU that does nothing but note its pid.
/// The context's `/proc` is empty, so neither can be found by looking there, as with a QEMU in the
/// moment after its spawn.
fn pretend_programs(scratch: &Scratch, context: &mut crate::context::Context) -> (PathBuf, PathBuf) {
    context.paths.binaries = scratch.path("bin");
    let (daemon_pid, qemu_pid) = (scratch.path("daemon-pid"), scratch.path("qemu-pid"));
    let daemon = format!(
        "#!/bin/sh\necho $$ > {}\ntouch \"$1/run/vgpu.sock\" \"$1/run/global.sock\"\nexec sleep {PRETEND_LIFE_SECONDS}\n",
        daemon_pid.display()
    );
    scratch.program("bin/guesthouse-daemon", &daemon);
    // It answers the question about its user network as a QEMU of Guesthouse's does, and stops.
    let qemu = format!(
        "#!/bin/sh\ncase \"$*\" in *host-loopback=ask*) exit 1;; esac\necho $$ > {}\nexec sleep {PRETEND_LIFE_SECONDS}\n",
        qemu_pid.display()
    );
    context.paths.qemu = scratch.program("not-qemu", &qemu);
    context.clock = Arc::new(QuickClock::default());
    (daemon_pid, qemu_pid)
}

fn read_pid(file: &Path) -> Option<u32> {
    fs::read_to_string(file).ok()?.trim().parse().ok()
}

/// The pid of a pretend QEMU that's still going. One killed early may never have written its pid.
fn left_running(file: &Path) -> Option<u32> {
    let end = Instant::now() + NOTICE_WITHIN;
    while Instant::now() < end {
        if let Some(pid) = read_pid(file) {
            return is_running(pid).then_some(pid);
        }
        thread::sleep(Duration::from_millis(10));
    }
    None
}

/// A zombie counts as gone, as only its parent can clear it.
fn is_running(pid: u32) -> bool {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    let state = stat.rsplit(')').next().and_then(|rest| rest.split_whitespace().next());
    state.is_some_and(|state| state != "Z")
}

fn end_pretend_program(file: &Path) {
    if let Some(pid) = read_pid(file) {
        let _ = Command::new("kill").args(["-KILL", &pid.to_string()]).status();
    }
}

/// The scratch context offers no jail, so a GPU machine here may start without one.
fn gpu_machine() -> Machine {
    Machine { start_without_jail: true, ..Machine::new("vm", "VM", "/d", "/v") }
}

fn windows_plan(context: &crate::context::Context) -> (Machine, VmPlan) {
    let machine = gpu_machine();
    machine.save(context).unwrap();
    make_token(context, &machine).unwrap();
    let plan = VmPlan::new(context, &machine, Boot::Gpu, slots::first(), 0);
    (machine, plan)
}

#[test]
fn a_qemu_that_proc_cannot_show_yet_is_still_a_qemu() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    let (daemon, qemu) = pretend_programs(&scratch, &mut context);
    context.agent = Arc::new(ScriptedAgent::answering(&["", "", "up"]));
    let (machine, plan) = windows_plan(&context);
    let started = LocalHost::new(&context).start(&plan, &machine);
    end_pretend_program(&qemu);
    end_pretend_program(&daemon);
    assert_eq!(started, Ok(()));
}

#[test]
fn a_start_that_gives_up_leaves_no_qemu_running() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    let (daemon, qemu) = pretend_programs(&scratch, &mut context);
    context.agent = Arc::new(ScriptedAgent::answering(&[]));
    let (machine, plan) = windows_plan(&context);
    let started = LocalHost::new(&context).start(&plan, &machine);
    let left = left_running(&qemu);
    end_pretend_program(&qemu);
    end_pretend_program(&daemon);
    assert!(started.unwrap_err().0.contains("the agent never answered"));
    assert_eq!(left, None, "QEMU was left running");
}

#[test]
fn a_daemon_that_stops_during_the_boot_ends_the_start_and_qemu() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    let (_, qemu) = pretend_programs(&scratch, &mut context);
    let daemon =
        "#!/bin/sh\ntouch \"$1/run/vgpu.sock\" \"$1/run/global.sock\"\nsleep 0.05\necho 'lost the card'\n";
    scratch.program("bin/guesthouse-daemon", daemon);
    context.agent = Arc::new(ScriptedAgent::answering(&[]));
    let (machine, plan) = windows_plan(&context);
    let started = LocalHost::new(&context).start(&plan, &machine);
    let left = left_running(&qemu);
    end_pretend_program(&qemu);
    let error = started.unwrap_err();
    assert!(error.0.contains("the daemon stopped: lost the card"), "{error}");
    assert_eq!(left, None, "QEMU was left running");
}

#[test]
fn a_plain_qemu_whose_monitor_never_answers_is_not_left_running() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    let (_, qemu) = pretend_programs(&scratch, &mut context);
    let host = LocalHost::new(&context);
    let machine = Machine { kind: Kind::Plain, ..Machine::new("vm", "VM", "/d", "/v") };
    let plan = VmPlan::new(&context, &machine, Boot::Plain { iso: String::new() }, slots::first(), 0);
    let started = host.start(&plan, &machine);
    let left = left_running(&qemu);
    end_pretend_program(&qemu);
    assert!(started.unwrap_err().0.contains("monitor never answered"));
    assert_eq!(left, None, "QEMU was left running");
}

#[test]
fn a_second_instance_never_reaches_what_finds_vms_by_program() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    context.paths.own_run = Some(scratch.path("second"));
    let host = LocalHost::new(&context);
    let machine = Machine::new("vm", "VM", "/d", "/v");
    assert!(host.stop(&machine, 1).unwrap_err().0.contains("leaves GPU-PV alone"));
    assert!(host.release().unwrap_err().0.contains("leaves GPU-PV alone"));
    let plan = VmPlan::new(&context, &machine, Boot::Gpu, slots::first(), 0);
    assert!(host.start(&plan, &machine).unwrap_err().0.contains("leaves GPU-PV alone"));
}

#[test]
fn a_qemu_that_exits_at_once_says_why() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    context.paths.qemu = scratch.program("not-qemu", "#!/bin/sh\necho 'could not open disk' >&2\nexit 1\n");
    let host = LocalHost::new(&context);
    let machine = Machine { kind: Kind::Plain, ..Machine::new("vm", "VM", "/d", "/v") };
    let plan = VmPlan::new(&context, &machine, Boot::Plain { iso: String::new() }, slots::first(), 0);
    let error = host.start(&plan, &machine).unwrap_err();
    assert_eq!(error.0, "The VM did not start. could not open disk");
}

#[test]
fn a_daemon_that_stops_says_why_and_no_qemu_is_started() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    context.paths.binaries = scratch.path("bin");
    let marker = scratch.path("qemu-ran");
    context.paths.qemu = scratch.program("not-qemu", &format!("#!/bin/sh\ntouch {}\n", marker.display()));
    scratch.program("bin/guesthouse-daemon", "#!/bin/sh\necho 'no card here'\nexit 1\n");
    let host = LocalHost::new(&context);
    let machine = gpu_machine();
    let plan = VmPlan::new(&context, &machine, Boot::Gpu, slots::first(), 0);
    let error = host.start(&plan, &machine).unwrap_err();
    assert!(error.0.contains("the daemon stopped: no card here"), "{error}");
    assert!(!marker.exists());
}

#[test]
fn release_frees_memory_only_when_no_other_vm_has_it() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let memory = context.paths.shared_file("ram", slots::first());
    fs::write(&memory, "").unwrap();
    crate::processes::stand_in::fake_process(
        &context,
        300,
        &["/other/qemu"],
        &[],
        &memory.display().to_string(),
    );
    let host = LocalHost::new(&context);
    host.release().unwrap();
    assert!(memory.exists());
    fs::remove_dir_all(context.processes.join("300")).unwrap();
    host.release().unwrap();
    assert!(!memory.exists());
}

#[test]
fn a_passt_left_running_is_ended_at_a_stop() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    context.processes = "/proc".into();
    context.clock = Arc::new(QuickClock::default());
    context.make_run_folder(None).unwrap();
    let pid_file = context.notes_folder(None).join(crate::vm_args::PASST_PID_FILE);
    let mut passt = Command::new("sleep").arg(PRETEND_LIFE_SECONDS.to_string()).spawn().unwrap();
    let stat = fs::read_to_string(format!("/proc/{}/stat", passt.id())).unwrap();
    let started = guesthouse_jail::launch::start_time(&stat).unwrap();
    fs::write(&pid_file, format!("{} {started}\n", passt.id())).unwrap();
    let host = LocalHost::new(&context);
    let plain = Machine { kind: Kind::Plain, ..Machine::new("vm", "VM", "/d", "/v") };
    host.stop(&plain, 1).unwrap();
    let ended = passt.try_wait().unwrap().is_some() || {
        thread::sleep(NOTICE_WITHIN);
        passt.try_wait().unwrap().is_some()
    };
    if !ended {
        let _ = passt.kill();
    }
    assert!(ended, "the stop ended it");
}

#[test]
fn last_words_are_the_end_of_the_output() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    assert_eq!(last_words(&context), "");
    let folder = context.run_folder(None);
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join(OUTPUT_FILE), format!("{}end\n", "é".repeat(400))).unwrap();
    let words = last_words(&context);
    assert_eq!(words.chars().count(), 300);
    assert!(words.ends_with("end"));
}

#[test]
fn qemu_with_no_display_says_what_to_install() {
    assert!(
        super::local::start_failure("qemu-system-x86_64: Display 'dbus' is not available.")
            .contains("qemu-ui-dbus")
    );
    assert_eq!(super::local::start_failure("it broke"), "The VM did not start. it broke");
}

/// A NixOS tester saw only "No such file or directory (os error 2)", which didn't say what was missing.
#[test]
fn a_program_that_cant_be_run_is_named() {
    let missing = std::io::Error::from(std::io::ErrorKind::NotFound);
    assert_eq!(super::local::not_run("bwrap", &missing).0, "The VM did not start: bwrap wasn't found.");
    let refused = std::io::Error::from_raw_os_error(13);
    let said = super::local::not_run("/p/guesthouse-jail", &refused).0;
    assert!(said.starts_with("The VM did not start: /p/guesthouse-jail couldn't be run. "), "{said}");
}

#[test]
fn no_qemu_at_all_says_so() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    context.paths.qemu = scratch.path("not-installed");
    let host = LocalHost::new(&context);
    let machine = Machine { kind: Kind::Plain, ..Machine::new("vm", "VM", "/d", "/v") };
    let plan = VmPlan::new(&context, &machine, Boot::Plain { iso: String::new() }, slots::first(), 0);
    assert!(host.start(&plan, &machine).unwrap_err().0.contains("isn't installed"));
}

#[test]
fn memory_that_doesnt_fit_stops_a_start_before_anything_runs() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    context.reserve_memory = |_, _, _| Err(crate::Failure::new("no room for it"));
    context.paths.binaries = scratch.path("bin");
    let marker = scratch.path("ran");
    let daemon = format!("#!/bin/sh\ntouch {}\n", marker.display());
    scratch.program("bin/guesthouse-daemon", &daemon);
    context.paths.qemu = scratch.program("not-qemu", &daemon);
    let host = LocalHost::new(&context);
    let machine = gpu_machine();
    let plan = VmPlan::new(&context, &machine, Boot::Gpu, slots::first(), 0);
    assert_eq!(host.start(&plan, &machine).unwrap_err().0, "no room for it");
    assert!(!marker.exists(), "neither the daemon nor QEMU may start");
}

/// Skips the launcher's words and runs what comes after, as the real one does once it's jailed.
const PRETEND_LAUNCHER: &str = "#!/bin/sh\nwhile [ \"$1\" != -- ]; do shift; done\nshift\nexec \"$@\"\n";

fn a_kernel_with_landlock(_: &VmPlan) -> crate::jailing::Offer {
    crate::jailing::Offer {
        support: guesthouse_jail::Support { landlock_abi: 10, seccomp: true },
        ..crate::jailing::Offer::nothing()
    }
}

#[test]
fn a_jailed_start_with_a_missing_backing_file_says_so_and_starts_nothing() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    let (daemon, qemu) = pretend_programs(&scratch, &mut context);
    scratch.program("bin/guesthouse-jail", PRETEND_LAUNCHER);
    context.jail_offer = a_kernel_with_landlock;
    let (base, disk) = (scratch.path("base.qcow2"), scratch.path("disk.qcow2"));
    let create = |words: &[&str]| assert!(Command::new("qemu-img").args(words).status().unwrap().success());
    create(&["create", "-q", "-f", "qcow2", base.to_str().unwrap(), "1G"]);
    create(&["create", "-q", "-f", "qcow2", "-b", "base.qcow2", "-F", "qcow2", disk.to_str().unwrap()]);
    fs::remove_file(&base).unwrap();
    let named = BackingFile { path: base.display().to_string(), format: "qcow2".into() };
    let machine =
        Machine { backing_files: vec![named], ..Machine::new("vm", "VM", disk.to_str().unwrap(), "/v") };
    let plan = VmPlan::new(&context, &machine, Boot::Gpu, slots::first(), 0);
    let refused = LocalHost::new(&context).start(&plan, &machine).unwrap_err();
    assert_eq!(
        refused.0,
        format!("The VM did not start, as its disk is built on {}, which is missing.", base.display())
    );
    assert_eq!((read_pid(&daemon), read_pid(&qemu)), (None, None));
}

#[test]
fn a_jailed_start_runs_qemu_through_the_launcher_with_a_clean_environment() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    let (daemon, qemu) = pretend_programs(&scratch, &mut context);
    let seen = scratch.path("qemu-environment");
    let qemu_program = format!(
        "#!/bin/sh\ncase \"$*\" in *host-loopback=ask*) exit 1;; esac\nenv > {}\necho $$ > {}\nexec sleep {PRETEND_LIFE_SECONDS}\n",
        seen.display(),
        qemu.display()
    );
    context.paths.qemu = scratch.program("not-qemu", &qemu_program);
    scratch.program("bin/guesthouse-jail", PRETEND_LAUNCHER);
    context.jail_offer = a_kernel_with_landlock;
    context.agent = Arc::new(ScriptedAgent::answering(&["up"]));
    let machine = Machine::new("vm", "VM", "/d", "/v");
    machine.save(&context).unwrap();
    make_token(&context, &machine).unwrap();
    let plan = VmPlan::new(&context, &machine, Boot::Gpu, slots::first(), 0);
    let started = LocalHost::new(&context).start(&plan, &machine);
    // The pretend agent answers at once, so QEMU may not have got far yet.
    left_running(&qemu);
    let environment = fs::read_to_string(&seen).unwrap_or_default();
    end_pretend_program(&qemu);
    end_pretend_program(&daemon);
    assert_eq!(started, Ok(()));
    assert!(environment.contains("PATH=/usr/bin"), "{environment}");
    assert!(!environment.contains("HOME="), "the app's environment stays behind: {environment}");
    let note = crate::jailing::read_note(&plan.run_folder).unwrap();
    assert!(note.starts_with("Jailed, without namespaces"), "{note}");
}

#[test]
fn a_machine_that_cant_be_jailed_doesnt_start_unless_allowed() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    let (daemon, qemu) = pretend_programs(&scratch, &mut context);
    let machine = Machine::new("vm", "VM", "/d", "/v");
    let plan = VmPlan::new(&context, &machine, Boot::Gpu, slots::first(), 0);
    let refused = LocalHost::new(&context).start(&plan, &machine).unwrap_err();
    assert!(refused.0.contains("can't be jailed"), "{refused}");
    assert_eq!((read_pid(&daemon), read_pid(&qemu)), (None, None), "nothing ran");
}

#[test]
fn a_machine_allowed_to_start_unjailed_tells_the_daemon_and_its_row() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    let (daemon, qemu) = pretend_programs(&scratch, &mut context);
    let seen = scratch.path("daemon-environment");
    let daemon_program = format!(
        "#!/bin/sh\nenv > {}\necho $$ > {}\ntouch \"$1/run/vgpu.sock\" \"$1/run/global.sock\"\nexec sleep {PRETEND_LIFE_SECONDS}\n",
        seen.display(),
        daemon.display()
    );
    scratch.program("bin/guesthouse-daemon", &daemon_program);
    context.agent = Arc::new(ScriptedAgent::answering(&["up"]));
    let (machine, plan) = windows_plan(&context);
    let started = LocalHost::new(&context).start(&plan, &machine);
    left_running(&qemu);
    end_pretend_program(&qemu);
    end_pretend_program(&daemon);
    assert_eq!(started, Ok(()));
    assert!(fs::read_to_string(&seen).unwrap().contains("GUESTHOUSE_JAIL=off"));
    let note = crate::jailing::read_note(&plan.run_folder).unwrap();
    assert!(note.starts_with("Not jailed: "), "{note}");
}

#[test]
fn a_machine_with_its_sound_off_gets_no_pipewire_and_no_sockets_of_its_own() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    let (daemon, qemu) = pretend_programs(&scratch, &mut context);
    let said = scratch.path("launcher-words");
    let launcher = format!("#!/bin/sh\necho \"$@\" > {}\n{}", said.display(), &PRETEND_LAUNCHER[10..]);
    scratch.program("bin/guesthouse-jail", &launcher);
    context.jail_offer = a_kernel_with_landlock;
    context.agent = Arc::new(ScriptedAgent::answering(&["up"]));
    let machine = Machine { sound: false, ..Machine::new("vm", "VM", "/d", "/v") };
    machine.save(&context).unwrap();
    make_token(&context, &machine).unwrap();
    let plan = VmPlan::new(&context, &machine, Boot::Gpu, slots::first(), 0);
    let started = LocalHost::new(&context).start(&plan, &machine);
    left_running(&qemu);
    end_pretend_program(&qemu);
    end_pretend_program(&daemon);
    assert_eq!(started, Ok(()));
    let words = fs::read_to_string(&said).unwrap();
    assert!(words.contains("--no-new-unix-sockets"), "{words}");
    assert!(words.contains("none,id=silent") && !words.contains("pipewire"), "{words}");
}

/// A pretend QEMU that notes what it has open at the throwaway run's first number.
fn noting_qemu(scratch: &Scratch, context: &mut crate::context::Context) -> (PathBuf, PathBuf, PathBuf) {
    let (daemon, qemu) = pretend_programs(scratch, context);
    let noted = scratch.path("overlay-seen");
    let script = format!(
        "#!/bin/sh\necho $$ > {}\nreadlink /proc/$$/fd/50 >> {}\nexec sleep {PRETEND_LIFE_SECONDS}\n",
        qemu.display(),
        noted.display()
    );
    context.paths.qemu = scratch.program("not-qemu", &script);
    (daemon, qemu, noted)
}

#[test]
fn a_throwaway_run_keeps_its_overlay_for_the_task_and_the_next_task_gets_a_new_one() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    let (daemon, qemu, noted) = noting_qemu(&scratch, &mut context);
    context.agent = Arc::new(ScriptedAgent::answering(&["up", "up", "up"]));
    let disk = scratch.path("machine-disk/disk.qcow2");
    fs::create_dir_all(disk.parent().unwrap()).unwrap();
    assert!(
        Command::new("qemu-img")
            .args(["create", "-q", "-f", "qcow2"])
            .arg(&disk)
            .arg("1G")
            .status()
            .unwrap()
            .success()
    );
    let vars = scratch.write("machine-disk/vars.fd", "settings").display().to_string();
    let mut machine = Machine { disk: disk.display().to_string(), vars, ..gpu_machine() };
    machine.save(&context).unwrap();
    make_token(&context, &machine).unwrap();
    crate::throwaway::set_throwaway(&context, &mut machine, true).unwrap();
    crate::throwaway::mark_for_deletion(&machine.folder(&context)).unwrap();
    let plan = VmPlan::new(&context, &machine, Boot::Gpu, slots::first(), 0);
    let seen = |count: usize| {
        let end = Instant::now() + NOTICE_WITHIN;
        while Instant::now() < end {
            let lines: Vec<String> =
                fs::read_to_string(&noted).unwrap_or_default().lines().map(str::to_owned).collect();
            if lines.len() >= count {
                return lines;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("QEMU never noted its overlay");
    };
    let task = LocalHost::new(&context);
    task.start(&plan, &machine).unwrap();
    let mark = machine.folder(&context).join(crate::throwaway::DELETE_MARK);
    assert_eq!(fs::read_to_string(mark).unwrap(), "started", "its VM was up");
    // Each pretend QEMU is left to note its overlay before it is ended: on a slow PC it could be
    // ended first, and its line would never come.
    seen(1);
    end_pretend_program(&qemu);
    end_pretend_program(&daemon);
    task.start(&plan, &machine).unwrap();
    seen(2);
    end_pretend_program(&qemu);
    end_pretend_program(&daemon);
    // The next task of the machine comes once this one has ended.
    drop(task);
    LocalHost::new(&context).start(&plan, &machine).unwrap();
    let lines = seen(3);
    end_pretend_program(&qemu);
    end_pretend_program(&daemon);
    assert!(lines.iter().all(|line| line.ends_with(" (deleted)")), "{lines:?}");
    assert_eq!(lines[0], lines[1], "one task, one overlay");
    assert_ne!(lines[1], lines[2], "a new task, a new one");
}

/// QEMU opens what a disk's header names, jailed or not, so a start checks it first.
#[test]
fn a_plain_start_refuses_a_disk_built_on_a_file_its_settings_dont_name() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    let marker = scratch.path("qemu-ran");
    context.paths.qemu = scratch.program("not-qemu", &format!("#!/bin/sh\ntouch {}\n", marker.display()));
    let other = scratch.write("elsewhere/notes.txt", "not the machine's");
    let disk = scratch.path("machines/vm/disk.qcow2");
    fs::create_dir_all(disk.parent().unwrap()).unwrap();
    let qemu_img = |words: &[&str]| assert!(Command::new("qemu-img").args(words).status().unwrap().success());
    qemu_img(&["create", "-q", "-f", "qcow2", disk.to_str().unwrap(), "1G"]);
    qemu_img(&[
        "rebase",
        "-u",
        "-f",
        "qcow2",
        "-F",
        "raw",
        "-b",
        other.to_str().unwrap(),
        disk.to_str().unwrap(),
    ]);
    let machine = Machine { kind: Kind::Plain, ..Machine::new("vm", "VM", disk.to_str().unwrap(), "/v") };
    let plan = VmPlan::new(&context, &machine, Boot::Plain { iso: String::new() }, slots::first(), 0);
    let error = LocalHost::new(&context).start(&plan, &machine).unwrap_err();
    assert!(error.0.contains(&other.display().to_string()), "{error}");
    assert!(!marker.exists(), "QEMU never ran");
}

/// QEMU and the daemon may write anything in the run folder, so a note there is never what the app
/// signals by.
#[test]
fn a_passt_note_in_the_run_folder_is_never_signalled() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    context.processes = "/proc".into();
    context.clock = Arc::new(QuickClock::default());
    context.make_run_folder(None).unwrap();
    let mut other = Command::new("sleep").arg(PRETEND_LIFE_SECONDS.to_string()).spawn().unwrap();
    let stat = fs::read_to_string(format!("/proc/{}/stat", other.id())).unwrap();
    let started = guesthouse_jail::launch::start_time(&stat).unwrap();
    let in_run_folder = context.run_folder(None).join(crate::vm_args::PASST_PID_FILE);
    fs::write(&in_run_folder, format!("{} {started}\n", other.id())).unwrap();
    let plain = Machine { kind: Kind::Plain, ..Machine::new("vm", "VM", "/d", "/v") };
    LocalHost::new(&context).stop(&plain, 1).unwrap();
    thread::sleep(Duration::from_millis(100));
    let left_alone = other.try_wait().unwrap().is_none();
    let _ = other.kill();
    let _ = other.wait();
    assert!(left_alone, "the process was signalled");
}

#[test]
fn a_qemu_without_the_host_loopback_setting_is_asked_and_never_started() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    let marker = scratch.path("qemu-ran");
    let qemu = format!(
        "#!/bin/sh\ncase \"$*\" in *host-loopback=ask*) echo \"Invalid parameter 'host-loopback'\" >&2; exit 1;; esac\n\
         touch {}\n",
        marker.display()
    );
    context.paths.qemu = scratch.program("not-qemu", &qemu);
    let host = LocalHost::new(&context);
    let machine = Machine { kind: Kind::Plain, ..Machine::new("vm", "VM", "/d", "/v") };
    let plan = VmPlan::new(&context, &machine, Boot::Plain { iso: String::new() }, slots::first(), 0);
    let error = host.start(&plan, &machine).unwrap_err();
    assert!(error.0.contains("the QEMU that comes with Guesthouse"), "{error}");
    assert!(!marker.exists(), "the machine's QEMU was started");
}
