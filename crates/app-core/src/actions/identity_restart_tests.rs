use super::*;
use crate::actions::desktop::{BOOT_LINE, SERIAL_FILE};
use crate::actions::identity::after_failed_gpu;
use crate::actions::tests::{machine, plain_task_context};
use crate::agent::GuestAgent;
use crate::agent::stand_in::ScriptedAgent;
use crate::context::Context;
use crate::failure::Failure;
use crate::gpu_identity::{IdentityState, Shown};
use crate::machine::Machine;
use crate::testing::Scratch;
use crate::vm_host::stand_in::RecordingHost;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

const DESKTOP_UP: &str = "3\n";
const GPU_UP: &str = "gpu working 1\n";

/// A guest that answers as scripted, and whose firmware boots again when Windows is asked to restart.
struct Guest {
    scripted: ScriptedAgent,
    serial: PathBuf,
    restarts: bool,
}

impl GuestAgent for Guest {
    fn ask(&self, port: u16, token: &str, script: &str, seconds: u64) -> String {
        if self.restarts && script == RESTART_SCRIPT {
            let mut serial = OpenOptions::new().append(true).create(true).open(&self.serial).unwrap();
            serial.write_all(BOOT_LINE).unwrap();
        }
        self.scripted.ask(port, token, script, seconds)
    }
}

struct Rig {
    context: Context,
    guest: Arc<Guest>,
    machine: Machine,
}

impl Rig {
    fn new(scratch: &Scratch, state: IdentityState, restarts: bool, answers: &[&str]) -> Self {
        let mut context = plain_task_context(scratch);
        let folder = context.run_folder(None);
        fs::create_dir_all(&folder).unwrap();
        let serial = folder.join(SERIAL_FILE);
        fs::write(&serial, BOOT_LINE).unwrap();
        let guest = Arc::new(Guest { scripted: ScriptedAgent::answering(answers), serial, restarts });
        context.agent = guest.clone();
        let machine = machine(&context);
        crate::agent::make_token(&context, &machine).unwrap();
        write_state(&context, &machine, &state).unwrap();
        Self { context, guest, machine }
    }

    fn start(&self) -> (Result<Gpu>, String) {
        let host = RecordingHost::default();
        let mut task = Task::new(&self.context, &host, "start", self.machine.clone());
        let result = restart_after_switch(&mut task);
        (result, task.error)
    }

    fn asked(&self) -> Vec<String> {
        self.guest.scripted.requests.lock().unwrap().iter().map(|(_, _, script)| script.clone()).collect()
    }

    fn state(&self) -> IdentityState {
        read_state(&self.context, &self.machine)
    }
}

fn switching() -> IdentityState {
    IdentityState { shown: Shown::Maker, restart_due: true, ..IdentityState::default() }
}

#[test]
fn only_the_first_start_on_nvidias_number_restarts_windows() {
    assert!(switching().restarts_windows());
    let settled = IdentityState { shown: Shown::Maker, ..IdentityState::default() };
    assert!(!settled.restarts_windows());
    assert!(!IdentityState::default().restarts_windows());
    let back = IdentityState { shown: Shown::Microsoft, restart_due: true, ..IdentityState::default() };
    assert!(!back.restarts_windows(), "Microsoft's number never needs it");
}

#[test]
fn the_switching_start_restarts_windows_and_waits_for_its_desktop_and_gpu() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, switching(), true, &["restarting\r\n", DESKTOP_UP, GPU_UP]);
    let (result, error) = rig.start();
    assert_eq!(result, Ok(Gpu::Working));
    assert_eq!(error, "");
    let asked = rig.asked();
    assert_eq!(asked.len(), 3);
    assert_eq!(asked[0], RESTART_SCRIPT);
    assert!(asked[1].contains("explorer"), "then the desktop is waited for");
    assert!(asked[2].contains("Win32_VideoController"), "and the GPU checked again");
    assert_eq!(rig.state(), IdentityState { shown: Shown::Maker, ..IdentityState::default() });

    let (result, _) = rig.start();
    assert_eq!(result, Ok(Gpu::Working));
    assert_eq!(rig.asked().len(), 3, "the next start doesn't restart it again");
}

#[test]
fn a_start_on_nvidias_number_that_has_settled_asks_nothing() {
    let scratch = Scratch::new();
    let settled = IdentityState { shown: Shown::Maker, ..IdentityState::default() };
    let rig = Rig::new(&scratch, settled.clone(), true, &[]);
    assert_eq!(rig.start(), (Ok(Gpu::Working), String::new()));
    assert!(rig.asked().is_empty());
    assert_eq!(rig.state(), settled);
}

