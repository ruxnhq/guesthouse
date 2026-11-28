use super::*;
use crate::actions::tests::{machine, plain_task_context};
use crate::agent::stand_in::ScriptedAgent;
use crate::context::Context;
use crate::gpu_identity::{AMD_RENDER, MICROSOFT_RENDER, NVIDIA_RENDER, shown_identity};
use crate::machine::Machine;
use crate::testing::Scratch;
use crate::vm_host::stand_in::RecordingHost;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

const STAGED: &str = "signing: Valid\r\npackage in: oem7.inf\r\nidentity staged\r\n";
const REFUSED: &str = "stay: Windows' driver store has no copy of vrd.sys.\r\n";
const SCRIPT_TEXT: &str = "make the package";

struct Rig {
    context: Context,
    agent: Arc<ScriptedAgent>,
    machine: Machine,
}

impl Rig {
    /// A guest answering as given, on a QEMU that can set the identity unless told otherwise.
    fn new(scratch: &Scratch, qemu_can: bool, answers: &[&str]) -> Self {
        let mut context = plain_task_context(scratch);
        scratch.write("program/guest/gpu-identity.ps1", SCRIPT_TEXT);
        let listing =
            if qemu_can { "echo 'channel=<str>'; echo 'vendor-id=<uint16>'" } else { "echo 'channel=<str>'" };
        let qemu = scratch.write("qemu", format!("#!/bin/sh\n{listing}\n"));
        fs::set_permissions(&qemu, fs::Permissions::from_mode(0o700)).unwrap();
        context.paths.qemu = qemu;
        let agent = Arc::new(ScriptedAgent::answering(answers));
        context.agent = agent.clone();
        let machine = machine(&context);
        crate::agent::make_token(&context, &machine).unwrap();
        Self { context, agent, machine }
    }

    /// The same, for a machine on an AMD card.
    fn on_amd(scratch: &Scratch, answers: &[&str]) -> Self {
        let mut rig = Self::new(scratch, true, answers);
        rig.machine.amd_gpu = "0000:7c:00.0".into();
        rig
    }

    fn boot(&self) -> String {
        let host = RecordingHost::default();
        let mut task = Task::new(&self.context, &host, "start", self.machine.clone());
        set_up_identity(&mut task);
        task.error
    }

    fn asked(&self) -> Vec<String> {
        self.agent.requests.lock().unwrap().iter().map(|(_, _, script)| script.clone()).collect()
    }

    fn state(&self) -> IdentityState {
        read_state(&self.context, &self.machine)
    }
}

#[test]
fn a_guest_that_takes_the_package_is_shown_nvidias_number_from_the_next_start() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, true, &[STAGED]);
    assert_eq!(shown_identity(&rig.context, &rig.machine), MICROSOFT_RENDER);
    assert_eq!(rig.boot(), "");
    let asked = rig.asked();
    assert_eq!(asked.len(), 1);
    assert!(asked[0].starts_with("$vendor = '10DE'\n$device = '2BFE'\n"), "the script gets the numbers");
    assert!(asked[0].ends_with(SCRIPT_TEXT));
    let state = rig.state();
    assert_eq!(state.shown, Shown::Maker);
    assert_eq!(state.reason, "");
    assert_eq!(state.note, next_start("NVIDIA"));
    assert!(state.restarts_windows(), "the next start restarts Windows once");
    assert_eq!(shown_identity(&rig.context, &rig.machine), NVIDIA_RENDER);
}

#[test]
fn an_amd_machine_gets_the_package_for_amds_number() {
    let scratch = Scratch::new();
    let rig = Rig::on_amd(&scratch, &["identity staged\r\n"]);
    assert_eq!(rig.boot(), "");
    assert!(
        rig.asked()[0].starts_with("$vendor = '1002'\n$device = '7FFE'\n"),
        "the script gets AMD's numbers"
    );
    let state = rig.state();
    assert_eq!((state.shown, state.package_vendor), (Shown::Maker, 0x1002));
    assert_eq!(state.note, next_start("AMD"));
    assert_eq!(shown_identity(&rig.context, &rig.machine), AMD_RENDER);
}

#[test]
fn a_package_for_nvidias_number_on_an_amd_machine_is_made_again_for_amds() {
    let scratch = Scratch::new();
    let rig = Rig::on_amd(&scratch, &["identity staged\r\n"]);
    let old = IdentityState { shown: Shown::Maker, ..IdentityState::default() };
    write_state(&rig.context, &rig.machine, &old).unwrap();
    assert_eq!(
        shown_identity(&rig.context, &rig.machine),
        MICROSOFT_RENDER,
        "never a number with no package"
    );
    assert_eq!(rig.boot(), "");
    assert_eq!(rig.asked().len(), 1);
    assert_eq!(shown_identity(&rig.context, &rig.machine), AMD_RENDER);
}

