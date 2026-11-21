use super::*;
use crate::actions::desktop::DESKTOP_SCRIPT;
use crate::actions::tests::{machine, plain_task_context, with_vm_running};
use crate::actions::{STOP_WAIT_SECONDS, boot};
use crate::agent::GuestAgent;
use crate::failure::Failure;
use crate::machine::load;
use crate::testing::Scratch;
use crate::vm_host::stand_in::RecordingHost;
use std::sync::{Arc, Mutex};

const SCRIPT: &str = "ready Windows for NVMe";

/// A Windows guest with its desktop up and its GPU working, which answers this module's script as
/// told and every other script with nothing.
struct Guest {
    readied: &'static str,
    asked: Mutex<Vec<String>>,
}

impl GuestAgent for Guest {
    fn ask(&self, _port: u16, _token: &str, script: &str, _seconds: u64) -> String {
        self.asked.lock().unwrap().push(script.to_owned());
        let answer = if script == SCRIPT {
            self.readied
        } else if script == DESKTOP_SCRIPT {
            "3\n"
        } else if script.contains("Win32_VideoController") {
            "gpu working 1\n"
        } else {
            ""
        };
        answer.to_owned()
    }
}

impl Guest {
    fn times_asked(&self) -> usize {
        self.asked.lock().unwrap().iter().filter(|script| *script == SCRIPT).count()
    }
}

struct Rig {
    _scratch: Scratch,
    context: Context,
    guest: Arc<Guest>,
    host: RecordingHost,
}

/// A PC with one Windows machine on SATA whose guest says `readied` to the script.
fn rig(readied: &'static str) -> Rig {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    scratch.write(&format!("program/guest/{SCRIPT_NAME}"), SCRIPT);
    let guest = Arc::new(Guest { readied, asked: Mutex::default() });
    context.agent = guest.clone();
    fs::create_dir_all(context.run_folder(None)).unwrap();
    with_vm_running(&context);
    Rig { _scratch: scratch, context, guest, host: RecordingHost::default() }
}

impl Rig {
    fn task(&self) -> Task<'_> {
        let vm = machine(&self.context);
        crate::agent::make_token(&self.context, &vm).unwrap();
        Task::new(&self.context, &self.host, "start", vm)
    }

    fn disks_started_on(&self) -> Vec<bool> {
        self.host.plans.lock().unwrap().iter().map(|plan| plan.nvme).collect()
    }

    fn saved(&self, task: &Task) -> Machine {
        load(&self.context, &task.machine.id).unwrap()
    }
}

#[test]
fn a_windows_machine_on_sata_moves_to_nvme_in_the_same_start() {
    let rig = rig("ready\r\n");
    let mut task = rig.task();
    boot(&mut task).unwrap();
    assert_eq!(rig.disks_started_on(), [false, true], "started on SATA, then again on NVMe");
    assert_eq!(rig.host.said()[1], format!("stop {STOP_WAIT_SECONDS}"), "Windows shut down in between");
    let saved = rig.saved(&task);
    assert!(saved.nvme && !on_trial(&rig.context, &saved), "it stays on NVMe");
    assert!(!task.error.contains(BACK_ON_SATA));
    boot(&mut task).unwrap();
    assert_eq!(rig.disks_started_on(), [false, true, true], "and the next start is on NVMe from the first");
    assert_eq!(rig.guest.times_asked(), 1);
}

#[test]
fn a_guest_that_was_not_readied_stays_on_sata_and_is_asked_again_next_time() {
    let rig = rig("FAILED nvme: access denied\r\n");
    let mut task = rig.task();
    boot(&mut task).unwrap();
    boot(&mut task).unwrap();
    assert_eq!(rig.disks_started_on(), [false, false], "no VM was started again for it");
    assert_eq!(rig.guest.times_asked(), 2);
    assert!(!rig.saved(&task).nvme);
}

