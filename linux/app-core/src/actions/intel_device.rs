//! The Intel render device in the guest, switched off on the software desktop and on again for the
//! hardware one: see [`crate::intel_desktop::switches_device_off`]. It is done through the agent,
//! which needs nothing from the desktop. The device stays in the VM, as without it Windows puts the
//! session on a display QEMU never shows.

use super::Task;
use super::identity_restart::restart_windows;
use crate::failure::Result;
use crate::gpu_identity::PciIdentity;
use crate::gpu_maker::GpuMaker;
use crate::intel_desktop::{device_off_file, device_off_for_start, for_start, lab_device_on_recorded_card};
use crate::log::write_log;
use guesthouse_backend_intel::stage::LAB_DEVICE_ON_RECORDED_CARD_VARIABLE;
use std::fs;

const OFF: &str = "Intel device off in the guest: desktop drawn by the basic display, no GPU for programs";
const LEFT_ON: &str = "Intel device left on with the software desktop, for work on the card: the screen \
     stays black, so tasks go through the guest agent";
const BACK_ON: &str = "Intel device on again in the guest";
const RESTARTING: &str = "Restarting Windows once, so it starts with the Intel device on";
const RESTARTED: &str = "Windows restarted, so it starts with the Intel device on";
const DID_NOT_RESTART: &str = "Windows did not restart after the Intel device was switched back on, so Direct3D 12, \
     OpenCL and Level Zero may not find the card until the machine is started again.";
/// What the guest says of a device Windows has stopped, as a switched-off one is.
const STOPPED: &str = "Error";
const PATIENCE_SECONDS: u64 = 60;

/// Switches the devices the guest shows with this identity on or off, and says each one's status.
fn switch_script(identity: PciIdentity, on: bool) -> String {
    let verb = if on { "Enable" } else { "Disable" };
    format!(
        "$d=@(Get-PnpDevice -PresentOnly -ea 0 | Where-Object {{ $_.InstanceId -like '{}' }}); \
         foreach($x in $d){{ {verb}-PnpDevice -InstanceId $x.InstanceId -Confirm:$false -ea 0 }}; \
         foreach($x in $d){{ (Get-PnpDevice -InstanceId $x.InstanceId).Status }}",
        identity.windows_pattern()
    )
}

/// Whether the guest's answer shows every device stopped, or every one running.
fn all_stopped(said: &str, stopped: bool) -> bool {
    let statuses: Vec<&str> = said.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    !statuses.is_empty() && statuses.iter().all(|status| (*status == STOPPED) == stopped)
}

fn is_intel(task: &Task) -> bool {
    task.machine.windows_maker() == Some(GpuMaker::Intel)
}

/// Before the GPU is checked: a device the app switched off on an earlier start is on again, so the
/// check sees the GPU, and a hardware desktop gets its card.
pub(super) fn switch_back_on(task: &mut Task, shown: PciIdentity) -> Result<()> {
    let file = device_off_file(task.context, &task.machine);
    if !is_intel(task) || !file.exists() {
        return Ok(());
    }
    let said = task.ask_guest(&switch_script(shown, true), PATIENCE_SECONDS);
    let how =
        if all_stopped(&said, false) { String::new() } else { format!(" (the guest said: {})", said.trim()) };
    write_log(task.context, &format!("{BACK_ON}{how}"))?;
    let _ = fs::remove_file(file);
    if stays_on_for_start(task) {
        restart_with_device_on(task)?;
    }
    Ok(())
}

/// Whether the device stays on for the rest of this start: on the hardware desktop, or for work
/// asked on the software one.
fn stays_on_for_start(task: &Task) -> bool {
    for_start(task.context, &task.machine).has_display_driver()
        || !device_off_for_start(task.context, &task.machine)
}

/// A device switched on after Windows booted gives Vulkan and OpenGL the card, but Direct3D 12,
/// OpenCL and Level Zero find no adapter on it until Windows starts again with it on. So Windows is
/// restarted once, and the start goes on to wait for its desktop. The device-off file is gone by
/// then, so the next boot doesn't come back here.
fn restart_with_device_on(task: &mut Task) -> Result<()> {
    task.step(RESTARTING);
    if restart_windows(task) {
        write_log(task.context, RESTARTED)?;
        return Ok(());
    }
    write_log(task.context, &format!("{RESTARTED}: Windows did not restart when asked"))?;
    task.error = DID_NOT_RESTART.to_owned();
    Ok(())
}

