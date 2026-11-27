//! A start that brings the guest's copy of NVIDIA's driver to this PC's version: see
//! [`crate::guest_driver_store`]. The disc is in the VM from its first boot. Once the agent answers,
//! and before anything here uses the GPU, `nv-store.ps1` copies it beside the guest's copy, checks
//! it, and has the two change places. When something in Windows has the driver loaded, which by the
//! time the agent answers is usual, the change is queued for the next boot and Windows restarts once.
//! The same script runs at each boot from a task the first such start leaves in the guest, so a
//! boot that gets there before Windows loads the driver needs no restart.
//!
//! Nothing here stops the start: Windows keeps the driver it had, the start says why, and the next
//! start tries again, as the note of the guest's version is only written once the guest has it.

use super::Task;
use super::desktop::{GUEST_FOLDER, put_file};
use super::identity_restart::restart_windows;
use crate::agent::QUICK_SECONDS;
use crate::failure::Result;
use crate::guest_driver_store::{StoreAnswer, disc_path, due, make_disc, read_answer, record_version};
use crate::log::write_log;
use crate::settings::driver_folder;
use crate::setup::inf::retail_version;
use std::path::PathBuf;
use std::time::Duration;

pub(super) const SCRIPT: &str = "nv-store.ps1";
const STATUS: &str = "nv-store.status";
const BOOT_STATUS: &str = "nv-store-boot.status";
const BOOT_TASK: &str = "gpupv-nv-store";
const LAUNCHED: &str = "store launched";
const LAUNCH_SECONDS: u64 = 30;
/// The copy is about 1.5 GB from a virtual disc, so well under this.
const COPY_WAIT: Duration = Duration::from_secs(30 * 60);
const COPY_POLL: Duration = Duration::from_secs(3);
/// From the restart to the agent's first answer. Windows took about 20 seconds to the firmware's next
/// boot line, and the agent starts with Windows.
const AGENT_WAIT: Duration = Duration::from_secs(10 * 60);
const AGENT_POLL: Duration = Duration::from_secs(3);
const AGENT_MARK: &str = "agent up";
const KEPT_OLD: &str = "NVIDIA's driver in Windows stays at the version it had";

fn guest_path(name: &str) -> String {
    format!("{GUEST_FOLDER}\\{name}")
}

/// The disc for this start, made the first time a version is due. `None` when nothing is due, or
/// the disc could not be made, which is logged: the guest then keeps its copy, as before.
pub(super) fn driver_disc_for_start(task: &Task) -> Option<PathBuf> {
    let version = due(task.context, &task.machine)?;
    if !disc_path(task.context, &version).is_file() {
        task.step("Putting NVIDIA's driver on a disc for Windows");
    }
    match make_disc(task.context, &driver_folder(task.context), &version) {
        Ok(disc) => Some(disc),
        Err(failure) => {
            let _ = write_log(task.context, &format!("driver store: {}", failure.0));
            None
        }
    }
}

/// Starts the script in the guest on its own, so the agent answers while it copies, and the status
/// file it writes has only this run's lines. The boot task goes in beside it, if it isn't there.
fn launch_script(version: &str) -> String {
    let (script, status) = (guest_path(SCRIPT), guest_path(STATUS));
    let arguments = |status: &str, version: &str| {
        let version = if version.is_empty() { String::new() } else { format!(" -Version {version}") };
        format!("-NoProfile -ExecutionPolicy Bypass -File {script}{version} -Status {status}")
    };
    let boot = arguments(&guest_path(BOOT_STATUS), "");
    format!(
        "Remove-Item -LiteralPath '{status}' -Force -ea 0\n\
         if (-not (Get-ScheduledTask -TaskName '{BOOT_TASK}' -ea 0)) {{\n\
         $action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument '{boot}'\n\
         $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit 0\n\
         Register-ScheduledTask -TaskName '{BOOT_TASK}' -Action $action -Trigger (New-ScheduledTaskTrigger -AtStartup) -User 'SYSTEM' -RunLevel Highest -Settings $settings -Force | Out-Null\n\
         }}\n\
         Start-Process -FilePath 'powershell.exe' -ArgumentList '{}' -WindowStyle Hidden\n\
         '{LAUNCHED}'",
        arguments(&status, version)
    )
}

