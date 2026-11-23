use super::super::SCAN_ALL_TASK;
use super::super::tests::{machine, plain_task_context, with_vm_running};
use super::*;
use crate::agent::stand_in::ScriptedAgent;
use crate::context::Context;
use crate::machine::{Kind, Machine};
use crate::testing::Scratch;
use crate::text::base64_decode;
use crate::vm_host::stand_in::RecordingHost;
use std::sync::Arc;

fn setting_up(
    scratch: &Scratch,
    answers: &[&str],
    folders: &[&str],
) -> (Context, Arc<ScriptedAgent>, Machine) {
    let mut context = plain_task_context(scratch);
    scratch.write("program/guest/scan-exclusions.ps1", "SCRIPT\n");
    with_vm_running(&context);
    let agent = Arc::new(ScriptedAgent::answering(answers));
    context.agent = agent.clone();
    let vm = Machine { kind: Kind::Windows, ..machine(&context) };
    crate::agent::make_token(&context, &vm).unwrap();
    let folders: Vec<String> = folders.iter().map(|folder| (*folder).to_owned()).collect();
    crate::scan_exclusions::write_exclusions(&context, &vm, &folders).unwrap();
    (context, agent, vm)
}

#[test]
fn a_machine_made_without_them_is_not_asked_anything() {
    let scratch = Scratch::new();
    let (context, agent, vm) = setting_up(&scratch, &[], &[]);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm);
    after_install(&mut task);
    assert!(agent.requests.lock().unwrap().is_empty());
}

#[test]
fn the_folders_go_to_the_guest_as_data() {
    let scratch = Scratch::new();
    let (context, agent, vm) = setting_up(&scratch, &["done 1"], &[r"D:\Games"]);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm);
    after_install(&mut task);
    assert!(task.error.is_empty(), "{}", task.error);
    let state = crate::scan_exclusions::state_text(&context, &task.machine).unwrap();
    assert!(state.starts_with("Defender doesn't scan D:\\Games."), "{state}");
    let requests = agent.requests.lock().unwrap();
    let sent = &requests[0].2;
    assert!(sent.contains("\n$change = 'add'\nSCRIPT\n"), "{sent}");
    let data = sent.split('\'').nth(1).unwrap();
    assert_eq!(base64_decode(data).unwrap(), br#"{"folders":["D:\\Games"]}"#);
}

#[test]
fn when_defender_does_not_take_them_the_install_goes_on_with_a_note() {
    let scratch = Scratch::new();
    let (context, _, vm) = setting_up(&scratch, &["not done: no"], &[r"D:\Games"]);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm);
    after_install(&mut task);
    assert_eq!(task.error, "Defender still scans every folder: not done: no.");
    let state = crate::scan_exclusions::state_text(&context, &task.machine).unwrap();
    assert!(state.ends_with("but didn't take them, so every folder is scanned."), "{state}");
}

#[test]
fn scanning_them_again_forgets_them_only_once_defender_has() {
    let scratch = Scratch::new();
    let (context, agent, vm) = setting_up(&scratch, &["not done: busy", "done 1"], &[r"D:\Games"]);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, SCAN_ALL_TASK, vm.clone());
    assert!(scan_all(&mut task).unwrap_err().0.contains("as it was"));
    assert_eq!(crate::scan_exclusions::read_exclusions(&context, &vm).len(), 1);
    scan_all(&mut task).unwrap();
    assert!(crate::scan_exclusions::read_exclusions(&context, &vm).is_empty());
    assert!(agent.requests.lock().unwrap()[1].2.contains("$change = 'remove'"));
    assert!(scan_all(&mut task).unwrap_err().0.contains("already scans"));
}

#[test]
fn a_start_after_an_import_asks_the_guest_and_keeps_what_it_says() {
    let scratch = Scratch::new();
    let (context, agent, vm) = setting_up(&scratch, &["", "folder D:\\Games\nlisted\n"], &[]);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", vm.clone());
    after_start(&mut task);
    assert!(agent.requests.lock().unwrap().is_empty(), "no mark, no question");
    crate::scan_exclusions::mark_for_look(&vm.folder(&context)).unwrap();
    after_start(&mut task);
    assert!(crate::scan_exclusions::needs_look(&context, &vm), "a guest that said nothing is asked again");
    after_start(&mut task);
    assert!(!crate::scan_exclusions::needs_look(&context, &vm));
    assert_eq!(crate::scan_exclusions::read_exclusions(&context, &vm), [r"D:\Games"]);
    assert!(agent.requests.lock().unwrap()[0].2.contains("Get-MpPreference"));
}

#[test]
fn a_throwaway_does_not_take_its_folders_back_for_one_run() {
    let scratch = Scratch::new();
    let (context, agent, vm) = setting_up(&scratch, &["done 1"], &[r"D:\Games"]);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, SCAN_ALL_TASK, Machine { throwaway: true, ..vm.clone() });
    assert!(scan_all(&mut task).unwrap_err().0.contains("Turn Throwaway off"));
    assert!(agent.requests.lock().unwrap().is_empty());
    assert_eq!(crate::scan_exclusions::read_exclusions(&context, &vm).len(), 1);
}
