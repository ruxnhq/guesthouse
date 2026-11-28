//! Intel's driver files for a Windows guest on an Intel card. Setup unpacks the folder the guest
//! gets, and the install disc takes it to the guest. A guest installed by a test build before the
//! kernel driver went in lacks that file, and Windows then won't start the adapter, so the GPU is
//! missing from the first boot on. Each start gives the guest's copy what it lacks of the folder on
//! this PC before the GPU is checked, and restarts Windows if it gave anything, so the adapter starts
//! with the file there.

use super::Task;
use super::desktop::wait_for_windows;
use super::driver_store::{HOST_DRIVER_STORE, guest_names, missing_in_guest};
use super::guest_upload::put_big_file;
use super::identity_restart::restart_windows;
use crate::failure::Result;
use crate::gpu_maker::GpuMaker;
use crate::log::write_log;
use crate::setup::guest_driver::guest_driver;

/// The Direct3D 12 driver, which every guest set up for Intel got at install.
const USER_DRIVER: &str = "igd12umd64.dll";
const PLACING: &str = "Giving Windows Intel's driver files it lacks";
const RESTARTING: &str = "Restarting Windows for the driver's new files";
const NOT_PLACED: &str = "Some of Intel's driver files did not reach Windows. The log says which, and the next \
                          start tries again.";
const NOT_RESTARTED: &str = "Windows was given Intel's driver files it lacked, but did not restart, so the GPU comes at the next start.";
const DESKTOP_GONE: &str = "Windows restarted for Intel's driver files, but its desktop never came back.";

/// Brings the guest's copy of Intel's driver up to date, and restarts Windows if that changed it.
/// What goes wrong in the copy is said and the start goes on, as the GPU check says what it costs.
pub(super) fn complete_intel_driver(task: &mut Task) -> Result<()> {
    if task.machine.windows_maker() != Some(GpuMaker::Intel) || !give_missing_files(task)? {
        return Ok(());
    }
    task.step(RESTARTING);
    if !restart_windows(task) {
        write_log(task.context, "Intel driver: Windows did not restart when asked")?;
        task.error = NOT_RESTARTED.to_owned();
        return Ok(());
    }
    task.step("Waiting for the desktop");
    wait_for_windows(task, DESKTOP_GONE)
}

