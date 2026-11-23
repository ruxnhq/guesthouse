use super::*;
use crate::carry::Writer;
use crate::export::tests::{exportable, exported};
use crate::gpu_maker::GpuMaker;
use crate::machine::load;
use crate::network::{Forward, Protocol};
use crate::testing::{Scratch, scratch_context};
use crate::token_change::next_token_file;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

/// A PC with sixteen cores and 64 GiB, whatever the one the test runs on has, so a machine comes
/// across with the cores and RAM it had and no note about them.
fn roomy_pc(scratch: &Scratch) -> Context {
    let mut context = scratch_context(scratch);
    context.cpus = crate::host::stand_in::fake_cpus(scratch, "cpus", &[32768; 32]);
    context.meminfo = scratch.write("meminfo", "MemTotal: 67108864 kB\nMemAvailable: 60000000 kB\n");
    context
}

fn imported(context: &Context, file: &Path, name: Option<&str>) -> Result<Imported> {
    import(context, file, name, &mut |_| {})
}

fn holds(disk: &str, byte: u8) -> bool {
    let read = format!("read -P {byte} 0 64k");
    let done = Command::new("qemu-io").args(["-f", "qcow2", "-r", "-c", &read, disk]).output().unwrap();
    done.status.success() && !String::from_utf8_lossy(&done.stdout).contains("verification failed")
}

/// An export written by hand, with these settings and files.
fn crafted(scratch: &Scratch, settings: &Machine, files: &[(&str, Vec<u8>)]) -> PathBuf {
    let path = scratch.path("crafted.guesthouse");
    let mut writer = Writer::new(File::create(&path).unwrap()).unwrap();
    let text = serde_json::to_vec(settings).unwrap();
    writer.file(MACHINE_FILE, text.len() as u64, &mut &text[..]).unwrap();
    for (name, data) in files {
        writer.file(name, data.len() as u64, &mut &data[..]).unwrap();
    }
    writer.end().unwrap();
    path
}

fn qcow2(scratch: &Scratch, name: &str, options: &[&str]) -> Vec<u8> {
    let path = scratch.path(name);
    let done = Command::new("qemu-img")
        .args(["create", "-q", "-f", "qcow2"])
        .args(options)
        .arg(&path)
        .arg("64M")
        .status()
        .unwrap();
    assert!(done.success());
    fs::read(path).unwrap()
}

fn plain_settings() -> Machine {
    Machine { kind: Kind::Plain, ..Machine::new("", "From Afar", DISK_FILE, FIRMWARE_FILE) }
}

/// Nothing of a refused import is left: no machine, no half-made folder.
fn nothing_left(context: &Context) {
    let names = crate::machine::sorted_names(&context.paths.machines);
    assert!(names.is_empty(), "{names:?}");
}

