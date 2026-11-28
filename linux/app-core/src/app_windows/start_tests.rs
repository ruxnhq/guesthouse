use super::stand_in::FakeHelper;
use super::*;
use crate::agent::make_token;
use crate::agent::stand_in::ScriptedAgent;
use crate::context::Context;
use crate::machine::{Kind, Machine};
use crate::processes::stand_in::fake_process;
use crate::testing::{Scratch, scratch_context};
use crate::text::base64_encode;
use std::fs;
use std::sync::Arc;
use std::time::Duration;

fn quick() -> LinkSettings {
    LinkSettings {
        timing: Timing {
            connect: Duration::from_millis(500),
            hello: Duration::from_millis(300),
            write: Duration::from_millis(500),
            reconnect_first: Duration::from_millis(20),
            reconnect_most: Duration::from_millis(100),
            start_cooldown: Duration::from_millis(300),
            request: Duration::from_millis(400),
            apps_request: Duration::from_millis(800),
        },
        patience: Duration::from_secs(3),
    }
}

/// This test process as the machine's QEMU, which forwards the helper's port from `port`.
fn acting(context: &Context, machine: &Machine, port: u16) -> crate::testing::ActingQemu {
    crate::testing::act_as_qemu(context, machine, None, &[(crate::network::GUEST_APP_WINDOWS_PORT, port)])
}

fn token_of(context: &Context, machine: &Machine) -> String {
    fs::read_to_string(machine.token_file(context)).unwrap().trim().to_owned()
}

/// A Windows machine with a token, whose agent answers from a list.
fn rig(scratch: &Scratch, answers: &[&str], on: bool) -> (Arc<Context>, Machine, Arc<ScriptedAgent>) {
    let mut context = scratch_context(scratch);
    context.app_windows = quick();
    for (name, text) in [
        ("app-windows.ps1", "the helper"),
        ("app-windows.cs", "the source"),
        ("app-windows-start.ps1", "start it"),
        ("helper-stop.ps1", "stop it"),
    ] {
        scratch.write(&format!("program/guest/{name}"), text);
    }
    let agent = Arc::new(ScriptedAgent::answering(answers));
    context.agent = agent.clone();
    let machine = Machine::new("vm", "vm", "/d", "/v");
    machine.save(&context).unwrap();
    make_token(&context, &machine).unwrap();
    if on {
        set_on(&context, &machine, true).unwrap();
    }
    (Arc::new(context), machine, agent)
}

/// QEMU as it runs a machine in slot 0, with or without the forward for the helper.
fn running(context: &Context, machine: &Machine, with_forward: bool) {
    let qemu = context.paths.qemu.display().to_string();
    let monitor = format!("unix:{}/qmp.sock,server=on", context.run_folder(None).display());
    let disk = format!("file={},format=qcow2", machine.disk);
    let nic = if with_forward {
        format!("user,hostfwd=tcp:127.0.0.1:0-:8765,{}", forward_word())
    } else {
        "user,hostfwd=tcp:127.0.0.1:0-:8765".to_owned()
    };
    fake_process(context, 40, &[&qemu, "-qmp", &monitor, &disk, "-nic", &nic], &[], "");
}

#[test]
fn a_link_is_refused_with_the_switch_off_and_nothing_is_asked_of_the_guest() {
    let scratch = Scratch::new();
    let (context, machine, agent) = rig(&scratch, &[], false);
    let refused = connect(&context, &machine, None).err().unwrap();
    assert!(
        refused.0.starts_with("App windows is off for vm. guesthouse-vm set vm app-windows on"),
        "{refused}"
    );
    assert!(agent.requests.lock().unwrap().is_empty());
}

#[test]
fn only_a_windows_machine_gets_a_link() {
    let scratch = Scratch::new();
    let (context, machine, _) = rig(&scratch, &[], true);
    let plain = Machine { kind: Kind::Plain, ..machine };
    let refused = connect(&context, &plain, None).err().unwrap();
    assert!(refused.0.contains("not a Windows machine"), "{refused}");
}

#[test]
fn a_helper_that_is_running_is_never_started() {
    let scratch = Scratch::new();
    let (context, machine, agent) = rig(&scratch, &[], true);
    let helper = FakeHelper::start(&token_of(&context, &machine));
    let _qemu = acting(&context, &machine, helper.port);
    let mut link = connect(&context, &machine, None).unwrap();
    link.wait_ready().unwrap();
    assert!(agent.requests.lock().unwrap().is_empty(), "no start, and no stop");
    link.end_if_started();
    assert!(agent.requests.lock().unwrap().is_empty());
}

