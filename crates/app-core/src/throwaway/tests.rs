use super::*;
use crate::agent::GuestAgent;
use crate::library::{NewMachine, create};
use crate::machine::{Kind, load};
use crate::processes::stand_in::fake_process;
use crate::remote_access::Access;
use crate::slots;
use crate::testing::{Scratch, scratch_context};
use crate::vm_args::Boot;
use guesthouse_jail::Grant;
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::sync::{Arc, Mutex};

fn qemu_io(disk: &str, command: &str) -> bool {
    let done = Command::new("qemu-io").args(["-f", "qcow2", "-c", command, disk]).output().unwrap();
    done.status.success() && !String::from_utf8_lossy(&done.stdout).contains("Pattern verification failed")
}

fn plain(scratch: &Scratch, context: &Context) -> Machine {
    let new = NewMachine {
        name: "Sandbox".into(),
        iso: scratch.write("disc.iso", "disc").display().to_string(),
        kind: Kind::Plain,
        windows_disc: true,
        cores: 1,
        ram_gib: 1,
        disk_gib: 1,
        refresh_hz: None,
        user: String::new(),
        password: String::new(),
        amd_gpu: None,
        disk_place: String::new(),
        gpu_maker: None,
        access: Access::NEW,
    };
    create(context, &new).unwrap()
}

fn throwaway(scratch: &Scratch, context: &Context) -> Machine {
    let mut machine = plain(scratch, context);
    set_throwaway(context, &mut machine, true).unwrap();
    machine
}

/// The overlay as another program can open it: through this process's descriptor.
fn by_descriptor(fd: &OwnedFd) -> String {
    format!("/proc/{}/fd/{}", std::process::id(), fd.as_raw_fd())
}

#[test]
fn the_setting_keeps_the_clean_state_and_an_older_app_cannot_read_it() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let mut machine = plain(&scratch, &context);
    fs::write(machine.folder(&context).join("display.json"), "as installed").unwrap();
    set_throwaway(&context, &mut machine, true).unwrap();
    let saved = fs::read_to_string(machine.folder(&context).join("machine.json")).unwrap();
    assert!(saved.contains("\"throwaway\": true"), "{saved}");
    assert!(load(&context, &machine.id).unwrap().throwaway);
    assert!(machine.summary().ends_with(", throwaway"));
    let kept = clean_folder(&context, &machine).join("display.json");
    assert_eq!(fs::read_to_string(kept).unwrap(), "as installed");
    set_throwaway(&context, &mut machine, false).unwrap();
    assert!(!clean_folder(&context, &machine).exists());
    let saved = fs::read_to_string(machine.folder(&context).join("machine.json")).unwrap();
    assert!(!saved.contains("throwaway"), "an ordinary machine's file is as before");
}

#[test]
fn a_running_or_installing_machine_keeps_its_setting() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let mut machine = plain(&scratch, &context);
    let qemu = context.paths.qemu.display().to_string();
    let socket = format!("unix:{}/qmp.sock,server=on", context.paths.run_folder(slots::first()).display());
    let drive = format!("file={},format=qcow2", machine.disk);
    fake_process(&context, 60, &[&qemu, "-qmp", &socket, "-drive", &drive], &[], "");
    assert_eq!(set_throwaway(&context, &mut machine, true).unwrap_err().0, SHUT_DOWN_FIRST);
    fs::remove_dir_all(context.processes.join("60")).unwrap();
    machine.installing = true;
    assert!(set_throwaway(&context, &mut machine, true).unwrap_err().0.contains("installing"));
    assert!(!machine.throwaway && !load(&context, &machine.id).unwrap().throwaway);
}

