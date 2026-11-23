use super::*;
use crate::disk_image::backing::backing_files;
use crate::library::{NewMachine, create};
use crate::machine::{Drive, Kind, load, load_all};
use crate::processes::stand_in::fake_process;
use crate::remote_access::Access;
use crate::slots;
use crate::testing::{Scratch, scratch_context};
use crate::token_change::next_token_file;
use crate::vm_host::stand_in::RecordingHost;
use std::io::Write as _;
use std::process::Stdio;

fn qemu_io(disk: &str, command: &str) -> bool {
    let done = Command::new("qemu-io").args(["-f", "qcow2", "-c", command, disk]).output().unwrap();
    done.status.success() && !String::from_utf8_lossy(&done.stdout).contains("Pattern verification failed")
}

fn fill(disk: &str, byte: u8) {
    assert!(qemu_io(disk, &format!("write -P {byte} 0 64k")));
}

fn holds(disk: &str, byte: u8) -> bool {
    qemu_io(disk, &format!("read -P {byte} 0 64k"))
}

fn new_plain(scratch: &Scratch, name: &str, disk_place: &str) -> NewMachine {
    NewMachine {
        name: name.into(),
        iso: scratch.write("linux.iso", "disc").display().to_string(),
        kind: Kind::Plain,
        windows_disc: true,
        cores: 1,
        ram_gib: 1,
        disk_gib: 1,
        refresh_hz: None,
        user: String::new(),
        password: String::new(),
        amd_gpu: None,
        disk_place: disk_place.into(),
        gpu_maker: None,
        access: Access::NEW,
    }
}

fn copy_called(context: &Context, source: &Machine, name: &str) -> Result<Machine> {
    let host = RecordingHost::default();
    let mut task = Task::new(context, &host, DUPLICATE_TASK, source.clone());
    let before = sorted_names(&context.paths.machines);
    duplicate(&mut task, name, false, false)?;
    let made = sorted_names(&context.paths.machines).into_iter().find(|id| !before.contains(id));
    load(context, &made.expect("a new machine"))
}

#[test]
fn a_copy_keeps_what_the_guest_relies_on_and_gets_its_own_identity() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let mut source = create(&context, &new_plain(&scratch, "Lab", "")).unwrap();
    source.drives = vec![Drive { id: "usb-Some_Stick".into() }];
    source.start_with_pc = true;
    source.save(&context).unwrap();
    fill(&source.disk, 0x11);
    let snapshot =
        Command::new("qemu-img").args(["snapshot", "-c", "guesthouse-1", &source.disk]).status().unwrap();
    assert!(snapshot.success());
    let folder = source.folder(&context);
    scratch.write(&format!("{}/tpm/tpm2-00.permall", folder.display()), "sealed");
    scratch.write(&format!("{}/snapshots/guesthouse-1.fd", folder.display()), "firmware then");
    scratch.write(&format!("{}/answer.iso", folder.display()), "the install's");
    for secret in ["guest.token", "windows-password"] {
        let path = scratch.write(&format!("{}/{secret}", folder.display()), "secret\n");
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let copy = copy_called(&context, &source, "Lab Copy").unwrap();
    assert_eq!((copy.id.as_str(), copy.name.as_str()), ("lab-copy", "Lab Copy"));
    assert_ne!(copy.mac, source.mac);
    assert!(copy.drives.is_empty() && !copy.start_with_pc, "{copy:?}");
    assert!(copy.owns_files(&context));
    assert_eq!((copy.cores, copy.tpm, copy.iso.as_str()), (source.cores, source.tpm, source.iso.as_str()));
    let new = copy.folder(&context);
    assert_eq!(fs::read_to_string(new.join("tpm/tpm2-00.permall")).unwrap(), "sealed");
    assert_eq!(fs::read_to_string(new.join("snapshots/guesthouse-1.fd")).unwrap(), "firmware then");
    assert_eq!(fs::read(&copy.vars).unwrap(), fs::read(&source.vars).unwrap());
    assert!(!new.join("answer.iso").exists());
    assert_eq!(fs::metadata(new.join("windows-password")).unwrap().mode() & 0o777, 0o600);
    assert_eq!(fs::read_to_string(copy.token_file(&context)).unwrap(), "secret\n", "its disk knows this one");
    let (copy_next, source_next) = (next_token_file(&context, &copy), next_token_file(&context, &source));
    assert!(copy_next.exists() && source_next.exists());
    assert_eq!(fs::read(&copy_next).unwrap(), fs::read(&source_next).unwrap(), "planned before the copy");
    assert!(
        crate::settings::with_suffix(&copy_next, ".again").exists(),
        "so the copy moves on to one of its own"
    );
    assert!(disk_info(&copy.disk).unwrap()["snapshots"][0]["name"] == "guesthouse-1");
    assert!(holds(&copy.disk, 0x11));
    fill(&copy.disk, 0x22);
    assert!(holds(&source.disk, 0x11), "the copy's disk is its own");
    assert!(!staging_root(&context).exists());
    assert_eq!(load_all(&context).0.len(), 2);
}