#[test]
fn a_helper_that_is_not_running_is_started_with_its_script_its_source_and_its_start_script() {
    let scratch = Scratch::new();
    let (context, machine, agent) = rig(&scratch, &["started\r\n", ""], true);
    // Nothing listens on the port, so the link asks the agent, which says it started and ends it.
    let mut link = connect(&context, &machine, None).unwrap();
    assert!(link.wait_ready().is_err(), "the pretend start left nothing listening");
    let requests = agent.requests.lock().unwrap().clone();
    let start = &requests[0].2;
    let expected = format!(
        "$helperName = 'app-windows'\nstop it\n$helper = '{}'\n$source = '{}'\nstart it",
        base64_encode(b"the helper"),
        base64_encode(b"the source")
    );
    assert_eq!(start, &expected);
    let monitor = context.run_folder(None).join(crate::monitor::MAIN_SOCKET);
    assert_eq!(requests[0].0, monitor, "through the agent of the machine's slot");
    link.end_if_started();
    let requests = agent.requests.lock().unwrap().clone();
    assert_eq!(
        requests.last().unwrap().2,
        "$helperName = 'app-windows'\nstop it",
        "a helper this link started is ended"
    );
}

#[test]
fn a_start_that_cannot_be_done_says_why_at_once() {
    let scratch = Scratch::new();
    for (said, words) in [
        ("nobody is logged in\r\n", "Nobody is logged in to vm"),
        ("", "agent did not answer"),
        ("not listening\r\n", "did not start in vm"),
    ] {
        let (context, machine, _) = rig(&scratch, &[said], true);
        let mut link = connect(&context, &machine, None).unwrap();
        let failure = link.wait_ready().unwrap_err();
        assert!(failure.0.contains(words), "{said:?}: {failure}");
    }
}

#[test]
fn a_machine_started_without_the_forward_is_told_to_restart_and_the_helper_is_not_started() {
    let scratch = Scratch::new();
    let (context, machine, agent) = rig(&scratch, &["started\r\n"], true);
    running(&context, &machine, false);
    let mut link = connect(&context, &machine, None).unwrap();
    let failure = link.wait_ready().unwrap_err();
    assert!(
        failure.0.contains("started without the way in") && failure.0.contains("Restart it."),
        "{failure}"
    );
    assert!(agent.requests.lock().unwrap().is_empty());
}

#[test]
fn a_machine_that_has_the_forward_is_started_for_as_usual() {
    let scratch = Scratch::new();
    let (context, machine, agent) = rig(&scratch, &["started\r\n", ""], true);
    running(&context, &machine, true);
    let mut link = connect(&context, &machine, None).unwrap();
    let _ = link.wait_ready();
    assert!(!agent.requests.lock().unwrap().is_empty());
}

#[test]
fn an_older_program_folder_without_the_helper_files_is_told_in_words() {
    let scratch = Scratch::new();
    let (context, machine, agent) = rig(&scratch, &["started\r\n"], true);
    fs::remove_file(context.paths.guest_files().join("app-windows.ps1")).unwrap();
    let mut link = connect(&context, &machine, None).unwrap();
    let failure = link.wait_ready().unwrap_err();
    assert_eq!(failure.0, "This program folder has no app windows helper for the guest. Update Guesthouse.");
    assert!(agent.requests.lock().unwrap().is_empty());
}

#[test]
fn a_machine_with_no_token_is_refused_in_words() {
    let scratch = Scratch::new();
    let (context, machine, _) = rig(&scratch, &[], true);
    fs::remove_file(machine.token_file(&context)).unwrap();
    let refused = connect(&context, &machine, None).err().unwrap();
    assert!(refused.0.contains("has no token"), "{refused}");
}

#[test]
fn a_layout_that_cannot_be_read_is_said_and_not_waited_out() {
    let scratch = Scratch::new();
    let (context, machine, _) = rig(&scratch, &[], true);
    let helper = FakeHelper::start(&token_of(&context, &machine));
    let _qemu = acting(&context, &machine, helper.port);
    // The helper's cells lack the fields the contract names.
    helper.with(|inner| {
        inner.layout_line =
            Some(r#"{"t":"layout","seq":1,"screen":{"w":1,"h":1},"cells":[{"id":"1"}]}"#.into());
    });
    let started = std::time::Instant::now();
    let mut link = connect(&context, &machine, None).unwrap();
    let failure = link.wait_ready().unwrap_err();
    assert!(failure.0.contains("sent no layout this program can read"), "{failure}");
    assert!(failure.0.contains("a layout message that cannot be read"), "the reason is in it: {failure}");
    assert!(started.elapsed() < Duration::from_secs(2), "the patience was 3 s");
}
