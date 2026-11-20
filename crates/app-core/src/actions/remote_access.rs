//! Puts the machine's choices of Remote Desktop and SSH right in Windows: at every start once the
//! desktop is up, as anything in Windows can change them, and in a task of its own when one changes
//! while Windows runs. A choice nobody made is left alone. Windows is up by now, so what goes wrong is
//! said and doesn't stop the machine.

use super::Task;
use super::guest_upload::{put_big_file, quoted};
use crate::agent::QUICK_SECONDS;
use crate::failure::{Failure, Result};
use crate::log::write_log;
use crate::remote_access::{GUEST_INSTALLER, Installer, NO_OPENSSH, Service, read_access, script_for};

const SCRIPT_SECONDS: u64 = 60;
/// OpenSSH's installer, and sshd's first start, which makes its host keys.
const INSTALL_SECONDS: u64 = 300;
const NEEDS_INSTALLER: &str = "needs the installer";
/// What the scripts say when they did their job, after "kept " if nothing needed changing.
const DONE: [&str; 2] = ["on", "off"];
const KEPT: &str = "kept ";

fn add_note(task: &mut Task, note: &str) {
    task.error = if task.error.is_empty() { note.to_owned() } else { format!("{} {note}", task.error) };
}

fn switch(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

/// Each choice the machine has, put right in Windows.
pub(super) fn apply_remote_access(task: &mut Task) -> Result<()> {
    let access = read_access(task.context, &task.machine);
    for (service, on) in access.chosen() {
        apply(task, service, on)?;
    }
    Ok(())
}

/// The task for a change made while the machine runs.
pub(super) fn apply_change(task: &mut Task) -> Result<()> {
    if task.pid().is_none() {
        return Err(Failure::new("The machine is off, so Windows gets the change at its next start."));
    }
    task.step("Taking the change to Windows");
    apply_remote_access(task)
}

fn apply(task: &mut Task, service: Service, on: bool) -> Result<()> {
    let script = match script_for(task.context, service, on, Installer::NotGiven) {
        Ok(script) => script,
        Err(failure) => {
            write_log(task.context, &format!("{}: {}", service.key(), failure.0))?;
            return Ok(());
        }
    };
    let mut said = task.ask_guest(&script, SCRIPT_SECONDS);
    if service == Service::Ssh && said.trim() == NEEDS_INSTALLER {
        let Some(installed) = send_installer_and_ask(task, on)? else { return Ok(()) };
        said = installed;
    }
    let said = said.trim();
    let name = service.name();
    if said.is_empty() {
        add_note(
            task,
            &format!("Windows did not answer about {name}, so it gets the change at the next start."),
        );
        return Ok(());
    }
    if DONE.contains(&said.strip_prefix(KEPT).unwrap_or(said)) {
        if !said.starts_with(KEPT) {
            write_log(task.context, &format!("{}: {said}", service.key()))?;
        }
        return Ok(());
    }
    write_log(task.context, &format!("{}: {said}", service.key()))?;
    add_note(task, &format!("{name} could not be turned {} in Windows. The log says why.", switch(on)));
    Ok(())
}

/// Sends OpenSSH's installer to a guest without OpenSSH, and runs the script again with it. What the
/// guest said then, or `None` if the installer never got there, which has been said.
fn send_installer_and_ask(task: &mut Task, on: bool) -> Result<Option<String>> {
    let installer = task.context.paths.openssh_installer();
    if !installer.is_file() {
        add_note(task, &format!("SSH is not on in Windows. {NO_OPENSSH}"));
        return Ok(None);
    }
    task.step("Putting OpenSSH in Windows");
    if !put_big_file(task, &installer, GUEST_INSTALLER)? {
        add_note(
            task,
            "OpenSSH's installer did not reach Windows, so SSH is not on yet. The next start tries again.",
        );
        return Ok(None);
    }
    let script = script_for(task.context, Service::Ssh, on, Installer::Sent)?;
    let said = task.ask_guest(&script, INSTALL_SECONDS);
    let tidy = format!("Remove-Item -LiteralPath {} -ea 0", quoted(GUEST_INSTALLER));
    task.ask_guest(&tidy, QUICK_SECONDS);
    Ok(Some(said))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::tests::{machine, plain_task_context};
    use crate::agent::stand_in::ScriptedAgent;
    use crate::remote_access::{Access, write_access};
    use crate::testing::Scratch;
    use crate::vm_host::stand_in::RecordingHost;
    use std::sync::Arc;

    fn with_scripts(scratch: &Scratch) {
        scratch.write("program/guest/remote-desktop.ps1", "turn it");
        scratch.write("program/guest/ssh.ps1", "set it up");
    }

    /// Applies these choices with the guest answering so, and says what the task said and what the
    /// guest was asked.
    fn applied(scratch: &Scratch, access: Option<Access>, answers: &[&str]) -> (String, Vec<String>) {
        let mut context = plain_task_context(scratch);
        with_scripts(scratch);
        let agent = Arc::new(ScriptedAgent::answering(answers));
        context.agent = agent.clone();
        let vm = machine(&context);
        crate::agent::make_token(&context, &vm).unwrap();
        if let Some(access) = access {
            write_access(&context, &vm, &access).unwrap();
        }
        let host = RecordingHost::default();
        let mut task = Task::new(&context, &host, "start", vm);
        apply_remote_access(&mut task).unwrap();
        let asked = agent.requests.lock().unwrap().iter().map(|request| request.2.clone()).collect();
        (task.error, asked)
    }

    #[test]
    fn a_machine_nobody_chose_for_is_left_alone() {
        let scratch = Scratch::new();
        assert_eq!(applied(&scratch, None, &[]), (String::new(), Vec::new()));
    }

    #[test]
    fn each_choice_goes_to_windows_with_its_setting() {
        let scratch = Scratch::new();
        let (said, asked) = applied(&scratch, Some(Access::NEW), &["kept on\r\n", "off\r\n"]);
        assert_eq!(said, "");
        assert_eq!(asked.len(), 2);
        assert!(asked[0].starts_with("$on = $true\n") && asked[0].ends_with("turn it"), "{}", asked[0]);
        assert!(asked[1].starts_with("$on = $false\n$package = ''\n") && asked[1].ends_with("set it up"));
    }

    #[test]
    fn a_failure_or_silence_is_said_and_the_start_goes_on() {
        let scratch = Scratch::new();
        let access = Access { remote_desktop: Some(true), ssh: None };
        let (said, _) = applied(&scratch, Some(access), &["FAILED remote desktop: access denied\r\n"]);
        assert_eq!(said, "Remote Desktop could not be turned on in Windows. The log says why.");
        let scratch = Scratch::new();
        let (said, _) = applied(&scratch, Some(Access { ssh: Some(false), ..access }), &["", "ERROR: x"]);
        assert!(said.starts_with("Windows did not answer about Remote Desktop"), "{said}");
        assert!(said.ends_with("SSH could not be turned off in Windows. The log says why."), "{said}");
    }

    #[test]
    fn a_guest_without_openssh_gets_the_installer_through_the_agent() {
        let scratch = Scratch::new();
        scratch.write("program/openssh/OpenSSH-Win64.msi", "installer");
        let access = Access { remote_desktop: None, ssh: Some(true) };
        let answers = ["needs the installer\r\n", "piece written\r\n", "size 9\r\n", "on\r\n", ""];
        let (said, asked) = applied(&scratch, Some(access), &answers);
        assert_eq!(said, "");
        assert!(asked[1].contains("OpenSSH-Win64.msi.part"), "the installer went in pieces: {}", asked[1]);
        assert!(asked[3].contains("$package = 'C:\\gpupv\\OpenSSH-Win64.msi'\n"), "{}", asked[3]);
        assert!(asked[4].starts_with("Remove-Item"), "the installer doesn't stay: {}", asked[4]);
    }

    #[test]
    fn with_no_installer_here_the_guest_is_told_nothing_more_and_the_user_is_told_why() {
        let scratch = Scratch::new();
        let access = Access { remote_desktop: None, ssh: Some(true) };
        let (said, asked) = applied(&scratch, Some(access), &["needs the installer\r\n"]);
        assert_eq!(said, format!("SSH is not on in Windows. {NO_OPENSSH}"));
        assert_eq!(asked.len(), 1);
    }
}