#[test]
fn a_run_writes_to_an_overlay_with_no_name_and_the_disk_stays_clean() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let machine = throwaway(&scratch, &context);
    assert!(qemu_io(&machine.disk, "write -P 17 0 64k"));
    scratch.write(&format!("{}/permall", machine.tpm_folder(&context).display()), "sealed");
    let run = ThrowawayRun::prepare(&context, &machine).unwrap();
    let left: Vec<String> = fs::read_dir(machine.folder(&context))
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(OVERLAY_PREFIX))
        .collect();
    assert!(left.is_empty(), "{left:?}");
    let [writing, reading] = run.overlay();
    assert_eq!(fs::metadata(by_descriptor(writing)).unwrap().nlink(), 0, "nothing has its name");
    assert_eq!(
        fs::metadata(by_descriptor(reading)).unwrap().ino(),
        fs::metadata(by_descriptor(writing)).unwrap().ino()
    );
    let overlay = by_descriptor(writing);
    assert!(qemu_io(&overlay, "read -P 17 0 64k"), "the run reads the disk through it");
    assert!(qemu_io(&overlay, "write -P 34 0 64k"));
    assert!(qemu_io(&machine.disk, "read -P 17 0 64k"), "the disk is as it was");
    let plan = VmPlan::new(&context, &machine, Boot::Plain { iso: String::new() }, slots::first(), 0);
    let applied = run.applied_to(&plan).unwrap();
    assert_eq!(
        (applied.disk.as_str(), applied.throwaway.as_deref()),
        ("/dev/fdset/50", Some(machine.disk.as_str()))
    );
    let folder = run_folder(&context, &machine);
    assert_eq!(Path::new(&applied.vars), folder.join(VARS_FILE));
    assert_eq!(fs::read(&applied.vars).unwrap(), fs::read(&machine.vars).unwrap());
    assert_eq!(fs::read_to_string(folder.join("tpm/permall")).unwrap(), "sealed");
}

#[test]
fn each_run_starts_from_the_notes_kept_when_it_became_a_throwaway() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let folder = plain(&scratch, &context).folder(&context);
    fs::write(folder.join("gpu-identity.json"), "microsoft's").unwrap();
    let mut machine = load(&context, "sandbox").unwrap();
    set_throwaway(&context, &mut machine, true).unwrap();
    fs::write(folder.join("gpu-identity.json"), "nvidia's, as the run set it up").unwrap();
    fs::write(folder.join("nvme-refused"), "").unwrap();
    fs::write(folder.join(crate::recipe::OUTCOME_FILE), "steps run again in the run").unwrap();
    ThrowawayRun::prepare(&context, &machine).unwrap();
    assert_eq!(fs::read_to_string(folder.join("gpu-identity.json")).unwrap(), "microsoft's");
    assert!(!folder.join("nvme-refused").exists());
    assert!(!folder.join(crate::recipe::OUTCOME_FILE).exists(), "what the run installed is gone");
}

#[test]
fn a_snapshot_keeps_the_clean_notes_and_going_back_makes_its_notes_the_clean_ones() {
    use crate::snapshots::{go_back, list_snapshots, snapshot_file, take_snapshot};
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let folder = plain(&scratch, &context).folder(&context);
    fs::write(folder.join("display.json"), "as installed").unwrap();
    let mut machine = load(&context, "sandbox").unwrap();
    set_throwaway(&context, &mut machine, true).unwrap();
    fs::write(folder.join("display.json"), "a run's").unwrap();
    let host = crate::vm_host::stand_in::RecordingHost::default();
    let mut task = crate::actions::Task::new(&context, &host, "take_snapshot", machine.clone());
    take_snapshot(&mut task, "Before").unwrap();
    let tag = list_snapshots(&context, &machine)[0].tag.clone();
    let kept = snapshot_file(&context, &machine, &tag, ".state").join("display.json");
    assert_eq!(fs::read_to_string(kept).unwrap(), "as installed", "the disk's notes, not the run's");
    let clean = folder.join("throwaway-clean");
    fs::write(clean.join("display.json"), "a later clean state").unwrap();
    fs::write(clean.join("nvme-refused"), "").unwrap();
    go_back(&mut task, &tag).unwrap();
    assert_eq!(fs::read_to_string(clean.join("display.json")).unwrap(), "as installed");
    assert!(!clean.join("nvme-refused").exists());
    ThrowawayRun::prepare(&context, &machine).unwrap();
    assert_eq!(fs::read_to_string(folder.join("display.json")).unwrap(), "as installed");
}

#[test]
fn turned_back_into_an_ordinary_machine_its_notes_say_what_its_disk_holds() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let folder = plain(&scratch, &context).folder(&context);
    fs::write(folder.join("display.json"), "as installed").unwrap();
    let mut machine = load(&context, "sandbox").unwrap();
    set_throwaway(&context, &mut machine, true).unwrap();
    fs::write(folder.join("display.json"), "a run's").unwrap();
    fs::write(folder.join(crate::recipe::OUTCOME_FILE), "steps run again in the run").unwrap();
    set_throwaway(&context, &mut machine, false).unwrap();
    assert_eq!(fs::read_to_string(folder.join("display.json")).unwrap(), "as installed");
    assert!(!folder.join(crate::recipe::OUTCOME_FILE).exists(), "what the run installed is gone");
    assert!(!folder.join("throwaway-clean").exists());
}