/// Copies into the guest what its copy of the driver lacks, and says whether any of it got there.
fn give_missing_files(task: &mut Task) -> Result<bool> {
    // A folder that lacks files counts as no driver, and the start was refused before this.
    let Ok(driver) = guest_driver(task.context, &task.machine) else { return Ok(false) };
    let store = format!("{HOST_DRIVER_STORE}\\{}", driver.store_folder);
    let Some(in_guest) = guest_names(task, &store) else {
        write_log(task.context, "Intel driver: the guest didn't list its driver files")?;
        return Ok(false);
    };
    // A guest without the driver itself didn't get it at install, and copying all of it this way
    // would take long. Installing again is the way there.
    if !in_guest.contains(USER_DRIVER) {
        write_log(task.context, "Intel driver: the guest has no copy of the driver to complete")?;
        return Ok(false);
    }
    let missing = missing_in_guest(&driver.folder, &in_guest)?;
    let mut failed = Vec::new();
    for (done, local) in missing.iter().enumerate() {
        task.step(&format!("{PLACING}, {} of {}", done + 1, missing.len()));
        let name = local.file_name().unwrap_or_default().to_string_lossy().into_owned();
        if !put_big_file(task, local, &format!("{store}\\{name}"))? {
            failed.push(name);
        }
    }
    let placed = missing.len() - failed.len();
    if placed > 0 {
        write_log(task.context, &format!("Intel driver: gave Windows {placed} files it lacked"))?;
    }
    if !failed.is_empty() {
        write_log(
            task.context,
            &format!("Intel driver: these did not reach Windows: {}", failed.join(", ")),
        )?;
        task.error = NOT_PLACED.to_owned();
    }
    Ok(placed > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::desktop::{BOOT_LINE, SERIAL_FILE};
    use crate::actions::driver_store::LISTED_MARK;
    use crate::actions::guest_upload::stand_in::FilesGuest;
    use crate::actions::identity_restart::RESTART_SCRIPT;
    use crate::actions::tests::plain_task_context;
    use crate::agent::GuestAgent;
    use crate::context::Context;
    use crate::machine::Machine;
    use crate::setup::intel::stand_in::lay_out_driver;
    use crate::setup::intel::write_intel_setup;
    use crate::testing::Scratch;
    use crate::vm_host::stand_in::RecordingHost;
    use guesthouse_backend_intel::protocol::guest_driver::XE2_LPG_DRIVERS;
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::Arc;

    const STORE: &str = "C:\\Windows\\System32\\HostDriverStore\\FileRepository\\iigd_dch.inf_amd64_gpupv";

    /// A guest that keeps files, restarts when asked if `restarts`, and has its desktop up.
    struct Guest {
        files: FilesGuest,
        serial: PathBuf,
        restarts: bool,
    }

    impl GuestAgent for Guest {
        fn ask(&self, port: u16, token: &str, script: &str, seconds: u64) -> String {
            if script == RESTART_SCRIPT {
                self.files.requests.lock().unwrap().push(script.to_owned());
                if self.restarts {
                    OpenOptions::new().append(true).open(&self.serial).unwrap().write_all(BOOT_LINE).unwrap();
                }
                return "restarting\r\n".to_owned();
            }
            if script.contains("explorer") {
                return "3\n".to_owned();
            }
            // Windows starts the adapter only on a boot with the kernel driver in its copy.
            if script.contains("Win32_VideoController") {
                let has_kernel = self.files.files.lock().unwrap().keys().any(|path| path.ends_with(".sys"));
                let booted_since = fs::read(&self.serial)
                    .unwrap()
                    .windows(BOOT_LINE.len())
                    .filter(|at| *at == BOOT_LINE)
                    .count()
                    > 1;
                return format!("gpu working {}\n", u8::from(has_kernel && booted_since));
            }
            self.files.ask(port, token, script, seconds)
        }
    }

    /// A machine on an Intel card whose guest was installed by a test build: it has every file of
    /// the driver's folder on this PC but `lacking`.
    fn installed_without(
        scratch: &Scratch,
        lacking: &[&str],
        restarts: bool,
    ) -> (Context, Arc<Guest>, Machine) {
        let mut context = plain_task_context(scratch);
        lay_out_driver(scratch, "intel-driver");
        let folder = scratch.path("intel-driver");
        write_intel_setup(&context, &folder, "32.0.101.9175").unwrap();
        let run = context.run_folder(None);
        fs::create_dir_all(&run).unwrap();
        let serial = run.join(SERIAL_FILE);
        fs::write(&serial, BOOT_LINE).unwrap();
        let mut listing = String::new();
        for entry in fs::read_dir(&folder).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !lacking.contains(&name.as_str()) {
                listing += &format!("{}\r\n", name.to_uppercase());
            }
        }
        let files = FilesGuest::default();
        *files.other_answer.lock().unwrap() = format!("{listing}{LISTED_MARK}\r\n");
        let guest = Arc::new(Guest { files, serial, restarts });
        context.agent = guest.clone();
        let mut machine = crate::actions::tests::machine(&context);
        machine.use_windows_card(GpuMaker::Intel, "0000:03:00.0");
        crate::agent::make_token(&context, &machine).unwrap();
        (context, guest, machine)
    }

    #[test]
    fn a_guest_from_a_test_build_gets_the_kernel_driver_and_a_restart() {
        let scratch = Scratch::new();
        let (context, guest, machine) = installed_without(&scratch, &[XE2_LPG_DRIVERS.kernel], true);
        let host = RecordingHost::default();
        let mut task = Task::new(&context, &host, "start", machine);
        assert_eq!(complete_intel_driver(&mut task), Ok(()));
        assert_eq!(task.error, "");
        let files = guest.files.files.lock().unwrap();
        let placed: Vec<&String> = files.keys().collect();
        assert_eq!(placed, [&format!("{STORE}\\{}", XE2_LPG_DRIVERS.kernel)]);
        let local = fs::read(scratch.path("intel-driver").join(XE2_LPG_DRIVERS.kernel)).unwrap();
        assert!(files[placed[0]] == local);
        assert!(guest.files.requests.lock().unwrap().iter().any(|asked| asked == RESTART_SCRIPT));
    }

    /// The whole start: the file goes in and Windows restarts before the GPU is checked, so the
    /// check finds it and the start goes on as any other.
    #[test]
    fn a_start_of_a_guest_from_a_test_build_has_its_gpu() {
        let scratch = Scratch::new();
        let (context, _guest, machine) = installed_without(&scratch, &[XE2_LPG_DRIVERS.kernel], true);
        let host = RecordingHost::default();
        let mut task = Task::new(&context, &host, "start", machine);
        assert_eq!(crate::actions::get_gpu_ready(&mut task), Ok(crate::actions::gpu_check::Gpu::Working));
        assert!(!task.error.contains("without its GPU"), "{}", task.error);
    }

    #[test]
    fn a_guest_that_does_not_restart_is_told_the_gpu_comes_next_time() {
        let scratch = Scratch::new();
        let (context, _guest, machine) = installed_without(&scratch, &[XE2_LPG_DRIVERS.kernel], false);
        let host = RecordingHost::default();
        let mut task = Task::new(&context, &host, "start", machine);
        assert_eq!(complete_intel_driver(&mut task), Ok(()));
        assert_eq!(task.error, NOT_RESTARTED);
    }

    #[test]
    fn a_guest_with_everything_is_only_asked_for_its_list() {
        let scratch = Scratch::new();
        let (context, guest, machine) = installed_without(&scratch, &[], true);
        let host = RecordingHost::default();
        let mut task = Task::new(&context, &host, "start", machine);
        assert_eq!(complete_intel_driver(&mut task), Ok(()));
        assert_eq!(guest.files.requests.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_guest_without_the_driver_itself_or_on_another_card_is_sent_nothing() {
        let scratch = Scratch::new();
        let (context, guest, machine) =
            installed_without(&scratch, &[USER_DRIVER, XE2_LPG_DRIVERS.kernel], true);
        let host = RecordingHost::default();
        let mut task = Task::new(&context, &host, "start", machine.clone());
        assert_eq!(complete_intel_driver(&mut task), Ok(()));
        assert!(guest.files.files.lock().unwrap().is_empty());
        let on_amd = Machine { gpu_maker: None, ..machine };
        guest.files.requests.lock().unwrap().clear();
        complete_intel_driver(&mut Task::new(&context, &host, "start", on_amd)).unwrap();
        assert!(guest.files.requests.lock().unwrap().is_empty(), "an AMD machine isn't asked");
    }
}
