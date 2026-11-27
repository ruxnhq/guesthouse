//! A Windows that asks for a password or PIN has no desktop until someone signs in. A start or a
//! restart that finds it up with nobody signed in leaves the VM on and is reported done, with a note
//! saying so. Its display and shared folder are set in the user's session, so this waits for someone
//! to sign in, in the same process but outside the task, as [`super::late_watch`] does: it holds no
//! lock and writes no step while it waits. Once the desktop is up it takes the slot and sets the
//! desktop up as a start would have.
//!
//! The wait ends with the VM, or with a restart of Windows that the app has not set up yet, as the
//! app's own look at restarts sets that one up.

use super::Task;
use super::desktop::{desktop_up, rebooted, set_up_desktop};
use super::late_watch::take_slot;
use crate::log::write_log;

/// Waits for someone to sign in, then sets up the desktop. What goes wrong there is said, and the VM
/// runs on: Windows is up and someone is using it.
pub(super) fn set_up_desktop_after_sign_in(task: &mut Task) {
    let Some(qemu) = task.pid() else { return };
    note(task, "nobody has signed in yet, so the desktop is set up once someone does");
    if let Some(end) = wait_for_sign_in(task, qemu) {
        note(task, end);
        return;
    }
    let Some(_lock) = take_slot(task, qemu) else {
        note(
            task,
            "someone signed in, but the slot was not free, or the VM went, so the desktop was left as it is",
        );
        return;
    };
    note(task, "someone signed in, so the desktop is set up");
    task.error.clear();
    task.step("Setting up the desktop");
    task.desktop_owed = false;
    if let Err(failure) = set_up_desktop(task) {
        task.error = failure.0;
    }
    task.finish();
}

/// Until the desktop is up in `qemu`. What ended the wait otherwise, if something did.
fn wait_for_sign_in(task: &Task, qemu: u32) -> Option<&'static str> {
    loop {
        if task.pid() != Some(qemu) {
            return Some("the VM went before anyone signed in");
        }
        if rebooted(task.context, None) {
            return Some("Windows restarted before anyone signed in, and is set up after that restart");
        }
        if desktop_up(task) {
            return None;
        }
    }
}

fn note(task: &Task, text: &str) {
    // The wait goes on without its line in the log.
    let _ = write_log(task.context, &format!("sign-in: {text}"));
}

#[cfg(test)]
mod tests {
    use super::super::desktop::{
        BOOT_LINE, BOOTS_FILE, RESTART_SIGN_IN_NOTE, SERIAL_FILE, START_SIGN_IN_NOTE,
    };
    use super::super::tests::{machine, plain_task_context, with_vm_running};
    use super::super::{after_reboot, boot};
    use super::*;
    use crate::agent::stand_in::ScriptedAgent;
    use crate::context::Context;
    use crate::tasks::read_task;
    use crate::testing::Scratch;
    use crate::vm_host::stand_in::RecordingHost;
    use std::fs;
    use std::sync::Arc;

    const NO_DESKTOP: &str = "1\n";
    const NOBODY: &str = "nobody signed in\r\n";

    /// A VM whose guest answers as scripted: a whole wait for the desktop with no desktop, then `then`.
    fn context_after_a_wait(scratch: &Scratch, then: &[&str]) -> (Context, Arc<ScriptedAgent>) {
        let mut context = plain_task_context(scratch);
        let mut answers = vec![NO_DESKTOP; 60];
        answers.extend(then);
        let agent = Arc::new(ScriptedAgent::answering(&answers));
        context.agent = agent.clone();
        fs::create_dir_all(context.run_folder(None)).unwrap();
        with_vm_running(&context);
        (context, agent)
    }

    fn asked(agent: &ScriptedAgent) -> Vec<String> {
        agent.requests.lock().unwrap().iter().map(|(_, _, script)| script.clone()).collect()
    }

    #[test]
    fn a_windows_waiting_for_sign_in_is_left_on_and_its_desktop_waits() {
        let scratch = Scratch::new();
        let (context, agent) = context_after_a_wait(&scratch, &[NOBODY, "gpu working 1\n"]);
        let vm = machine(&context);
        crate::agent::make_token(&context, &vm).unwrap();
        let host = RecordingHost::default();
        let mut task = Task::new(&context, &host, "start", vm);
        assert_eq!(boot(&mut task), Ok(()));
        assert_eq!(host.said(), ["start gpu"], "nothing turned it off");
        assert!(task.error.starts_with(START_SIGN_IN_NOTE), "{}", task.error);
        assert!(task.desktop_owed);
        let asked = asked(&agent);
        assert!(asked[61].contains("Win32_VideoController"), "the GPU is checked at the sign-in screen");
        assert!(
            !asked.iter().any(|script| script.contains("New-ScheduledTaskPrincipal")),
            "no session is asked"
        );
        assert!(!context.run_folder(None).join(BOOTS_FILE).exists(), "the boot is noted once it's set up");
    }

