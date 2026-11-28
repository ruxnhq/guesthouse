use super::super::LEAN_TASK;
use super::super::tests::{machine, plain_task_context, with_vm_running};
use super::*;
use crate::agent::stand_in::ScriptedAgent;
use crate::context::Context;
use crate::machine::{Kind, Machine};
use crate::testing::Scratch;
use crate::text::base64_decode;
use crate::vm_host::stand_in::RecordingHost;
use std::sync::Arc;

fn lean_context(scratch: &Scratch, answers: &[&str]) -> (Context, Arc<ScriptedAgent>) {
    let mut context = plain_task_context(scratch);
    scratch.write("program/guest/lean-start.ps1", "START\n");
    scratch.write("program/guest/lean.ps1", "RUNNER\n");
    with_vm_running(&context);
    let agent = Arc::new(ScriptedAgent::answering(answers));
    context.agent = agent.clone();
    (context, agent)
}

fn windows_machine(context: &Context, lean: bool) -> Machine {
    let vm = Machine { kind: Kind::Windows, ..machine(context) };
    crate::agent::make_token(context, &vm).unwrap();
    crate::lean::write_lean(context, &vm, lean).unwrap();
    vm
}

#[test]
fn the_status_file_says_how_it_ended() {
    assert_eq!(read_end("begin\ndeprovisioned Microsoft.BingNews\n"), None);
    assert_eq!(
        read_end("begin\nset Microsoft\\Dsh\\AllowNewsAndInterests\nend 0\n"),
        Some(Ended { not_done: 0, first: None })
    );
    let ended =
        read_end("begin\nkept Microsoft.YourPhone 0x80073CF1 in use\nkept Clipchamp.Clipchamp x\nend 2\n")
            .unwrap();
    assert_eq!(ended, Ended { not_done: 2, first: Some("Microsoft.YourPhone 0x80073CF1 in use".to_owned()) });
    assert_eq!(read_end("end x\n").unwrap().not_done, 1, "an end it can't read is not taken as all done");
}

#[test]
fn a_machine_made_without_it_is_not_asked_anything() {
    let scratch = Scratch::new();
    let (context, agent) = lean_context(&scratch, &[]);
    let vm = windows_machine(&context, false);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm);
    after_install(&mut task);
    assert!(agent.requests.lock().unwrap().is_empty());
    assert!(task.error.is_empty());
}

#[test]
fn the_list_goes_to_the_guest_as_data_and_the_script_runs_as_system() {
    let scratch = Scratch::new();
    let (context, agent) = lean_context(&scratch, &["started", "", "begin\n", "begin\nend 0\n"]);
    let vm = windows_machine(&context, true);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm);
    after_install(&mut task);
    assert!(task.error.is_empty(), "{}", task.error);
    let requests = agent.requests.lock().unwrap();
    let start = &requests[0].2;
    assert!(start.ends_with("START\n"), "{start}");
    let parts: Vec<&str> = start.split('\'').collect();
    let data: serde_json::Value = serde_json::from_slice(&base64_decode(parts[1]).unwrap()).unwrap();
    assert_eq!(data["apps"][0], "Microsoft.YourPhone");
    assert_eq!(base64_decode(parts[3]).unwrap(), b"RUNNER\n");
}

#[test]
fn what_did_not_happen_is_a_note_and_never_fails_the_install() {
    let scratch = Scratch::new();
    let (context, _) =
        lean_context(&scratch, &["started", "begin\nkept Microsoft.YourPhone in use\nend 1\n"]);
    let vm = windows_machine(&context, true);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm);
    after_install(&mut task);
    assert!(task.error.starts_with("Lean Windows did not all happen: 1 things were left"), "{}", task.error);
    assert!(task.error.contains("Microsoft.YourPhone in use") && task.error.ends_with("machine's settings."));
}

#[test]
fn a_quiet_agent_is_a_note_too() {
    let scratch = Scratch::new();
    let (context, _) = lean_context(&scratch, &[""]);
    let vm = windows_machine(&context, true);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm);
    after_install(&mut task);
    assert!(task.error.contains("the agent went quiet"), "{}", task.error);
}