#[test]
fn qemu_finds_the_overlay_by_its_set_and_the_app_finds_the_machine_by_the_note() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let machine = Machine { throwaway: true, ..Machine::new("odd", "Odd", "/m/a,b/disk.qcow2", "/m/v") };
    let plan = VmPlan {
        throwaway: Some(machine.disk.clone()),
        disk: throwaway_disk(),
        ..VmPlan::new(&context, &machine, Boot::Plain { iso: String::new() }, slots::first(), 0)
    };
    let words = plan.qemu_arguments();
    let note = "fd=50,set=50,opaque=guesthouse-throwaway:/m/a,,b/disk.qcow2";
    assert!(words.windows(2).any(|pair| pair == ["-add-fd", note]), "{words:?}");
    assert!(words.windows(2).any(|pair| pair == ["-add-fd", "fd=51,set=50"]));
    assert!(words.iter().any(|word| word.contains("file=/dev/fdset/50,")));
    let qemu = context.paths.qemu.display().to_string();
    let socket = format!("unix:{}/qmp.sock,server=on", context.paths.run_folder(slots::first()).display());
    let mut running: Vec<&str> = vec![&qemu, "-qmp", &socket];
    running.extend(words.iter().map(String::as_str));
    fake_process(&context, 61, &running, &[], "");
    assert!(crate::processes::running_machine(&context, &machine).is_some());
    let other = Machine { disk: "/m/a".into(), ..machine };
    assert!(crate::processes::running_machine(&context, &other).is_none());
}

#[test]
fn the_jail_lets_qemu_read_the_disk_and_write_nothing_by_the_overlays_name() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let machine = throwaway(&scratch, &context);
    let plan = VmPlan::new(&context, &machine, Boot::Gpu, slots::first(), 0);
    let run = ThrowawayRun::prepare(&context, &machine).unwrap();
    let plan = run.applied_to(&plan).unwrap();
    let jail = crate::vm_args::QemuJail {
        launcher: "/l".into(),
        namespaces: true,
        qemu_sandbox: false,
        sound_socket: None,
        backing_files: Vec::new(),
    };
    let jail = plan.with_backing_files(jail).unwrap();
    assert_eq!(jail.backing_files, [PathBuf::from(&machine.disk)]);
    let granted = plan.qemu_jail_plan(&jail);
    let grants = |path: &str| {
        granted
            .paths
            .iter()
            .filter(|(_, at)| at == Path::new(path))
            .map(|(grant, _)| *grant)
            .collect::<Vec<_>>()
    };
    assert_eq!(grants(&machine.disk), [Grant::Read]);
    assert!(grants("/dev/fdset/50").is_empty());
    assert_eq!(grants(&plan.vars), [Grant::Write]);
}

#[test]
fn a_jail_chosen_before_the_run_still_lets_qemu_read_the_disk() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let machine = throwaway(&scratch, &context);
    let jail = crate::vm_args::QemuJail {
        launcher: "/l".into(),
        namespaces: true,
        qemu_sandbox: false,
        sound_socket: None,
        backing_files: Vec::new(),
    };
    // Ubuntu and macOS choose their jail before the host puts the run in place.
    let plan = VmPlan::new(&context, &machine, Boot::Plain { iso: String::new() }, slots::first(), 0);
    let plan = VmPlan { jail: Some(plan.with_backing_files(jail).unwrap()), ..plan };
    let run = ThrowawayRun::prepare(&context, &machine).unwrap();
    let plan = run.applied_to(&plan).unwrap();
    let jail = plan.jail.clone().unwrap();
    assert_eq!(jail.backing_files, [PathBuf::from(&machine.disk)]);
    let granted = plan.qemu_jail_plan(&jail);
    let grants: Vec<_> = granted
        .paths
        .iter()
        .filter(|(_, at)| at == Path::new(&machine.disk))
        .map(|(grant, _)| *grant)
        .collect();
    assert_eq!(grants, [Grant::Read]);
}

/// A guest's agent that answers only to its token, and takes a new one as the real one does.
struct Guest(Mutex<String>);

impl GuestAgent for Guest {
    fn ask(&self, _port: u16, token: &str, script: &str, _seconds: u64) -> String {
        let mut held = self.0.lock().unwrap();
        if *held != token {
            return String::new();
        }
        if let Some(new) = crate::token_change::token_in_change(script) {
            *held = new.to_owned();
            return "token changed\n".to_owned();
        }
        "up\n".to_owned()
    }
}