    #[test]
    fn someone_signed_in_with_no_desktop_still_fails_the_start_and_turns_off() {
        let scratch = Scratch::new();
        let (context, _) = context_after_a_wait(&scratch, &["\r\n"]);
        let vm = machine(&context);
        crate::agent::make_token(&context, &vm).unwrap();
        let host = RecordingHost::default();
        let mut task = Task::new(&context, &host, "start", vm);
        assert!(boot(&mut task).unwrap_err().0.contains("desktop never came up"));
        assert_eq!(host.said(), ["start gpu", "stop 30"]);
        assert!(!task.desktop_owed);
    }

    #[test]
    fn a_restart_at_the_sign_in_screen_says_so_and_waits() {
        let scratch = Scratch::new();
        let (context, _) = context_after_a_wait(&scratch, &[NOBODY]);
        let vm = machine(&context);
        crate::agent::make_token(&context, &vm).unwrap();
        let host = RecordingHost::default();
        let mut task = Task::new(&context, &host, "after_reboot", vm);
        assert_eq!(after_reboot(&mut task), Ok(()));
        assert_eq!(task.error, RESTART_SIGN_IN_NOTE);
        assert!(task.desktop_owed);
        assert!(host.said().is_empty());
    }

    #[test]
    fn once_someone_signs_in_the_desktop_is_set_up() {
        let scratch = Scratch::new();
        let mut context = plain_task_context(&scratch);
        let agent = Arc::new(ScriptedAgent::answering(&[NO_DESKTOP, "", "3\n"]));
        context.agent = agent.clone();
        fs::create_dir_all(context.run_folder(None)).unwrap();
        with_vm_running(&context);
        let vm = machine(&context);
        crate::agent::make_token(&context, &vm).unwrap();
        let host = RecordingHost::default();
        let mut task = Task::new(&context, &host, "start", vm);
        task.desktop_owed = true;
        task.error = START_SIGN_IN_NOTE.to_owned();
        set_up_desktop_after_sign_in(&mut task);
        assert!(!task.desktop_owed);
        assert_eq!(task.said.lock().unwrap().first().map(String::as_str), Some("Setting up the desktop"));
        assert!(context.run_folder(None).join(BOOTS_FILE).exists(), "the boot is noted");
        let record = read_task(&context, None);
        assert_eq!((record.pid, record.error.as_str()), (0, ""), "the note is gone once it's done");
        assert!(asked(&agent)[..3].iter().all(|script| script.contains("explorer")), "three looks first");
    }

    #[test]
    fn a_vm_that_has_gone_ends_the_wait() {
        let scratch = Scratch::new();
        let context = plain_task_context(&scratch);
        let vm = machine(&context);
        let host = RecordingHost::default();
        let mut task = Task::new(&context, &host, "start", vm);
        task.desktop_owed = true;
        set_up_desktop_after_sign_in(&mut task);
        assert!(task.said.lock().unwrap().is_empty());
        assert!(!context.run_folder(None).join(BOOTS_FILE).exists());
    }

    #[test]
    fn a_restart_the_app_has_not_set_up_ends_the_wait() {
        let scratch = Scratch::new();
        let mut context = plain_task_context(&scratch);
        let agent = Arc::new(ScriptedAgent::answering(&[]));
        context.agent = agent.clone();
        let folder = context.run_folder(None);
        fs::create_dir_all(&folder).unwrap();
        with_vm_running(&context);
        fs::write(folder.join(SERIAL_FILE), [BOOT_LINE, b"\n", BOOT_LINE].concat()).unwrap();
        fs::write(folder.join(BOOTS_FILE), "1").unwrap();
        let vm = machine(&context);
        let host = RecordingHost::default();
        let mut task = Task::new(&context, &host, "after_reboot", vm);
        task.desktop_owed = true;
        set_up_desktop_after_sign_in(&mut task);
        assert!(task.said.lock().unwrap().is_empty());
        assert!(asked(&agent).is_empty(), "the app's own look sets that restart up");
    }
}
