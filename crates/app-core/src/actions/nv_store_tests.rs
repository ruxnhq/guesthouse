use super::*;
use crate::actions::desktop::{BOOT_LINE, SERIAL_FILE};
use crate::actions::identity_restart::RESTART_SCRIPT;
use crate::actions::tests::{machine, plain_task_context};
use crate::agent::GuestAgent;
use crate::context::Context;
use crate::guest_driver_store::{record_file, recorded_version};
use crate::machine::Machine;
use crate::testing::Scratch;
use crate::vm_args::Boot;
use crate::vm_host::stand_in::RecordingHost;
use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

const OFFERED: &str = "32.0.16.1800";
const OLDER: &str = "32.0.15.9159";

/// A guest that answers each kind of request the step makes, gives the status file's lines in turn,
/// and boots again when Windows is asked to restart.
struct Guest {
    statuses: Mutex<VecDeque<&'static str>>,
    asked: Mutex<Vec<String>>,
    serial: PathBuf,
}

impl GuestAgent for Guest {
    fn ask(&self, _port: u16, _token: &str, script: &str, _seconds: u64) -> String {
        self.asked.lock().unwrap().push(script.to_owned());
        if script == RESTART_SCRIPT {
            OpenOptions::new().append(true).open(&self.serial).unwrap().write_all(BOOT_LINE).unwrap();
            return "restarting".into();
        }
        if script.contains("WriteAllBytes") {
            return "file placed".into();
        }
        if script.contains(LAUNCHED) {
            return format!("{LAUNCHED}\r\n");
        }
        if script.contains(AGENT_MARK) {
            return AGENT_MARK.into();
        }
        if script.starts_with("Get-Content") {
            return self.statuses.lock().unwrap().pop_front().unwrap_or("").to_owned();
        }
        String::new()
    }
}

struct Setup {
    context: Context,
    machine: Machine,
    guest: Arc<Guest>,
}