#[test]
fn a_guest_that_keeps_microsofts_number_says_why_and_is_not_asked_again() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, true, &[REFUSED, STAGED]);
    assert_eq!(rig.boot(), "");
    let state = rig.state();
    assert_eq!(state.shown, Shown::Microsoft);
    assert_eq!(
        state.reason,
        "Windows keeps Microsoft's vendor ID. Windows' driver store has no copy of vrd.sys."
    );
    assert_eq!(state.note, "");
    assert!(!state.failed_script.is_empty());
    assert_eq!(rig.boot(), "");
    assert_eq!(rig.asked().len(), 1, "the same script isn't tried again");
    assert_eq!(rig.state(), state);
}

#[test]
fn a_guest_that_says_nothing_useful_keeps_microsofts_number_too() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, true, &["pnputil said something else"]);
    assert_eq!(rig.boot(), "");
    let state = rig.state();
    assert_eq!(state.shown, Shown::Microsoft);
    assert_eq!(state.reason, did_not_stage("NVIDIA"));
}

#[test]
fn a_qemu_without_the_properties_keeps_microsofts_number_without_asking() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, false, &[STAGED]);
    assert_eq!(rig.boot(), "");
    assert_eq!(rig.asked().len(), 0);
    let state = rig.state();
    assert_eq!(state.shown, Shown::Microsoft);
    assert_eq!(state.reason, qemu_without("NVIDIA"));
}

#[test]
fn a_machine_on_nvidias_number_is_left_alone_and_its_note_goes() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, true, &[STAGED]);
    let state = IdentityState { shown: Shown::Maker, note: next_start("NVIDIA"), ..IdentityState::default() };
    write_state(&rig.context, &rig.machine, &state).unwrap();
    assert_eq!(rig.boot(), "");
    assert_eq!(rig.asked().len(), 0);
    assert_eq!(rig.state(), IdentityState { shown: Shown::Maker, ..IdentityState::default() });
}

#[test]
fn a_failed_gpu_start_on_nvidias_number_goes_back_to_microsofts() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, true, &[]);
    let host = RecordingHost::default();
    let task = Task::new(&rig.context, &host, "start", rig.machine.clone());
    let same = after_failed_gpu(&task, Failure::new("no GPU"));
    assert_eq!(same.0, "no GPU", "a machine on Microsoft's number has nothing to go back to");
    let state = IdentityState { shown: Shown::Maker, restart_due: true, ..IdentityState::default() };
    write_state(&rig.context, &rig.machine, &state).unwrap();
    let said = after_failed_gpu(&task, Failure::new("no GPU"));
    assert_eq!(said.0, format!("no GPU {FELL_BACK} That start then tries NVIDIA's once more."));
    let state = rig.state();
    assert_eq!(state.shown, Shown::Microsoft);
    assert_eq!(state.reason, FELL_BACK);
    assert!(!state.restart_due);
    assert_eq!(
        (state.failed_checks, state.failed_script.as_str()),
        (1, ""),
        "one check is not the script's end"
    );
}

/// A start on the maker's number that ended without the GPU.
fn fail_a_check(rig: &Rig) -> String {
    let host = RecordingHost::default();
    let task = Task::new(&rig.context, &host, "start", rig.machine.clone());
    let mut state = rig.state();
    state.shown = Shown::Maker;
    state.restart_due = true;
    write_state(&rig.context, &rig.machine, &state).unwrap();
    after_failed_gpu(&task, Failure::new("no GPU")).0
}

#[test]
fn a_package_that_cost_one_start_is_staged_once_more() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, true, &[STAGED]);
    fail_a_check(&rig);
    let try_two = will_stage(&rig.context, &rig.machine).unwrap();
    assert!(try_two.ends_with(":try-2"), "a snapshot before it is its own: {try_two}");
    assert_eq!(rig.boot(), "");
    assert_eq!(rig.asked().len(), 1, "the next start has Windows take the package again");
    let state = rig.state();
    assert_eq!((state.shown, state.reason.as_str()), (Shown::Maker, ""));
    assert!(state.restarts_windows());
    assert_eq!(state.failed_checks, 1, "kept until a check on the maker's number has passed");
    assert_eq!(shown_identity(&rig.context, &rig.machine), NVIDIA_RENDER);
}

#[test]
fn a_check_that_passes_on_the_makers_number_clears_the_count() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, true, &[STAGED]);
    fail_a_check(&rig);
    assert_eq!(rig.boot(), "");
    // The next call is the one after the new VM's check and Windows' restart have passed.
    assert_eq!(rig.boot(), "");
    assert_eq!(
        rig.state(),
        IdentityState { shown: Shown::Maker, restart_due: true, ..IdentityState::default() }
    );
    assert_eq!(fail_a_check(&rig), format!("no GPU {FELL_BACK} That start then tries NVIDIA's once more."));
    assert_eq!(rig.state().failed_checks, 1, "a failure long after is the first again");
}