#[test]
fn a_disk_on_another_drive_is_copied_beside_it() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let other = scratch.path("other-drive");
    fs::create_dir_all(&other).unwrap();
    let source = create(&context, &new_plain(&scratch, "Far", &other.display().to_string())).unwrap();
    let copy = copy_called(&context, &source, "Far Copy").unwrap();
    assert_eq!(Path::new(&copy.disk), other.join("far-copy/disk.qcow2"));
    assert_eq!(Path::new(&copy.disk_folder), other.join("far-copy"));
    assert!(Path::new(&copy.vars).starts_with(copy.folder(&context)) && copy.owns_files(&context));
    assert_eq!(sorted_names(&other), ["far", "far-copy"]);
}

#[test]
fn a_machine_that_does_not_own_its_disk_makes_one_that_does() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let disk = context.paths.store.join("disk/win.qcow2");
    fs::create_dir_all(disk.parent().unwrap()).unwrap();
    assert!(
        Command::new("qemu-img")
            .args(["create", "-q", "-f", "qcow2"])
            .arg(&disk)
            .arg("1G")
            .status()
            .unwrap()
            .success()
    );
    scratch.write("home/.local/share/guesthouse/disk/vars.fd", "settings");
    let imported = crate::library::import_existing(&context).unwrap().unwrap();
    let copy = copy_called(&context, &imported, "Win Copy").unwrap();
    assert!(!imported.owns_files(&context) && copy.owns_files(&context));
    assert_eq!(Path::new(&copy.disk), copy.folder(&context).join(DISK_FILE));
    assert_eq!(fs::read_to_string(&copy.vars).unwrap(), "settings");
}

#[test]
fn a_disk_built_on_another_reads_the_same_one_from_its_new_folder() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let source = create(&context, &new_plain(&scratch, "Layered", "")).unwrap();
    let base = scratch.path("home/.local/share/guesthouse/machines/layered/base.qcow2");
    assert!(
        Command::new("qemu-img")
            .args(["create", "-q", "-f", "qcow2"])
            .arg(&base)
            .arg("1G")
            .status()
            .unwrap()
            .success()
    );
    let rebased = Command::new("qemu-img")
        .args(["rebase", "-u", "-f", "qcow2", "-F", "qcow2", "-b", "base.qcow2", &source.disk])
        .status()
        .unwrap();
    assert!(rebased.success());
    let copy = copy_called(&context, &source, "Layered Copy").unwrap();
    assert_eq!(backing_files(Path::new(&copy.disk)).unwrap(), [base]);
}