/// A machine on NVIDIA's card, this PC's driver at `OFFERED`, its disc made, and a guest that says
/// `statuses` as the script's status file.
fn set_up(scratch: &Scratch, statuses: &[&'static str]) -> Setup {
    let mut context = plain_task_context(scratch);
    let folder = context.run_folder(None);
    fs::create_dir_all(&folder).unwrap();
    let serial = folder.join(SERIAL_FILE);
    fs::write(&serial, BOOT_LINE).unwrap();
    let guest = Arc::new(Guest {
        statuses: Mutex::new(statuses.iter().copied().collect()),
        asked: Mutex::default(),
        serial,
    });
    context.agent = guest.clone();
    let driver = driver_folder(&context);
    fs::create_dir_all(&driver).unwrap();
    fs::write(driver.join("nv_dispi.inf"), format!("[Version]\nDriverVer = 01/01/2026, {OFFERED}\n"))
        .unwrap();
    let disc = disc_path(&context, OFFERED);
    fs::create_dir_all(disc.parent().unwrap()).unwrap();
    fs::write(&disc, b"disc").unwrap();
    scratch.write(&format!("program/guest/{SCRIPT}"), "the script");
    let machine = machine(&context);
    crate::agent::make_token(&context, &machine).unwrap();
    Setup { context, machine, guest }
}

/// Runs the step, and says what the guest was asked and what the start's error became.
fn refresh(setup: &Setup) -> (Vec<String>, String) {
    let host = RecordingHost::default();
    let mut task = Task::new(&setup.context, &host, "start", setup.machine.clone());
    refresh_driver_store(&mut task).unwrap();
    (setup.guest.asked.lock().unwrap().clone(), std::mem::take(&mut task.error))
}

#[test]
fn a_guest_noted_at_this_pcs_version_is_not_asked_anything() {
    let scratch = Scratch::new();
    let setup = set_up(&scratch, &[]);
    record_version(&setup.context, &setup.machine, OFFERED).unwrap();
    let (asked, error) = refresh(&setup);
    assert!(asked.is_empty(), "{asked:?}");
    assert_eq!(error, "");
}

/// A machine from before the note: the guest's copy is already the version, so it is only noted.
#[test]
fn a_machine_without_a_note_is_noted_once_the_guest_says_its_version() {
    let scratch = Scratch::new();
    let setup = set_up(&scratch, &["", "store current 32.0.16.1800\r\n"]);
    let (asked, error) = refresh(&setup);
    assert_eq!(error, "");
    assert_eq!(recorded_version(&setup.context, &setup.machine).as_deref(), Some(OFFERED));
    assert!(!asked.contains(&RESTART_SCRIPT.to_owned()), "nothing changed, so nothing restarts");
    let launch = asked.iter().find(|script| script.contains(LAUNCHED)).unwrap();
    assert!(launch.contains(&format!("-Version {OFFERED} -Status C:\\gpupv\\nv-store.status")), "{launch}");
    assert!(launch.contains("Register-ScheduledTask -TaskName 'gpupv-nv-store'"), "{launch}");
    assert!(launch.contains("-AtStartup"), "the boot task runs the script before the agent");
}

#[test]
fn a_change_queued_for_the_next_boot_restarts_windows_once_and_is_noted_after() {
    let scratch = Scratch::new();
    let statuses = ["store copying", "store queued 32.0.16.1800", "store current 32.0.16.1800"];
    let setup = set_up(&scratch, &statuses);
    record_version(&setup.context, &setup.machine, OLDER).unwrap();
    let (asked, error) = refresh(&setup);
    assert_eq!(error, "");
    assert_eq!(asked.iter().filter(|script| *script == RESTART_SCRIPT).count(), 1, "{asked:?}");
    let restart = asked.iter().position(|script| script == RESTART_SCRIPT).unwrap();
    let launches: Vec<usize> =
        asked.iter().enumerate().filter(|(_, script)| script.contains(LAUNCHED)).map(|(at, _)| at).collect();
    assert_eq!(launches.len(), 2, "once before the restart, once after to see it through");
    assert!(launches[0] < restart && restart < launches[1]);
    assert!(asked[restart..launches[1]].iter().any(|script| script.contains(AGENT_MARK)));
    assert_eq!(recorded_version(&setup.context, &setup.machine).as_deref(), Some(OFFERED));
}

#[test]
fn a_guest_without_room_keeps_its_copy_and_is_asked_again_next_start() {
    let scratch = Scratch::new();
    let setup =
        set_up(&scratch, &["store no room: the driver needs 2048 MB on C:\\, and it has 900 MB free"]);
    record_version(&setup.context, &setup.machine, OLDER).unwrap();
    let (asked, error) = refresh(&setup);
    assert!(error.starts_with(KEPT_OLD), "{error}");
    assert!(error.contains("needs 2048 MB"), "{error}");
    assert!(!asked.contains(&RESTART_SCRIPT.to_owned()));
    assert_eq!(recorded_version(&setup.context, &setup.machine).as_deref(), Some(OLDER));
}

#[test]
fn a_start_whose_disc_was_not_made_leaves_the_guest_alone() {
    let scratch = Scratch::new();
    let setup = set_up(&scratch, &[]);
    fs::remove_file(disc_path(&setup.context, OFFERED)).unwrap();
    let (asked, error) = refresh(&setup);
    assert!(asked.is_empty(), "{asked:?}");
    assert_eq!(error, "");
    assert!(!record_file(&setup.context, &setup.machine).exists());
}

#[test]
fn the_driver_disc_goes_where_the_tools_disc_would_and_qemu_may_read_it() {
    let scratch = Scratch::new();
    let setup = set_up(&scratch, &[]);
    let disc = disc_path(&setup.context, OFFERED);
    let mut plan = crate::actions::plan_for(&setup.context, &setup.machine, Boot::Gpu);
    plan.driver_disc = Some(disc.clone());
    let arguments = plan.qemu_arguments();
    let drive = format!("if=none,id=cd1,media=cdrom,readonly=on,file={}", disc.display());
    assert!(arguments.contains(&drive), "{arguments:?}");
    let jail = crate::vm_args::QemuJail {
        launcher: "/bin/guesthouse-jail".into(),
        namespaces: true,
        qemu_sandbox: false,
        sound_socket: None,
        backing_files: Vec::new(),
    };
    let grants = plan.qemu_jail_plan(&jail).paths;
    assert!(
        grants.contains(&(guesthouse_jail::Grant::Read, disc)),
        "a jailed QEMU can open the disc: {grants:?}"
    );
}

// The real script, where PowerShell is installed, against folders that stand in for the store and the
// disc. Windows' own parts, the CD drives and the queue for the next boot, can't be run here.

fn pwsh() -> Option<&'static str> {
    Command::new("pwsh").arg("-v").output().ok().filter(|done| done.status.success()).map(|_| "pwsh")
}

fn script_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../guest").join(SCRIPT)
}

fn run_pwsh(command: &str) -> String {
    let done =
        Command::new("pwsh").args(["-NoProfile", "-NonInteractive", "-Command", command]).output().unwrap();
    String::from_utf8_lossy(&done.stdout).into_owned()
}

fn run(root: &Path, disc: &Path, version: &str) -> String {
    let version = if version.is_empty() { String::new() } else { format!(" -Version {version}") };
    run_pwsh(&format!(
        "& '{}'{version} -Source '{}' -StoreRoot '{}'",
        script_path().display(),
        disc.display(),
        root.display()
    ))
}

fn write_inf(folder: &Path, version: &str) {
    fs::create_dir_all(folder).unwrap();
    fs::write(folder.join("nv_dispi.inf"), format!("[Version]\r\nDriverVer=01/01/2026,{version}\r\n"))
        .unwrap();
}