#[test]
fn each_run_moves_the_guest_to_a_token_of_its_own_and_the_machines_stays() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    let guest = Arc::new(Guest(Mutex::new("base".into())));
    context.agent = guest.clone();
    let machine = throwaway(&scratch, &context);
    fs::write(machine.token_file(&context), "base\n").unwrap();
    fs::write(crate::token_change::next_token_file(&context, &machine), "from a duplicate\n").unwrap();
    ThrowawayRun::prepare(&context, &machine).unwrap();
    crate::token_change::finish_token_change(&context, &machine, 1);
    let run = fs::read_to_string(run_token_file(&context, &machine)).unwrap();
    assert_eq!(*guest.0.lock().unwrap(), run.trim());
    assert_eq!(
        fs::read_to_string(machine.token_file(&context)).unwrap(),
        "base\n",
        "the clean disk knows it"
    );
    assert!(crate::token_change::next_token_file(&context, &machine).exists(), "never taken by a throwaway");
    assert_eq!(context.ask_guest(&machine, "'up'", 1, None), "up\n");
    // The next run's guest starts back on the machine's token.
    *guest.0.lock().unwrap() = "base".into();
    ThrowawayRun::prepare(&context, &machine).unwrap();
    assert_eq!(context.ask_guest(&machine, "'up'", 1, None), "up\n");
}

#[test]
fn an_install_keeps_the_setting_and_its_notes_become_the_clean_ones() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let machine = throwaway(&scratch, &context);
    let folder = machine.folder(&context);
    // A refused install changes nothing of it.
    let host = crate::vm_host::stand_in::RecordingHost::default();
    let refused = crate::actions::run_task(&context, &host, "install", &machine.id, &[]);
    assert!(refused.is_err());
    assert!(load(&context, &machine.id).unwrap().throwaway && clean_folder(&context, &machine).is_dir());
    fs::write(folder.join("display.json"), "as the new install left it").unwrap();
    installed(&context, &machine).unwrap();
    assert_eq!(
        fs::read_to_string(clean_folder(&context, &machine).join("display.json")).unwrap(),
        "as the new install left it"
    );
}

#[test]
fn a_macs_opencore_image_and_qemus_home_are_a_runs_own_copies() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let mut machine = throwaway(&scratch, &context);
    let folder = machine.folder(&context);
    machine.opencore =
        scratch.write(&format!("{}/opencore.qcow2", folder.display()), "efi partition").display().to_string();
    scratch.write(&format!("{}/gpu-cache/shaders", folder.display()), "compiled before");
    machine.save(&context).unwrap();
    let run = ThrowawayRun::prepare(&context, &machine).unwrap();
    let boot = crate::vm_args::MacosBoot {
        opencore: machine.opencore.clone().into(),
        firmware_code: folder.join("OVMF_CODE.fd"),
        installer: String::new(),
        rom: "/p/rom".into(),
        card: crate::vm_args::MacosCard { nodes: Vec::new(), numbers: None },
        home: folder.join("gpu-cache"),
    };
    let plan = VmPlan::new(&context, &machine, Boot::Macos(boot), slots::first(), 0);
    let Boot::Macos(applied) = run.applied_to(&plan).unwrap().boot else { panic!("still a Mac") };
    let own = run_folder(&context, &machine);
    assert_eq!(
        (applied.opencore.as_path(), applied.home.as_path()),
        (own.join("opencore.qcow2").as_path(), own.join("gpu-cache").as_path())
    );
    assert_eq!(applied.firmware_code, folder.join("OVMF_CODE.fd"), "read only, so the machine's own");
    fs::write(&applied.opencore, "the guest wrote this").unwrap();
    fs::write(applied.home.join("shaders"), "the run's").unwrap();
    assert_eq!(fs::read_to_string(&machine.opencore).unwrap(), "efi partition");
    assert_eq!(fs::read_to_string(folder.join("gpu-cache/shaders")).unwrap(), "compiled before");
}

#[test]
fn an_overlay_is_this_users_alone_and_only_this_machines_leftovers_go() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let machine = throwaway(&scratch, &context);
    let folder = machine.folder(&context);
    let mine = scratch.write(&format!("{}/.throwaway-sandbox-0a0b0c0d.qcow2", folder.display()), "left");
    let other = scratch.write(&format!("{}/.throwaway-other-0a0b0c0d.qcow2", folder.display()), "theirs");
    let user = scratch.write(&format!("{}/.throwaway-sandbox-notes.qcow2", folder.display()), "the user's");
    let run = ThrowawayRun::prepare(&context, &machine).unwrap();
    assert!(!mine.exists() && other.exists() && user.exists());
    let mode = fs::metadata(by_descriptor(&run.overlay()[0])).unwrap().mode() & 0o777;
    assert_eq!(mode, 0o600);
}