/// Once the GPU is up: on the software desktop the device goes off, so the basic display draws.
pub(super) fn switch_off_for_software_desktop(task: &Task, shown: PciIdentity) -> Result<()> {
    if !is_intel(task) || for_start(task.context, &task.machine).has_display_driver() {
        return Ok(());
    }
    if !device_off_for_start(task.context, &task.machine) {
        let lab = if lab_device_on_recorded_card(task.context) {
            format!(
                " (lab: {LAB_DEVICE_ON_RECORDED_CARD_VARIABLE}=1, the recorded card counted as a real one)"
            )
        } else {
            String::new()
        };
        write_log(task.context, &format!("{LEFT_ON}{lab}"))?;
        return Ok(());
    }
    let said = task.ask_guest(&switch_script(shown, false), PATIENCE_SECONDS);
    let how =
        if all_stopped(&said, true) { String::new() } else { format!(" (the guest said: {})", said.trim()) };
    fs::write(device_off_file(task.context, &task.machine), format!("{OFF}\n"))?;
    write_log(task.context, &format!("{OFF}{how}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::desktop::{BOOT_LINE, SERIAL_FILE};
    use crate::actions::identity_restart::RESTART_SCRIPT;
    use crate::actions::tests::{machine, plain_task_context};
    use crate::agent::GuestAgent;
    use crate::agent::stand_in::ScriptedAgent;
    use crate::gpu_identity::INTEL_RENDER;
    use crate::testing::Scratch;
    use crate::vm_host::stand_in::RecordingHost;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::Arc;

    #[test]
    fn the_script_finds_the_device_by_the_identity_it_shows() {
        let off = switch_script(INTEL_RENDER, false);
        assert!(off.contains(r"-like 'PCI\VEN_8086&DEV_CFFE*'"));
        assert!(off.contains("Disable-PnpDevice") && !off.contains("Enable-PnpDevice"));
        assert!(switch_script(INTEL_RENDER, true).contains("Enable-PnpDevice"));
    }

    /// A guest that answers as scripted, and whose firmware boots again when Windows is asked to
    /// restart.
    struct Guest {
        scripted: ScriptedAgent,
        serial: PathBuf,
    }

    impl GuestAgent for Guest {
        fn ask(&self, port: u16, token: &str, script: &str, seconds: u64) -> String {
            if script == RESTART_SCRIPT {
                let mut serial = OpenOptions::new().append(true).open(&self.serial).unwrap();
                serial.write_all(BOOT_LINE).unwrap();
            }
            self.scripted.ask(port, token, script, seconds)
        }
    }

    /// Switches a device the app had switched off back on, on an Intel machine, and says what the
    /// guest was asked and what the start's error became.
    fn switch_on(scratch: &Scratch, fell_back: bool) -> (Vec<String>, String) {
        let mut context = plain_task_context(scratch);
        let folder = context.run_folder(None);
        fs::create_dir_all(&folder).unwrap();
        let serial = folder.join(SERIAL_FILE);
        fs::write(&serial, BOOT_LINE).unwrap();
        let guest =
            Arc::new(Guest { scripted: ScriptedAgent::answering(&["OK\r\n", "restarting\r\n"]), serial });
        context.agent = guest.clone();
        let mut machine = machine(&context);
        machine.gpu_maker = Some(GpuMaker::Intel);
        crate::agent::make_token(&context, &machine).unwrap();
        fs::write(device_off_file(&context, &machine), format!("{OFF}\n")).unwrap();
        if fell_back {
            crate::intel_desktop::fall_back(&context, &machine, "no ring for the display driver").unwrap();
        }
        let host = RecordingHost::default();
        let mut task = Task::new(&context, &host, "start", machine.clone());
        switch_back_on(&mut task, INTEL_RENDER).unwrap();
        assert!(!device_off_file(&context, &machine).exists());
        let asked =
            guest.scripted.requests.lock().unwrap().iter().map(|(_, _, script)| script.clone()).collect();
        (asked, std::mem::take(&mut task.error))
    }

    #[test]
    fn a_device_switched_back_on_that_stays_on_restarts_windows_once() {
        let scratch = Scratch::new();
        let (asked, error) = switch_on(&scratch, false);
        assert_eq!(asked.len(), 2, "{asked:?}");
        assert!(asked[0].contains("Enable-PnpDevice"));
        assert_eq!(asked[1], RESTART_SCRIPT, "the hardware desktop keeps the device, so Windows restarts");
        assert_eq!(error, "");
    }

    #[test]
    fn a_device_that_goes_off_again_on_the_software_desktop_restarts_nothing() {
        let scratch = Scratch::new();
        let (asked, error) = switch_on(&scratch, true);
        assert_eq!(asked.len(), 1, "{asked:?}");
        assert!(asked[0].contains("Enable-PnpDevice"));
        assert_eq!(error, "");
    }

    #[test]
    fn the_answer_is_read_device_by_device() {
        assert!(all_stopped("Error\r\n", true));
        assert!(all_stopped("OK\n", false));
        assert!(!all_stopped("OK\nError\n", true));
        assert!(!all_stopped("", true), "no device found is not a device off");
    }
}
