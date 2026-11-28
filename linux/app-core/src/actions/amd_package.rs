//! Tells a Windows guest on an AMD card about AMD's driver package, so the user-mode driver it loads
//! from the host driver store belongs to a package Windows knows. BattlEye blocks the driver until it
//! does, and games under it then never open a window. `setup::amd_package` has what the package is.
//!
//! A start asks the guest whether it has the package, and that is all once it does. A guest that
//! doesn't gets the INF and catalog, and its own script adds them to Windows' driver store.

use super::Task;
use super::guest_upload::{put_big_file, quoted};
use crate::agent::guest_script;
use crate::failure::Result;
use crate::log::write_log;
use crate::setup::amd_package::{AmdPackageFiles, file_name, package_files};
use crate::setup::guest_driver::guest_driver;

const SCRIPT: &str = "amd-package.ps1";
/// Where the INF and catalog go in the guest, which the script reads and then removes.
const STAGING: &str = "C:\\gpupv\\amd-package";
const HOST_DRIVER_STORE: &str = "C:\\Windows\\System32\\HostDriverStore\\FileRepository";
/// A package in Windows' driver store is a folder named for its INF.
const DRIVER_STORE: &str = "C:\\Windows\\System32\\DriverStore\\FileRepository";
const LOOK_SECONDS: u64 = 20;
/// Windows checks every file against the catalog and copies it, well over a gigabyte in all. This is
/// what the agent gets.
const REGISTER_SECONDS: u64 = 110;
const REGISTERED_MARK: &str = "package registered";
const ABSENT_MARK: &str = "package absent";
const NOT_REGISTERED_MARK: &str = "not registered: ";
const REGISTERING: &str = "Telling Windows about AMD's driver package";
const NOT_REGISTERED: &str = "Windows did not take AMD's driver package, so games with BattlEye may not start. The \
                              log has what Windows said, and the next start tries again.";

/// Asks whether Windows has the package, and makes an empty staging folder if it doesn't.
fn look_script(inf: &str) -> String {
    let package = quoted(&format!("{DRIVER_STORE}\\{inf}_amd64_*"));
    let staging = quoted(STAGING);
    format!(
        "if (Test-Path {package}) {{ '{REGISTERED_MARK}' }} else {{ \
         Remove-Item -LiteralPath {staging} -Recurse -Force -ea 0; \
         New-Item -ItemType Directory -Force {staging} | Out-Null; '{ABSENT_MARK}' }}"
    )
}

/// A line for the log about something that leaves the start as it is.
fn note(task: &Task, text: &str) -> Result<()> {
    Ok(write_log(task.context, &format!("AMD package: {text}"))?)
}

fn says(said: &str, mark: &str) -> bool {
    said.lines().any(|line| line.trim() == mark)
}

/// Sends the INF and catalog and has the guest add them, and says whether Windows took the package.
fn register(task: &mut Task, package: &AmdPackageFiles, store: &str, script: &str) -> Result<bool> {
    task.step(REGISTERING);
    for local in [&package.inf, &package.catalog] {
        let name = file_name(local);
        if !put_big_file(task, local, &format!("{STAGING}\\{name}"))? {
            note(task, &format!("{name} did not reach Windows"))?;
            return Ok(false);
        }
    }
    let sent = format!(
        "$inf = {}\n$filesFolder = {}\n$hostStore = {}\n{script}",
        quoted(&file_name(&package.inf)),
        quoted(&package.files_folder),
        quoted(store)
    );
    let said = task.ask_guest(&sent, REGISTER_SECONDS);
    let registered = says(&said, REGISTERED_MARK);
    let why = said.lines().find_map(|line| line.trim().strip_prefix(NOT_REGISTERED_MARK));
    let outcome = match (registered, why) {
        (true, _) => "Windows has AMD's driver package now".to_owned(),
        (false, Some(why)) => format!("Windows said: {why}"),
        (false, None) => "the guest didn't say what came of it".to_owned(),
    };
    note(task, &outcome)?;
    Ok(registered)
}