fn lay_out(scratch: &Scratch) -> (PathBuf, PathBuf, PathBuf) {
    let (root, disc) = (scratch.path("FileRepository"), scratch.path("disc/nvidia"));
    let store = root.join("nv_dispi.inf_amd64_gpupv");
    write_inf(&store, OLDER);
    fs::write(store.join("nvwgf2umx.dll"), "old").unwrap();
    fs::write(store.join("gone.dll"), "a file the new driver doesn't have").unwrap();
    write_inf(&disc, OFFERED);
    fs::write(disc.join("nvwgf2umx.dll"), "newer").unwrap();
    fs::create_dir_all(disc.join("sub")).unwrap();
    fs::write(disc.join("sub").join("NvGpuComp64.dll"), "deep").unwrap();
    (root, disc, store)
}

fn names(folder: &Path) -> Vec<String> {
    let mut found = Vec::new();
    for entry in fs::read_dir(folder).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.path().is_dir() {
            found.extend(names(&entry.path()).into_iter().map(|inner| format!("{name}/{inner}")));
        } else {
            found.push(name);
        }
    }
    found.sort();
    found
}

#[test]
fn the_script_puts_the_discs_driver_in_place_of_the_old_and_leaves_nothing_behind() {
    if pwsh().is_none() {
        return;
    }
    let scratch = Scratch::new();
    let (root, disc, store) = lay_out(&scratch);
    let said = run(&root, &disc, OFFERED);
    assert!(said.contains("store copying"), "{said}");
    assert!(said.trim_end().ends_with(&format!("store replaced {OFFERED}")), "{said}");
    assert_eq!(names(&store), names(&disc), "the old driver's other files went with it");
    assert_eq!(fs::read_to_string(store.join("nvwgf2umx.dll")).unwrap(), "newer");
    assert!(
        !root.join("nv_dispi.inf_amd64_gpupv.old").exists()
            && !root.join("nv_dispi.inf_amd64_gpupv.new").exists()
    );
    let again = run(&root, &disc, "");
    assert_eq!(again.trim(), format!("store current {OFFERED}"), "the disc's own version, and nothing to do");
}

#[test]
fn the_script_leaves_the_store_alone_when_the_disc_is_another_version() {
    if pwsh().is_none() {
        return;
    }
    let scratch = Scratch::new();
    let (root, disc, store) = lay_out(&scratch);
    let before = names(&store);
    let said = run(&root, &disc, "32.0.16.9999");
    assert!(said.contains("store failed: the disc has driver 32.0.16.1800, not 32.0.16.9999"), "{said}");
    assert_eq!(names(&store), before);
    assert!(!root.join("nv_dispi.inf_amd64_gpupv.new").exists());
}

/// A change cut short between its two renames, by a power cut, left the store's place empty.
#[test]
fn the_script_puts_back_an_old_store_left_beside_its_place() {
    if pwsh().is_none() {
        return;
    }
    let scratch = Scratch::new();
    let (root, disc, store) = lay_out(&scratch);
    fs::rename(&store, root.join("nv_dispi.inf_amd64_gpupv.old")).unwrap();
    let said = run(&root, &disc, OFFERED);
    assert!(said.trim_end().ends_with(&format!("store replaced {OFFERED}")), "{said}");
    assert_eq!(names(&store), names(&disc));
}

/// A copy whose file is the wrong size is not the driver, even with the right version in its INF.
#[test]
fn the_script_copies_again_when_the_store_has_the_version_but_not_the_files() {
    if pwsh().is_none() {
        return;
    }
    let scratch = Scratch::new();
    let (root, disc, store) = lay_out(&scratch);
    write_inf(&store, OFFERED);
    let said = run(&root, &disc, OFFERED);
    assert!(said.trim_end().ends_with(&format!("store replaced {OFFERED}")), "{said}");
    assert_eq!(names(&store), names(&disc));
}

/// The queue for the next boot moves every old file out before any new one comes in.
#[test]
fn the_renames_for_the_next_boot_move_the_old_files_out_first() {
    if pwsh().is_none() {
        return;
    }
    let scratch = Scratch::new();
    let (root, disc, store) = lay_out(&scratch);
    let old = root.join("old");
    let said = run_pwsh(&format!(
        ". '{}'; Get-RenamePairs '{}' '{}' '{}' | ForEach-Object {{ $_ -join '|' }}",
        script_path().display(),
        store.display(),
        disc.display(),
        old.display()
    ));
    let pairs: Vec<(&str, &str)> = said.lines().filter_map(|line| line.split_once('|')).collect();
    assert_eq!(pairs.len(), 6, "{said}");
    let (out, into) = pairs.split_at(3);
    assert!(
        out.iter().all(|(from, to)| from.starts_with(&*store.to_string_lossy())
            && to.starts_with(&*old.to_string_lossy()))
    );
    assert!(
        into.iter().all(|(from, to)| from.starts_with(&*disc.to_string_lossy())
            && to.starts_with(&*store.to_string_lossy()))
    );
    assert!(into.iter().any(|(_, to)| to.ends_with("NvGpuComp64.dll") && to.contains("sub")));
}
