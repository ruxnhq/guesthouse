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
use std::path::PathBuf;
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

// The script itself runs in crates/guest-scripts, which builds on Windows too, so CI runs it there under
// Windows PowerShell 5.1 and pwsh.