/// Makes sure Windows knows AMD's driver package. Windows is up by now and Direct3D works without
/// this, so what goes wrong is said and the machine stays up.
pub(super) fn register_amd_package(task: &mut Task) -> Result<()> {
    let Ok(driver) = guest_driver(task.context, &task.machine) else { return Ok(()) };
    let package = match package_files(&driver.folder) {
        Ok(package) => package,
        Err(why) => return note(task, &format!("not given to Windows. {why}")),
    };
    let said = task.ask_guest(&look_script(&file_name(&package.inf)), LOOK_SECONDS);
    if says(&said, REGISTERED_MARK) {
        return Ok(());
    }
    if !says(&said, ABSENT_MARK) {
        return note(task, "the guest didn't say whether it has it");
    }
    let Ok(script) = guest_script(task.context, SCRIPT) else {
        return note(task, &format!("this build has no {SCRIPT}"));
    };
    let store = format!("{HOST_DRIVER_STORE}\\{}", driver.store_folder);
    // A start that already has something to say keeps it: files that didn't reach Windows are why
    // the package wasn't taken, and the log has both.
    if !register(task, &package, &store, &script)? && task.error.is_empty() {
        task.error = NOT_REGISTERED.to_owned();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::guest_upload::stand_in::FilesGuest;
    use crate::actions::tests::{machine, plain_task_context};
    use crate::agent::GuestAgent;
    use crate::context::Context;
    use crate::machine::Machine;
    use crate::setup::amd::write_amd_setup;
    use crate::setup::amd_guest_folder::stand_in::lay_out_driver;
    use crate::setup::amd_package::stand_in::{CATALOG_NAME, INF_NAME, lay_out_package};
    use crate::testing::Scratch;
    use crate::vm_host::stand_in::RecordingHost;
    use std::fs;
    use std::sync::{Arc, Mutex};

    const DRIVER: &str = "amd/WT6A_INF";
    const FOLDER: &str = "B026716";
    const SCRIPT_TEXT: &str = "add the package";

    /// A guest that keeps the files it is sent, and answers the look and the script as told.
    #[derive(Default)]
    struct PackageGuest {
        files: FilesGuest,
        look: Mutex<String>,
        script: Mutex<String>,
        scripts: Mutex<Vec<String>>,
    }

    impl GuestAgent for PackageGuest {
        fn ask(&self, port: u16, token: &str, script: &str, seconds: u64) -> String {
            if script.ends_with(SCRIPT_TEXT) {
                self.scripts.lock().unwrap().push(script.to_owned());
                return self.script.lock().unwrap().clone();
            }
            if script.contains(ABSENT_MARK) {
                self.files.requests.lock().unwrap().push(script.to_owned());
                return self.look.lock().unwrap().clone();
            }
            self.files.ask(port, token, script, seconds)
        }
    }

    fn on_amd(scratch: &Scratch, look: &str, script: &str) -> (Context, Arc<PackageGuest>, Machine) {
        let mut context = plain_task_context(scratch);
        lay_out_driver(scratch, DRIVER, FOLDER, false);
        lay_out_package(scratch, DRIVER, FOLDER);
        scratch.write(&format!("program/guest/{SCRIPT}"), SCRIPT_TEXT);
        write_amd_setup(&context, &scratch.path(&format!("{DRIVER}/{FOLDER}")), "32.0.32015.12012").unwrap();
        let guest = Arc::new(PackageGuest::default());
        *guest.look.lock().unwrap() = look.to_owned();
        *guest.script.lock().unwrap() = script.to_owned();
        context.agent = guest.clone();
        let machine = Machine { amd_gpu: "0000:7c:00.0".into(), ..machine(&context) };
        crate::agent::make_token(&context, &machine).unwrap();
        (context, guest, machine)
    }

    fn run(context: &Context, machine: Machine) -> String {
        let host = RecordingHost::default();
        let mut task = Task::new(context, &host, "start", machine);
        register_amd_package(&mut task).unwrap();
        task.error
    }

    #[test]
    fn a_guest_that_has_the_package_is_only_asked() {
        let scratch = Scratch::new();
        let (context, guest, machine) = on_amd(&scratch, "package registered\r\n", "");
        assert_eq!(run(&context, machine), "");
        let requests = guest.files.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("DriverStore\\FileRepository\\u0205499.inf_amd64_*"), "{}", requests[0]);
        assert!(guest.scripts.lock().unwrap().is_empty());
    }

    #[test]
    fn a_guest_without_it_gets_the_inf_and_catalog_and_the_script_with_their_names() {
        let scratch = Scratch::new();
        let (context, guest, machine) =
            on_amd(&scratch, "package absent\r\n", "Driver added\r\npackage registered\r\n");
        assert_eq!(run(&context, machine), "");
        let files = guest.files.files.lock().unwrap();
        let sent: Vec<&String> = files.keys().collect();
        assert_eq!(sent, [&format!("{STAGING}\\{CATALOG_NAME}"), &format!("{STAGING}\\{INF_NAME}")]);
        let local = fs::read(scratch.path(&format!("{DRIVER}/{INF_NAME}"))).unwrap();
        assert!(files[&format!("{STAGING}\\{INF_NAME}")] == local);
        let scripts = guest.scripts.lock().unwrap();
        let wanted = format!(
            "$inf = '{INF_NAME}'\n$filesFolder = '{FOLDER}'\n$hostStore = '{HOST_DRIVER_STORE}\\amdxc.inf_amd64_gpupv'\n\
             {SCRIPT_TEXT}"
        );
        assert_eq!(*scripts, [wanted]);
    }

    #[test]
    fn a_package_windows_refuses_is_said_and_the_start_goes_on() {
        let scratch = Scratch::new();
        let refusal =
            "not registered: The third-party INF does not contain digital signature information.\r\n";
        let (context, _, machine) = on_amd(&scratch, "package absent\r\n", refusal);
        assert_eq!(run(&context, machine.clone()), NOT_REGISTERED);
        let log = fs::read_to_string(context.paths.log_file()).unwrap();
        assert!(log.contains("Windows said: The third-party INF"), "{log}");
    }

    #[test]
    fn a_start_that_already_has_a_problem_to_say_keeps_it() {
        let scratch = Scratch::new();
        let (context, _, machine) = on_amd(&scratch, "package absent\r\n", "not registered: no\r\n");
        let host = RecordingHost::default();
        let mut task = Task::new(&context, &host, "start", machine);
        task.error = "Some of AMD's driver files did not reach Windows.".to_owned();
        register_amd_package(&mut task).unwrap();
        assert!(task.error.starts_with("Some of AMD's driver files"), "{}", task.error);
    }

    #[test]
    fn a_guest_that_does_not_answer_is_left_alone() {
        let scratch = Scratch::new();
        let (context, guest, machine) = on_amd(&scratch, "", "");
        assert_eq!(run(&context, machine), "");
        assert!(guest.files.files.lock().unwrap().is_empty() && guest.scripts.lock().unwrap().is_empty());
    }

    #[test]
    fn a_file_that_does_not_arrive_means_no_script_is_run() {
        let scratch = Scratch::new();
        let (context, guest, machine) = on_amd(&scratch, "package absent\r\n", "package registered\r\n");
        *guest.files.pieces_allowed.lock().unwrap() = Some(0);
        assert_eq!(run(&context, machine), NOT_REGISTERED);
        assert!(guest.scripts.lock().unwrap().is_empty());
    }

    #[test]
    fn a_driver_unpacked_without_its_inf_is_logged_and_the_guest_is_not_asked() {
        let scratch = Scratch::new();
        let (context, guest, machine) = on_amd(&scratch, "package absent\r\n", "");
        fs::remove_file(scratch.path(&format!("{DRIVER}/{INF_NAME}"))).unwrap();
        assert_eq!(run(&context, machine), "");
        assert!(guest.files.requests.lock().unwrap().is_empty());
    }
}
