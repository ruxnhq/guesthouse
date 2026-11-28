//! A start gives the guest this app's agent.

use super::tests::{machine, plain_task_context, with_vm_running};
use super::{Task, boot};
use crate::agent::stand_in::ScriptedAgent;
use crate::testing::Scratch;
use crate::vm_host::stand_in::RecordingHost;
use std::fs;
use std::sync::Arc;

/// A guest set up by an earlier version gets this app's agent at every start, as soon as its agent has
/// answered, for its next boot.
#[test]
fn a_start_gives_the_guest_this_apps_agent_first() {
    let scratch = Scratch::new();
    let mut context = plain_task_context(&scratch);
    scratch.write("program/guest/display.ps1", "what the session sees");
    scratch.write("program/guest/agent.ps1", "the agent");
    scratch.write("program/guest/agent-update.ps1", "put it in place");
    let agent = Arc::new(ScriptedAgent::answering(&[
        "agent updated\r\n",
        "3\n",
        "gpu working 1\n",
        "device missing",
        "",
        "screen basic 1920x1080 64 hdr 0",
        "",
        "screen 1920x1080\n",
        "",
        "screen basic 1920x1080 64 hdr 0",
    ]));
    context.agent = agent.clone();
    let vm = machine(&context);
    crate::agent::make_token(&context, &vm).unwrap();
    fs::create_dir_all(context.run_folder(None)).unwrap();
    with_vm_running(&context);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", vm.clone());
    boot(&mut task).unwrap();
    assert_eq!(task.error, "");
    let asked = agent.requests.lock().unwrap();
    let sent = crate::text::base64_encode(b"the agent");
    assert_eq!(asked[0].2, format!("$agent = '{sent}'\n$path = 'C:\\gpupv\\agent.ps1'\nput it in place"));
    assert!(asked[2].2.contains("Win32_VideoController"), "{}", asked[2].2);
    let log = fs::read_to_string(context.paths.log_file()).unwrap_or_default();
    assert!(log.contains(&format!("{}: agent updated", vm.id)), "{log}");
}