#[test]
fn a_start_on_nvme_that_fails_goes_back_to_sata_for_good_and_goes_on() {
    let rig = rig("ready\r\n");
    *rig.host.nvme_failure.lock().unwrap() =
        Some(Failure::new("The VM did not start: the agent never answered."));
    let mut task = rig.task();
    boot(&mut task).unwrap();
    assert_eq!(rig.disks_started_on(), [false, true, false]);
    assert!(task.error.starts_with(BACK_ON_SATA), "{}", task.error);
    let saved = rig.saved(&task);
    assert!(!saved.nvme && !on_trial(&rig.context, &saved));
    let why = fs::read_to_string(refused_file(&rig.context, &saved)).unwrap();
    assert!(why.contains("the agent never answered"), "{why}");
    boot(&mut task).unwrap();
    assert_eq!(rig.disks_started_on(), [false, true, false, false]);
    assert_eq!(rig.guest.times_asked(), 1, "a machine that went back isn't moved again");
}

#[test]
fn a_windows_that_had_to_be_turned_off_stays_on_sata_for_now() {
    let rig = rig("ready\r\n");
    *rig.host.stop_answer.lock().unwrap() = Some(Ok(Stopped::TurnedOff));
    let mut task = rig.task();
    boot(&mut task).unwrap();
    assert_eq!(rig.disks_started_on(), [false, false], "the deleted key may not have reached its disk");
    let saved = rig.saved(&task);
    assert!(!saved.nvme && !on_trial(&rig.context, &saved));
    assert!(!refused_file(&rig.context, &saved).exists(), "the next start tries again");
}

#[test]
fn a_first_start_on_nvme_left_over_from_a_stopped_app_is_still_a_trial() {
    let rig = rig("ready\r\n");
    *rig.host.nvme_failure.lock().unwrap() = Some(Failure::new("the agent never answered"));
    let mut task = rig.task();
    task.machine.nvme = true;
    task.machine.save(&rig.context).unwrap();
    fs::write(trial_file(&rig.context, &task.machine), "").unwrap();
    boot(&mut task).unwrap();
    assert_eq!(rig.disks_started_on(), [true, false]);
    assert!(refused_file(&rig.context, &rig.saved(&task)).exists());
}

#[test]
fn only_a_windows_machine_still_on_sata_that_never_went_back_is_moved() {
    let rig = rig("ready\r\n");
    let windows = machine(&rig.context);
    let not_due = [
        Machine { kind: Kind::Plain, ..windows.clone() },
        Machine { installing: true, ..windows.clone() },
        Machine { nvme: true, ..windows.clone() },
    ];
    for vm in not_due {
        assert!(!ready_windows(&mut Task::new(&rig.context, &rig.host, "start", vm)).unwrap());
    }
    fs::write(refused_file(&rig.context, &windows), "it did not boot\n").unwrap();
    assert!(!ready_windows(&mut Task::new(&rig.context, &rig.host, "start", windows)).unwrap());
    assert_eq!(rig.guest.times_asked(), 0);
}

#[test]
fn with_no_script_the_guest_is_not_asked() {
    let rig = rig("ready\r\n");
    fs::remove_file(rig.context.paths.guest_files().join(SCRIPT_NAME)).unwrap();
    let mut task = rig.task();
    assert!(!ready_windows(&mut task).unwrap());
    assert_eq!(rig.guest.times_asked(), 0);
}

#[test]
fn a_failed_start_on_nvme_that_is_no_trial_stays_a_failure() {
    let rig = rig("ready\r\n");
    *rig.host.nvme_failure.lock().unwrap() = Some(Failure::new("no daemon"));
    let mut task = rig.task();
    task.machine.nvme = true;
    assert_eq!(boot(&mut task).unwrap_err().0, "no daemon");
    assert_eq!(rig.disks_started_on(), [true]);
    assert!(task.machine.nvme, "a machine made on NVMe isn't moved off it");
}