#[test]
fn a_machine_exported_on_one_pc_is_whole_on_another() {
    let (here, there) = (Scratch::new(), Scratch::new());
    let (from, to) = (roomy_pc(&here), roomy_pc(&there));
    let machine = exportable(&here, &from);
    let kept = machine.folder(&from).join("snapshots");
    let display = serde_json::to_string(&crate::display::DisplayState::default()).unwrap();
    here.write(&format!("{}/guesthouse-0a1b2c3d.state/display.json", kept.display()), &display);
    here.write(&format!("{}/guesthouse-0a1b2c3d.state/nvme-trial", kept.display()), "");
    here.write(&format!("{}/guesthouse-0a1b2c3d.state/recipe-result.json", kept.display()), "{}");
    let before = r#"{"name": "Before", "auto": true, "nvme": true}"#;
    here.write(&format!("{}/guesthouse-0a1b2c3d.json", kept.display()), before);
    for stays in ["recipe.json", "recipe-result.json", "auto-snapshot.json"] {
        here.write(&format!("{}/{stays}", machine.folder(&from).display()), "{\"file\": \"/home\"}");
    }
    for bookkeeping in ["guest.token.next.again", "guest.token.lock", "guest.token.refused"] {
        here.write(&format!("{}/{bookkeeping}", machine.folder(&from).display()), "");
    }
    let file = here.path("carry-me.guesthouse");
    exported(&from, &machine, &file).unwrap();
    let made = imported(&to, &file, None).unwrap();
    let copy = load(&to, "carry-me").unwrap();
    assert_eq!(made.machine, copy);
    assert_eq!((copy.name.as_str(), copy.cores, copy.tpm), ("Carry Me", machine.cores, machine.tpm));
    assert!(
        copy.share.is_empty() && copy.drives.is_empty() && copy.forwards.is_empty() && copy.iso.is_empty()
    );
    assert!(
        made.notes.is_empty(),
        "an export holds nothing of the other PC's to leave out: {:?}",
        made.notes
    );
    assert!(copy.owns_files(&to));
    assert!(holds(&copy.disk, 7));
    let snapshot = &crate::snapshots::list_snapshots(&to, &copy)[0];
    assert_eq!(snapshot.name, "Before");
    assert!(!snapshot.automatic, "kept as the user's, so nothing deletes it by itself");
    let folder = copy.folder(&to);
    let saved = fs::read_to_string(folder.join("snapshots/guesthouse-0a1b2c3d.json")).unwrap();
    assert!(saved.contains(r#""nvme":true"#), "going back puts its disk where it boots: {saved}");
    let state = folder.join("snapshots/guesthouse-0a1b2c3d.state");
    assert!(state.join("display.json").is_file() && state.join("nvme-trial").is_file());
    for stays in ["recipe.json", "recipe-result.json", "auto-snapshot.json"] {
        assert!(!folder.join(stays).exists(), "{stays}");
    }
    assert_eq!(fs::read_to_string(folder.join("tpm/tpm2-00.permall")).unwrap(), "sealed");
    assert_eq!(fs::read(&copy.vars).unwrap(), fs::read(&machine.vars).unwrap());
    assert_eq!(fs::metadata(folder.join("windows-password")).unwrap().mode() & 0o777, 0o600);
    assert_eq!(fs::read_to_string(copy.token_file(&to)).unwrap(), "0123456789abcdef0123456789abcdef\n");
    assert!(next_token_file(&to, &copy).exists(), "the guest gets a token of its own here");
    let again = imported(&to, &file, Some("Second")).unwrap();
    assert_eq!(again.machine.id, "second");
    assert_ne!(again.machine.mac, copy.mac);
}

#[test]
fn settings_that_name_this_pc_are_never_taken_from_an_export() {
    let scratch = Scratch::new();
    let context = roomy_pc(&scratch);
    let settings = Machine {
        disk: "/etc/shadow".into(),
        vars: "/home/someone/vars.fd".into(),
        share: "/".into(),
        drives: vec![crate::machine::Drive { id: "nvme-Your_System_Disk".into() }],
        forwards: vec![Forward::new(Protocol::Tcp, 22, 22)],
        network_mode: Mode::Bridged,
        mac: "52:54:00:00:00:01".into(),
        start_without_jail: true,
        start_with_pc: true,
        clipboard: true,
        linux_gpu: true,
        vcache_pin: true,
        disk_folder: "/".into(),
        ..plain_settings()
    };
    let file = crafted(
        &scratch,
        &settings,
        &[("vars.fd", b"vars".to_vec()), (DISK_FILE, qcow2(&scratch, "d", &[]))],
    );
    let made = imported(&context, &file, None).unwrap().machine;
    assert!(made.disk.starts_with(&context.paths.machines.display().to_string()), "{}", made.disk);
    assert!(made.vars.starts_with(&context.paths.machines.display().to_string()));
    assert!(
        made.share.is_empty()
            && made.drives.is_empty()
            && made.forwards.is_empty()
            && made.disk_folder.is_empty()
    );
    assert_eq!(made.network_mode, Mode::Nat);
    assert!(
        !made.start_without_jail
            && !made.start_with_pc
            && !made.clipboard
            && !made.linux_gpu
            && !made.vcache_pin
    );
    assert_ne!(made.mac, "52:54:00:00:00:01");
    assert_eq!(imported_notes(&context, &file).len(), 3, "the bridge, the opt-ins and what names that PC");
}

#[test]
fn a_disk_that_reaches_past_itself_is_refused() {
    let scratch = Scratch::new();
    let context = roomy_pc(&scratch);
    let secret = scratch.write("secret.img", vec![1; 4096]);
    let backed = qcow2(&scratch, "backed", &["-F", "raw", "-b", &secret.display().to_string()]);
    let file = crafted(&scratch, &plain_settings(), &[("vars.fd", b"v".to_vec()), (DISK_FILE, backed)]);
    assert!(imported(&context, &file, None).unwrap_err().0.contains("built on another file"));
    nothing_left(&context);
    let data_file = scratch.path("outside.raw");
    let external = qcow2(&scratch, "external", &["-o", &format!("data_file={}", data_file.display())]);
    let file = crafted(&scratch, &plain_settings(), &[("vars.fd", b"v".to_vec()), (DISK_FILE, external)]);
    assert!(imported(&context, &file, None).unwrap_err().0.contains("another file"));
    nothing_left(&context);
    let raw = vec![0; 1 << 20];
    let file = crafted(&scratch, &plain_settings(), &[("vars.fd", b"v".to_vec()), (DISK_FILE, raw)]);
    assert!(imported(&context, &file, None).unwrap_err().0.contains("isn't a qcow2 disk"));
    nothing_left(&context);
}

#[test]
fn a_snapshot_whose_name_would_lead_out_of_the_folder_is_refused() {
    let scratch = Scratch::new();
    let context = roomy_pc(&scratch);
    let disk = scratch.path("snapped.qcow2");
    assert!(
        Command::new("qemu-img")
            .args(["create", "-q", "-f", "qcow2"])
            .arg(&disk)
            .arg("64M")
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("qemu-img")
            .args(["snapshot", "-c", "../../../.bashrc"])
            .arg(&disk)
            .status()
            .unwrap()
            .success()
    );
    let file = crafted(
        &scratch,
        &plain_settings(),
        &[("vars.fd", b"v".to_vec()), (DISK_FILE, fs::read(&disk).unwrap())],
    );
    assert!(imported(&context, &file, None).unwrap_err().0.contains("snapshot"));
    nothing_left(&context);
}

#[test]
fn a_file_named_outside_the_folder_said_twice_or_cut_short_leaves_nothing() {
    let scratch = Scratch::new();
    let context = roomy_pc(&scratch);
    let disk = qcow2(&scratch, "d", &[]);
    let mut evil = crafted(&scratch, &plain_settings(), &[("vars.fd", b"v".to_vec())]);
    let mut bytes = fs::read(&evil).unwrap();
    let end = bytes.len() - b"{\"end\":true}\n".len();
    bytes.truncate(end);
    bytes.extend_from_slice(b"{\"path\":\"../escaped\",\"size\":1}\nx");
    fs::write(&evil, &bytes).unwrap();
    assert!(imported(&context, &evil, None).unwrap_err().0.contains("can't be made safely"));
    assert!(
        !context.paths.machines.join("escaped").exists()
            && !context.paths.machines.parent().unwrap().join("escaped").exists()
    );
    nothing_left(&context);
    evil = crafted(&scratch, &plain_settings(), &[("vars.fd", b"v".to_vec()), ("vars.fd", b"w".to_vec())]);
    assert!(imported(&context, &evil, None).unwrap_err().0.contains("more than once"));
    nothing_left(&context);
    let whole = crafted(&scratch, &plain_settings(), &[("vars.fd", b"v".to_vec()), (DISK_FILE, disk)]);
    let mut cut = fs::read(&whole).unwrap();
    cut.truncate(cut.len() / 2);
    fs::write(&whole, &cut).unwrap();
    assert!(imported(&context, &whole, None).unwrap_err().0.contains("ends early"));
    nothing_left(&context);
}

#[test]
fn a_token_or_password_that_is_not_the_apps_is_left_out() {
    let scratch = Scratch::new();
    let context = roomy_pc(&scratch);
    let files = [
        ("vars.fd", b"v".to_vec()),
        ("guest.token", b"abc\n$(rm -rf ~)\n".to_vec()),
        ("guest.token.next", b"chosen by them\n".to_vec()),
        ("windows-password", b"one\ntwo\n".to_vec()),
        (DISK_FILE, qcow2(&scratch, "d", &[])),
    ];
    let file = crafted(&scratch, &plain_settings(), &files);
    let made = imported(&context, &file, None).unwrap();
    let folder = made.machine.folder(&context);
    assert!(!folder.join("guest.token").exists() && !folder.join("guest.token.next").exists());
    assert!(!folder.join("windows-password").exists());
    assert_eq!(made.notes.len(), 2, "{:?}", made.notes);
    for entry in fs::read_dir(&folder).unwrap().flatten() {
        if entry.file_type().unwrap().is_file() && entry.file_name() != "machine.json" {
            assert_eq!(
                entry.metadata().unwrap().permissions().mode() & 0o777,
                0o600,
                "{:?}",
                entry.file_name()
            );
        }
    }
}

#[test]
fn windows_for_a_maker_this_pc_has_no_card_of_is_refused_plainly() {
    let scratch = Scratch::new();
    let context = roomy_pc(&scratch);
    let mut settings = Machine::new("", "Radeon", DISK_FILE, FIRMWARE_FILE);
    settings.use_windows_card(GpuMaker::Amd, "0000:03:00.0");
    let file =
        crafted(&scratch, &settings, &[("vars.fd", b"v".to_vec()), (DISK_FILE, qcow2(&scratch, "d", &[]))]);
    assert!(imported(&context, &file, None).unwrap_err().0.contains("AMD card, and this PC has none"));
    nothing_left(&context);
}

#[test]
fn what_is_not_an_export_is_said_so() {
    let scratch = Scratch::new();
    let context = roomy_pc(&scratch);
    let zip = scratch.write("photos.zip", "PK\x03\x04");
    assert!(imported(&context, &zip, None).unwrap_err().0.contains("not a machine exported"));
    let missing = scratch.path("gone.guesthouse");
    assert!(imported(&context, &missing, None).unwrap_err().0.contains("could not be read"));
    let mut newer = serde_json::to_value(plain_settings()).unwrap();
    newer["a_setting_from_later"] = true.into();
    let path = scratch.path("newer.guesthouse");
    let mut writer = Writer::new(File::create(&path).unwrap()).unwrap();
    let text = serde_json::to_vec(&newer).unwrap();
    writer.file(MACHINE_FILE, text.len() as u64, &mut &text[..]).unwrap();
    writer.end().unwrap();
    assert!(imported(&context, &path, None).unwrap_err().0.contains("newer Guesthouse"));
    let _ = fs::metadata(&path).unwrap().mode();
}

fn imported_notes(context: &Context, file: &Path) -> Vec<String> {
    imported(context, file, Some("Again")).unwrap().notes
}

#[test]
fn any_qcow2_in_an_export_is_checked_before_qemu_img_opens_it() {
    let scratch = Scratch::new();
    let context = roomy_pc(&scratch);
    let secret = scratch.write("secret.img", vec![1; 4096]);
    let backed = qcow2(&scratch, "backed", &["-F", "raw", "-b", &secret.display().to_string()]);
    let settings = Machine { opencore: "opencore.qcow2".into(), ..plain_settings() };
    let files =
        [("vars.fd", b"v".to_vec()), ("opencore.qcow2", backed), (DISK_FILE, qcow2(&scratch, "d", &[]))];
    let file = crafted(&scratch, &settings, &files);
    assert!(imported(&context, &file, None).unwrap_err().0.contains("built on another file"));
    nothing_left(&context);
}

#[test]
fn only_files_the_app_knows_are_written_and_each_is_checked_or_made_again() {
    let scratch = Scratch::new();
    let context = roomy_pc(&scratch);
    let files = [
        ("vars.fd", b"v".to_vec()),
        ("remote-access.json", br#"{"ssh": true, "remote_desktop": true}"#.to_vec()),
        ("throwaway-clean/display.json", b"{}".to_vec()),
        ("throwaway/vars.fd", b"a run's".to_vec()),
        ("thumbnail.png", b"\x89PNG crafted".to_vec()),
        ("gpu-cache/shader", b"compiled elsewhere".to_vec()),
        ("notes.txt", b"anything".to_vec()),
        ("display.json", b"not a note of the app's".to_vec()),
        ("nvme-refused", b"some text".to_vec()),
        (
            "snapshots/guesthouse-0a1b2c3d.json",
            br#"{"name": "Before", "path": "/etc", "nvme": "yes"}"#.to_vec(),
        ),
        ("snapshots/guesthouse-0a1b2c3d.state/nvme-refused", b"some text".to_vec()),
        ("snapshots/guesthouse-0a1b2c3d.state/display.json", b"not a note of the app's".to_vec()),
        ("snapshots/guesthouse-0a1b2c3d.state/run.ps1", b"anything".to_vec()),
        ("snapshots/other.state/nvme-trial", b"".to_vec()),
        ("recipe.json", br#"{"recipe": {"steps": {"script": "Remove-Item C:\\ -Recurse"}}}"#.to_vec()),
        ("recipe-result.json", br#"{"state": "done"}"#.to_vec()),
        ("auto-snapshot.json", br#"{"tag": "guesthouse-0a1b2c3d", "pending": true}"#.to_vec()),
        ("snapshots/other.fd", b"not a tag".to_vec()),
        ("tpm/tpm2-00.permall", b"sealed".to_vec()),
        (DISK_FILE, qcow2(&scratch, "d", &[])),
    ];
    let file = crafted(&scratch, &plain_settings(), &files);
    let made = imported(&context, &file, None).unwrap();
    let folder = made.machine.folder(&context);
    for never in [
        "remote-access.json",
        "throwaway-clean",
        "throwaway",
        "thumbnail.png",
        "gpu-cache",
        "notes.txt",
        "snapshots/other.fd",
        "display.json",
        "snapshots/guesthouse-0a1b2c3d.state/display.json",
        "snapshots/guesthouse-0a1b2c3d.state/run.ps1",
        "snapshots/other.state",
        "recipe.json",
        "recipe-result.json",
        "auto-snapshot.json",
    ] {
        assert!(!folder.join(never).exists(), "{never}");
    }
    assert_eq!(fs::read_to_string(folder.join("nvme-refused")).unwrap(), "");
    let refused = folder.join("snapshots/guesthouse-0a1b2c3d.state/nvme-refused");
    assert_eq!(fs::read_to_string(refused).unwrap(), "");
    assert_eq!(
        fs::read_to_string(folder.join("snapshots/guesthouse-0a1b2c3d.json")).unwrap(),
        r#"{"name":"Before"}"#
    );
    assert_eq!(fs::read_to_string(folder.join("tpm/tpm2-00.permall")).unwrap(), "sealed");
    assert!(made.notes.iter().any(|note| note.starts_with("12 files")), "{:?}", made.notes);
    let throwaway = Machine { throwaway: true, ..plain_settings() };
    let file = crafted(&scratch, &throwaway, &files);
    let made = imported(&context, &file, Some("Clean")).unwrap();
    let clean = made.machine.folder(&context).join("throwaway-clean");
    assert!(clean.is_dir() && !clean.join("display.json").exists(), "made here from what it holds");
}

#[test]
fn cores_and_ram_past_what_this_pc_has_are_brought_down_to_it() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let settings = Machine { cores: 100_000, ram_gib: 4_000_000, ..plain_settings() };
    let file =
        crafted(&scratch, &settings, &[("vars.fd", b"v".to_vec()), (DISK_FILE, qcow2(&scratch, "d", &[]))]);
    let made = imported(&context, &file, None).unwrap();
    let cores = crate::host::physical_cores(&context.cpus);
    let ram = crate::host::total_ram(&context) / crate::host::GIB;
    assert_eq!(
        (u64::from(made.machine.cores), u64::from(made.machine.ram_gib)),
        (u64::from(cores.max(1)), ram.max(1))
    );
    assert!(made.notes.iter().any(|note| note.contains("cores or RAM")));
}

#[test]
fn lean_windows_comes_across_made_again_and_defender_is_asked_of_the_guest() {
    let scratch = Scratch::new();
    let context = roomy_pc(&scratch);
    let files = [
        ("vars.fd", b"v".to_vec()),
        ("lean.json", br#"{"lean": true}"#.to_vec()),
        (
            "lean-result.json",
            br#"{"taken_out": ["Microsoft.BingNews", "Evil.App"], "still_back": "x"}"#.to_vec(),
        ),
        ("scan-exclusions.json", br#"{"folders": ["C:\\"], "applied": true}"#.to_vec()),
        ("scan-exclusions-look", b"".to_vec()),
        (DISK_FILE, qcow2(&scratch, "d", &[])),
    ];
    let file = crafted(&scratch, &plain_settings(), &files);
    let made = imported(&context, &file, None).unwrap();
    let folder = made.machine.folder(&context);
    for never in ["scan-exclusions.json", "scan-exclusions-look", "lean.json", "lean-result.json"] {
        assert!(!folder.join(never).exists(), "{never}: a plain VM has no Windows to be lean or scanned");
    }
    let windows = scratch.path("windows-import");
    fs::create_dir_all(&windows).unwrap();
    fs::write(windows.join("lean.json"), r#"{"lean": true}"#).unwrap();
    guest_facts(&windows, Kind::Windows).unwrap();
    assert!(windows.join(crate::scan_exclusions::LOOK_MARK).exists(), "its first start asks the guest");
    assert!(windows.join("lean.json").exists());
}

#[test]
fn the_note_about_files_left_out_counts_them_and_says_one_file_for_one() {
    let scratch = Scratch::new();
    let context = roomy_pc(&scratch);
    let one = [
        ("vars.fd", b"v".to_vec()),
        ("remote-access.json", b"{}".to_vec()),
        (DISK_FILE, qcow2(&scratch, "d", &[])),
    ];
    let made = imported(&context, &crafted(&scratch, &plain_settings(), &one), None).unwrap();
    assert!(
        made.notes
            .contains(&"1 file of the other PC's that this app doesn't take here was left out.".to_owned()),
        "{:?}",
        made.notes
    );
    assert_eq!(left_out_note(3), "3 files of the other PC's that this app doesn't take here were left out.");
}