#[test]
fn a_package_that_cost_two_starts_in_a_row_is_not_staged_again() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, true, &[STAGED, STAGED, STAGED]);
    fail_a_check(&rig);
    assert_eq!(rig.boot(), "");
    let said = fail_a_check(&rig);
    assert_eq!(
        said,
        format!("no GPU {FELL_BACK} It stays there, as NVIDIA's has now failed 2 times in a row.")
    );
    let state = rig.state();
    assert!(state.out_of_tries() && !state.failed_script.is_empty());
    assert_eq!(will_stage(&rig.context, &rig.machine), None, "no snapshot for a start that changes nothing");
    for _ in 0..3 {
        assert_eq!(rig.boot(), "");
    }
    assert_eq!(rig.asked().len(), 1, "every later start stays on Microsoft's number");
    assert_eq!(rig.state(), state);
    assert_eq!(shown_identity(&rig.context, &rig.machine), MICROSOFT_RENDER);
}

#[test]
fn a_new_builds_script_gets_its_own_tries_after_the_old_one_ran_out() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, true, &[STAGED, STAGED]);
    fail_a_check(&rig);
    assert_eq!(rig.boot(), "");
    fail_a_check(&rig);
    scratch.write("program/guest/gpu-identity.ps1", "make the package another way");
    assert!(will_stage(&rig.context, &rig.machine).is_some());
    assert_eq!(rig.boot(), "");
    let state = rig.state();
    assert_eq!((state.shown, state.failed_checks, state.failed_script.as_str()), (Shown::Maker, 0, ""));
}

#[test]
fn the_users_word_gets_the_number_tried_again_after_the_tries_ran_out() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, true, &[STAGED, STAGED]);
    fail_a_check(&rig);
    assert_eq!(rig.boot(), "");
    fail_a_check(&rig);
    let mut state = rig.state();
    assert!(state.try_again());
    write_state(&rig.context, &rig.machine, &state).unwrap();
    assert_eq!(rig.boot(), "");
    assert_eq!(rig.asked().len(), 2);
    assert_eq!(rig.state().shown, Shown::Maker);
}

#[test]
fn a_machine_an_earlier_build_pinned_after_one_failed_check_is_staged_again() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, true, &[STAGED]);
    let pinned = IdentityState {
        failed_script: script_id(SCRIPT_TEXT),
        reason: FELL_BACK.into(),
        ..IdentityState::default()
    };
    write_state(&rig.context, &rig.machine, &pinned).unwrap();
    assert!(will_stage(&rig.context, &rig.machine).is_some());
    assert_eq!(rig.boot(), "");
    assert_eq!(rig.asked().len(), 1);
    assert_eq!(shown_identity(&rig.context, &rig.machine), NVIDIA_RENDER);
    assert_eq!(
        fail_a_check(&rig),
        format!("no GPU {FELL_BACK} It stays there, as NVIDIA's has now failed 2 times in a row.")
    );
}

#[test]
fn a_build_without_the_script_keeps_microsofts_number_and_says_so() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, true, &[STAGED]);
    fs::remove_file(rig.context.paths.guest_files().join("gpu-identity.ps1")).unwrap();
    assert_eq!(rig.boot(), "");
    assert_eq!(rig.asked().len(), 0);
    assert_eq!(rig.state().reason, no_script("NVIDIA"));
}

#[test]
fn a_failed_gpu_start_gives_the_display_back_the_name_microsofts_number_needs() {
    let scratch = Scratch::new();
    let rig = Rig::new(&scratch, true, &["named RDPUDD Chained DD from the next start\r\n"]);
    scratch.write("program/guest/display-name.ps1", "name it");
    let host = RecordingHost::default();
    let task = Task::new(&rig.context, &host, "start", rig.machine.clone());
    let state = IdentityState { shown: Shown::Maker, ..IdentityState::default() };
    write_state(&rig.context, &rig.machine, &state).unwrap();
    after_failed_gpu(&task, Failure::new("no GPU"));
    assert_eq!(rig.machine.windows_maker(), Some(GpuMaker::Nvidia));
    assert_eq!(rig.asked(), vec!["$name = 'RDPUDD Chained DD'\nname it".to_owned()]);

    let scratch = Scratch::new();
    let rig = Rig::on_amd(&scratch, &[]);
    scratch.write("program/guest/display-name.ps1", "name it");
    let task = Task::new(&rig.context, &host, "start", rig.machine.clone());
    write_state(&rig.context, &rig.machine, &state).unwrap();
    after_failed_gpu(&task, Failure::new("no GPU"));
    assert!(rig.asked().is_empty(), "only NVIDIA's driver goes by the name");
}