/// One run of the script, to its answer. `None` if it never gave one in time.
fn run_script(task: &Task, version: &str) -> Option<StoreAnswer> {
    if !task.ask_guest(&launch_script(version), LAUNCH_SECONDS).contains(LAUNCHED) {
        return None;
    }
    let read = format!("Get-Content -LiteralPath '{}' -ea 0", guest_path(STATUS));
    let end = task.now() + COPY_WAIT;
    let mut copying = false;
    while task.now() < end {
        task.sleep(COPY_POLL);
        match read_answer(&task.ask_guest(&read, QUICK_SECONDS)) {
            None => {}
            Some(StoreAnswer::Copying) => {
                if !copying {
                    task.step(&format!("Copying NVIDIA's driver {} into Windows", retail_version(version)));
                    copying = true;
                }
            }
            Some(answer) => return Some(answer),
        }
    }
    None
}

/// Whether the agent answered again, after a restart, in time.
fn wait_for_agent(task: &Task) -> bool {
    let end = task.now() + AGENT_WAIT;
    while task.now() < end {
        if task.ask_guest(&format!("'{AGENT_MARK}'"), QUICK_SECONDS).contains(AGENT_MARK) {
            return true;
        }
        task.sleep(AGENT_POLL);
    }
    false
}

/// What the start says when the guest kept its copy, which the log has more of.
fn kept(task: &mut Task, why: &str) -> Result<()> {
    write_log(task.context, &format!("driver store: {why}"))?;
    let note = format!("{KEPT_OLD}: {why}. The next start tries again.");
    task.error = if task.error.is_empty() { note } else { format!("{} {note}", task.error) };
    Ok(())
}

/// Brings the guest's copy to this PC's version, if it is due and the disc is in the VM. Only the
/// agent is needed, so a Windows at its sign-in screen gets it as well.
pub(super) fn refresh_driver_store(task: &mut Task) -> Result<()> {
    let Some(version) = due(task.context, &task.machine) else { return Ok(()) };
    if !disc_path(task.context, &version).is_file() {
        return Ok(());
    }
    task.step(&format!("Bringing NVIDIA's driver in Windows to {}", retail_version(&version)));
    let local = task.context.paths.guest_files().join(SCRIPT);
    if !local.is_file() {
        return kept(task, "this program folder has no script to copy it");
    }
    if !put_file(task, &local, &guest_path(SCRIPT))? {
        return kept(task, "the script that copies it did not reach Windows");
    }
    let mut answer = run_script(task, &version);
    if answer.as_ref().is_some_and(StoreAnswer::restart) {
        task.step(&format!("Restarting Windows once for NVIDIA's driver {}", retail_version(&version)));
        if !restart_windows(task) {
            return kept(task, "Windows did not restart when asked");
        }
        if !wait_for_agent(task) {
            return kept(task, "Windows did not come back after its restart");
        }
        answer = run_script(task, &version);
    }
    match answer {
        Some(StoreAnswer::Current { version: had, .. } | StoreAnswer::Replaced { version: had, .. })
            if had == version =>
        {
            write_log(task.context, &format!("driver store: Windows has NVIDIA's driver {version}"))?;
            record_version(task.context, &task.machine, &version)
        }
        Some(StoreAnswer::Queued) => kept(task, "the change is still waiting for Windows to restart"),
        Some(StoreAnswer::NoDisc) => kept(task, "Windows did not find the driver's disc"),
        Some(StoreAnswer::NoRoom(why)) => kept(task, &format!("there is not room for it: {why}")),
        Some(StoreAnswer::Failed(why)) => kept(task, &why),
        Some(other) => kept(task, &format!("the guest said {other:?}")),
        None => kept(task, "the guest did not say how the copy went"),
    }
}

#[cfg(test)]
#[path = "nv_store_tests.rs"]
mod tests;