#[test]
fn a_machine_that_is_running_installing_or_held_by_another_qemu_is_not_copied() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let source = create(&context, &new_plain(&scratch, "Busy", "")).unwrap();
    let qemu = context.paths.qemu.display().to_string();
    let socket = format!("unix:{}/qmp.sock,server=on", context.paths.run_folder(slots::first()).display());
    let drive = format!("file={},format=qcow2", source.disk);
    fake_process(&context, 60, &[&qemu, "-qmp", &socket, "-drive", &drive], &[], "");
    assert_eq!(plan_duplicate(&context, &source, "Two", false).unwrap_err().0, SHUT_DOWN_FIRST);
    fs::remove_dir_all(context.processes.join("60")).unwrap();
    let installing = Machine { installing: true, ..source.clone() };
    assert!(plan_duplicate(&context, &installing, "Two", false).unwrap_err().0.contains("installing"));
    let mut holder = Command::new("qemu-io")
        .args(["-f", "qcow2", &source.disk])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let held_until_open = |disk: &str| {
        (0..100).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(20));
            !disk_is_free(disk)
        })
    };
    assert!(held_until_open(&source.disk));
    assert_eq!(plan_duplicate(&context, &source, "Two", false).unwrap_err().0, DISK_IN_USE);
    holder.stdin.take().unwrap().write_all(b"quit\n").unwrap();
    holder.wait().unwrap();
    assert!(plan_duplicate(&context, &source, " ", false).unwrap_err().0.contains("needs a name"));
    plan_duplicate(&context, &source, "Two", false).unwrap();
    assert!(sorted_names(&context.paths.machines) == ["busy"], "a plan makes nothing");
}

#[test]
fn a_copy_that_fails_halfway_leaves_nothing_and_the_original_as_it_was() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let other = scratch.path("other-drive");
    fs::create_dir_all(&other).unwrap();
    let source = create(&context, &new_plain(&scratch, "Locked", &other.display().to_string())).unwrap();
    let unreadable = scratch.write("home/.local/share/guesthouse/machines/locked/private", "mine");
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(&unreadable).is_ok() {
        return; // Run as root, which reads anything.
    }
    assert!(copy_called(&context, &source, "Locked Copy").is_err());
    assert_eq!(sorted_names(&context.paths.machines), ["locked"]);
    assert_eq!(sorted_names(&other), ["locked"]);
    assert!(next_token_file(&context, &source).exists() || !source.token_file(&context).exists());
}

#[test]
fn what_a_copy_cut_short_left_is_taken_away_and_one_at_work_is_not() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let other = scratch.path("other-drive");
    let root = staging_root(&context);
    for id in ["cut", "working"] {
        scratch.write(&format!("{}/{id}/machine/vars.fd", root.display()), "half");
        scratch.write(&format!("{}/{id}/lock", root.display()), "");
        let staged = other.join(format!("{OTHER_DRIVE_PREFIX}{id}"));
        scratch.write(&format!("{}/disk.qcow2", staged.display()), "half a disk");
        fs::write(root.join(id).join(OTHER_DRIVE_NOTE), staged.to_string_lossy().as_bytes()).unwrap();
    }
    scratch.write(&format!("{}/cut/disk.qcow2", other.display()), "moved, with no machine");
    scratch.write(&format!("{}/kept/disk.qcow2", other.display()), "someone else's");
    let held = File::open(root.join("working/lock")).unwrap();
    flock(&held, FlockOperation::NonBlockingLockExclusive).unwrap();
    sweep_abandoned(&context);
    assert_eq!(sorted_names(&root), ["working"]);
    assert_eq!(sorted_names(&other), [".duplicate-working", "kept"]);
    assert!(load_all(&context).1.is_empty(), "work in progress is not a broken machine");
}

#[test]
fn the_cost_says_what_a_copy_takes() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let mut source = create(&context, &new_plain(&scratch, "Cost", "")).unwrap();
    fill(&source.disk, 0x33);
    source.drives = vec![Drive { id: "usb-Some_Stick".into() }];
    let plan = plan_duplicate(&context, &source, &copy_name(&source), false).unwrap();
    assert_eq!(plan.copy.name, "Cost Copy");
    assert!(plan.disk_gib > 0.0);
    let text = plan.cost_text();
    assert!(text.contains("GiB") && text.ends_with("Host drives stay with the original."), "{text}");
}