#[test]
fn running_it_again_is_only_for_a_lean_machine_that_runs() {
    let scratch = Scratch::new();
    let (context, _) = lean_context(&scratch, &["started", "begin\nend 0\n"]);
    let plain = windows_machine(&context, false);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, LEAN_TASK, plain.clone());
    assert!(run_again(&mut task).unwrap_err().0.contains("not made lean"));
    let vm = windows_machine(&context, true);
    let mut task = Task::new(&context, &host, LEAN_TASK, vm);
    run_again(&mut task).unwrap();
}

#[test]
fn the_machine_keeps_which_apps_this_windows_had_and_lost() {
    let scratch = Scratch::new();
    let logs = "begin\ndeprovisioned Microsoft.BingNews\ndeprovisioned Microsoft.Windows.DevHome\nend 0\n\
                begin\nremoved MSTeams\nend 0\n";
    let (context, _) = lean_context(&scratch, &["started", "begin\nend 0\n", logs]);
    let vm = windows_machine(&context, true);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm.clone());
    after_install(&mut task);
    let outcome = crate::lean::read_outcome(&context, &vm);
    assert_eq!(outcome.taken_out, ["MSTeams", "Microsoft.BingNews", "Microsoft.Windows.DevHome"]);
    let text = crate::lean::state_text(&context, &vm).unwrap();
    assert_eq!(text, "Took out Teams (free), News, Dev Home, which this Windows had.");
}

fn started_lean(scratch: &Scratch, answers: &[&str], lean: bool) -> (Context, Arc<ScriptedAgent>, Machine) {
    let (context, agent) = lean_context(scratch, answers);
    scratch.write("program/guest/lean-check.ps1", "CHECK\n");
    let vm = windows_machine(&context, lean);
    (context, agent, vm)
}

#[test]
fn what_windows_put_back_is_taken_out_again_after_a_start_and_said() {
    let scratch = Scratch::new();
    let (context, agent, vm) = started_lean(
        &scratch,
        &[
            "checked\n",
            "back Microsoft.Windows.DevHome\nchecked\n",
            "started",
            "begin\nremoved Microsoft.Windows.DevHome\nend 0\n",
        ],
        true,
    );
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", vm.clone());
    let before = crate::tasks::read_task(&context, None);
    after_start(&mut task);
    let outcome = crate::lean::read_outcome(&context, &vm);
    assert_eq!(outcome.put_back, ["Microsoft.Windows.DevHome"]);
    assert_eq!(outcome.still_back, None);
    assert!(crate::lean::state_text(&context, &vm).unwrap().contains("Windows had put back Dev Home"));
    assert_eq!(crate::tasks::read_task(&context, None), before, "the start is over, so no step is written");
    let requests = agent.requests.lock().unwrap();
    assert!(requests[0].2.starts_with("$data = '") && requests[0].2.ends_with("CHECK\n"));
    assert_eq!(requests.len(), 4, "a look at once, a look later, and one run");
}

#[test]
fn a_guest_that_cannot_say_or_a_machine_not_made_lean_is_left_alone() {
    let scratch = Scratch::new();
    let (context, agent, vm) = started_lean(&scratch, &["not checked: busy", ""], true);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", vm.clone());
    after_start(&mut task);
    assert_eq!(agent.requests.lock().unwrap().len(), 2, "two looks and nothing run");
    assert_eq!(crate::lean::read_outcome(&context, &vm), crate::lean::Outcome::default());
    let scratch = Scratch::new();
    let (context, agent, vm) = started_lean(&scratch, &[], false);
    let mut task = Task::new(&context, &host, "start", vm);
    after_start(&mut task);
    assert!(agent.requests.lock().unwrap().is_empty());
}

#[test]
fn a_throwaway_is_not_looked_at_each_run() {
    let scratch = Scratch::new();
    let (context, agent, vm) = started_lean(&scratch, &["back Microsoft.Windows.DevHome\nchecked\n"], true);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", Machine { throwaway: true, ..vm });
    after_start(&mut task);
    assert!(agent.requests.lock().unwrap().is_empty());
}