#[test]
fn a_fall_back_to_microsofts_number_does_not_restart_windows() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, switching(), true, &[]);
    let host = RecordingHost::default();
    let task = Task::new(&rig.context, &host, "start", rig.machine.clone());
    after_failed_gpu(&task, Failure::new("no GPU"));
    assert_eq!(rig.state().shown, Shown::Microsoft);
    assert_eq!(rig.start(), (Ok(Gpu::Working), String::new()));
    assert!(rig.asked().is_empty());
}

#[test]
fn a_windows_that_does_not_restart_is_said_and_not_asked_again() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, switching(), false, &[]);
    let (result, error) = rig.start();
    assert_eq!(result, Ok(Gpu::Working));
    assert_eq!(error, DID_NOT_RESTART);
    assert_eq!(rig.asked(), [RESTART_SCRIPT]);
    assert!(!rig.state().restart_due);
}

#[test]
fn a_desktop_that_does_not_come_back_fails_the_start() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, switching(), true, &["restarting\r\n"]);
    let (result, _) = rig.start();
    assert_eq!(result, Err(Failure::new(DESKTOP_GONE)));
    assert!(!rig.state().restart_due, "the next start goes on without it");
}

#[test]
fn a_restart_that_lands_at_the_sign_in_screen_goes_on_with_the_gpu_check() {
    let scratch = Scratch::new();
    let mut answers = vec!["restarting\r\n"];
    answers.extend(["1\n"; 60]);
    answers.extend(["nobody signed in\r\n", GPU_UP]);
    let rig = Rig::new(&scratch, switching(), true, &answers);
    let host = RecordingHost::default();
    let mut task = Task::new(&rig.context, &host, "start", rig.machine.clone());
    assert_eq!(restart_after_switch(&mut task), Ok(Gpu::Working));
    assert!(task.desktop_owed, "the desktop is set up once someone signs in");
    assert!(
        rig.asked().last().unwrap().contains("Win32_VideoController"),
        "the GPU is checked at the sign-in screen"
    );
}

#[test]
fn a_gpu_gone_after_the_restart_goes_back_to_microsofts_number() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, switching(), true, &["restarting\r\n", DESKTOP_UP]);
    let (result, _) = rig.start();
    assert!(result.unwrap_err().0.contains("without its GPU"));
    let state = rig.state();
    assert_eq!(state.shown, Shown::Microsoft);
    assert_eq!((state.failed_checks, state.failed_script.as_str()), (1, ""), "the next start tries again");
    let asked = rig.asked();
    assert!(asked.last().unwrap().contains("Win32_PnPEntity"), "the device's state is asked for the log");
    let log = crate::log::read_log(&rig.context);
    assert!(log.contains("gpu check: no working GPU. Windows did not answer"), "{log}");
}

#[test]
fn a_failed_check_logs_what_windows_says_about_the_device() {
    let scratch = Scratch::new();
    let mut answers = vec!["restarting\r\n", DESKTOP_UP];
    answers.extend(["gpu working 0\n"; 10]);
    answers.push("render device PCI\\VEN_10DE&DEV_2BFE\\3&1 status Error problem 31\r\nup 40 s\r\n");
    let rig = Rig::new(&scratch, switching(), true, &answers);
    assert!(rig.start().0.is_err());
    let log = crate::log::read_log(&rig.context);
    assert!(
        log.contains(
            "gpu check: no working GPU. render device PCI\\VEN_10DE&DEV_2BFE\\3&1 status Error problem 31"
        ),
        "{log}"
    );
}

#[test]
fn an_intel_gpu_gone_after_the_restart_keeps_intels_number_and_leaves_the_vm_running() {
    let scratch = Scratch::new();
    let mut rig = Rig::new(&scratch, switching(), true, &["restarting\r\n", DESKTOP_UP]);
    rig.machine.use_windows_card(crate::gpu_maker::GpuMaker::Intel, "0000:03:00.0");
    let package_vendor = crate::gpu_identity::INTEL_RENDER.vendor;
    write_state(&rig.context, &rig.machine, &IdentityState { package_vendor, ..switching() }).unwrap();
    let (result, error) = rig.start();
    assert_eq!(result, Ok(Gpu::Missing));
    assert!(error.contains("left running") && error.contains("keeps Intel's vendor ID"), "{error}");
    assert_eq!(rig.state().shown, Shown::Maker);
}