#[test]
fn a_throwaway_copy_starts_from_the_originals_state_and_leaves_its_run_behind() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let source = create(&context, &new_plain(&scratch, "Base", "")).unwrap();
    let folder = source.folder(&context);
    fs::write(folder.join("display.json"), "as installed").unwrap();
    scratch.write(&format!("{}/throwaway/vars.fd", folder.display()), "a run's");
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, DUPLICATE_TASK, source.clone());
    duplicate(&mut task, "Try Things", true, false).unwrap();
    let copy = load(&context, "try-things").unwrap();
    assert!(copy.throwaway && !source.throwaway);
    let new = copy.folder(&context);
    assert_eq!(fs::read_to_string(new.join("throwaway-clean/display.json")).unwrap(), "as installed");
    assert!(!new.join("throwaway").exists(), "a run's files are not the machine's");
}

#[test]
fn a_change_of_token_still_waiting_goes_with_the_copy_and_both_owe_another() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let source = create(&context, &new_plain(&scratch, "Waiting", "")).unwrap();
    let folder = source.folder(&context);
    scratch.write(&format!("{}/guest.token", folder.display()), "old\n");
    scratch.write(&format!("{}/guest.token.next", folder.display()), "middle\n");
    let copy = copy_called(&context, &source, "Waiting Copy").unwrap();
    for machine in [&source, &copy] {
        let next = next_token_file(&context, machine);
        assert_eq!(fs::read_to_string(&next).unwrap(), "middle\n", "the guest may hold it already");
        assert!(crate::settings::with_suffix(&next, ".again").exists(), "{}", machine.id);
    }
}

#[test]
fn a_copy_asked_to_go_is_deleted_by_the_next_look_once_it_has_run_and_stopped() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let source = create(&context, &new_plain(&scratch, "Base", "")).unwrap();
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, DUPLICATE_TASK, source.clone());
    assert!(duplicate(&mut task, "Plain Copy", false, true).unwrap_err().0.contains("Only a throwaway"));
    duplicate(&mut task, "Once", true, true).unwrap();
    let copy = load(&context, "once").unwrap();
    assert!(crate::throwaway::delete_finished(&context).is_empty(), "it hasn't run yet");
    crate::throwaway::note_started(&context, &copy);
    let qemu = context.paths.qemu.display().to_string();
    let socket = format!("unix:{}/qmp.sock,server=on", context.paths.run_folder(slots::first()).display());
    let drive = format!("file={},format=qcow2", copy.disk);
    fake_process(&context, 60, &[&qemu, "-qmp", &socket, "-drive", &drive], &[], "");
    assert!(crate::throwaway::delete_finished(&context).is_empty(), "it's running");
    fs::remove_dir_all(context.processes.join("60")).unwrap();
    assert_eq!(crate::throwaway::delete_finished(&context), ["once"]);
    assert!(!copy.folder(&context).exists() && source.folder(&context).exists());
}

#[test]
fn only_duplicate_marks_a_machine_to_go_and_nothing_carries_the_mark_on() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let source = create(&context, &new_plain(&scratch, "Base", "")).unwrap();
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, DUPLICATE_TASK, source.clone());
    duplicate(&mut task, "Once", true, true).unwrap();
    let mut copy = load(&context, "once").unwrap();
    crate::throwaway::note_started(&context, &copy);
    let mut task = Task::new(&context, &host, DUPLICATE_TASK, copy.clone());
    duplicate(&mut task, "Of Once", false, false).unwrap();
    assert!(
        !load(&context, "of-once").unwrap().folder(&context).join(crate::throwaway::DELETE_MARK).exists()
    );
    crate::throwaway::set_throwaway(&context, &mut copy, false).unwrap();
    assert!(crate::throwaway::delete_finished(&context).is_empty(), "made ordinary, it's kept");
    assert_eq!(load_all(&context).0.len(), 3);
}
