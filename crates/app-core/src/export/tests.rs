use super::*;
use crate::carry::Reader;
use crate::library::{NewMachine, create};
use crate::machine::{Drive, Kind};
use crate::network::{Forward, Protocol};
use crate::remote_access::Access;
use crate::testing::{Scratch, scratch_context};
use crate::vm_host::stand_in::RecordingHost;
use std::io::BufReader;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

pub(crate) fn exportable(scratch: &Scratch, context: &Context) -> Machine {
    let new = NewMachine {
        name: "Carry Me".into(),
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
    let mut machine = create(context, &new).unwrap();
    machine.share = scratch.path("shared").display().to_string();
    fs::create_dir_all(&machine.share).unwrap();
    machine.drives = vec![Drive { id: "usb-Stick".into() }];
    machine.forwards = vec![Forward::new(Protocol::Tcp, 2222, 22)];
    machine.save(context).unwrap();
    let folder = machine.folder(context);
    scratch.write(&format!("{}/tpm/tpm2-00.permall", folder.display()), "sealed");
    scratch.write(&format!("{}/guest.token", folder.display()), "0123456789abcdef0123456789abcdef\n");
    scratch.write(&format!("{}/windows-password", folder.display()), "hunter2\n");
    scratch.write(&format!("{}/answer.iso", folder.display()), "left behind");
    scratch.write(
        &format!("{}/remote-view.json", folder.display()),
        r#"{"allow_remote_view":true,"remote_port":7792}"#,
    );
    scratch.write(&format!("{}/throwaway/vars.fd", folder.display()), "a run's");
    let filled = Command::new("qemu-io")
        .args(["-f", "qcow2", "-c", "write -P 7 0 64k", &machine.disk])
        .output()
        .unwrap();
    assert!(filled.status.success());
    let snapshot = Command::new("qemu-img")
        .args(["snapshot", "-c", "guesthouse-0a1b2c3d", &machine.disk])
        .status()
        .unwrap();
    assert!(snapshot.success());
    scratch.write(&format!("{}/snapshots/guesthouse-0a1b2c3d.fd", folder.display()), "firmware then");
    scratch
        .write(&format!("{}/snapshots/guesthouse-0a1b2c3d.json", folder.display()), "{\"name\": \"Before\"}");
    machine
}

pub(crate) fn exported(context: &Context, machine: &Machine, to: &Path) -> Result<()> {
    let host = RecordingHost::default();
    let mut task = Task::new(context, &host, EXPORT_TASK, machine.clone());
    export(&mut task, &to.to_string_lossy())
}

fn names_in(file: &Path) -> Vec<String> {
    let mut reader = Reader::new(BufReader::new(File::open(file).unwrap())).unwrap();
    let mut names = Vec::new();
    while let Some(member) = reader.next_file().unwrap() {
        reader.copy_to(&member, &mut std::io::sink()).unwrap();
        names.push(member.path.display().to_string());
    }
    names
}

#[test]
fn an_export_carries_what_the_guest_needs_and_none_of_this_pcs_paths() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let machine = exportable(&scratch, &context);
    let to = scratch.path("out/carry-me.guesthouse");
    fs::create_dir_all(to.parent().unwrap()).unwrap();
    exported(&context, &machine, &to).unwrap();
    assert_eq!(fs::metadata(&to).unwrap().permissions().mode() & 0o777, 0o600, "it holds the token");
    assert!(!with_suffix(&to, PART_SUFFIX).exists());
    let names = names_in(&to);
    assert_eq!(names.first().map(String::as_str), Some(MACHINE_FILE));
    assert_eq!(names.last().map(String::as_str), Some(DISK_FILE));
    for wanted in [
        "vars.fd",
        "tpm/tpm2-00.permall",
        "guest.token",
        "windows-password",
        "snapshots/guesthouse-0a1b2c3d.fd",
    ] {
        assert!(names.iter().any(|name| name == wanted), "{wanted} in {names:?}");
    }
    assert!(!names.iter().any(|name| name == "answer.iso" || name.starts_with("throwaway/")), "{names:?}");
    assert!(!names.iter().any(|name| name == "remote-view.json"), "its port is this PC's: {names:?}");
    let whole = fs::read(&to).unwrap();
    let text = String::from_utf8_lossy(&whole);
    let home = scratch.root().display().to_string();
    let settings_end = text.find("vars.fd").unwrap();
    assert!(!text[..settings_end].contains(&home), "no path of this PC's: {}", &text[..settings_end]);
}

#[test]
fn a_throwaway_goes_with_its_clean_notes_and_not_its_last_runs() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let mut machine = exportable(&scratch, &context);
    let folder = machine.folder(&context);
    fs::write(folder.join("gpu-identity.json"), "as installed").unwrap();
    fs::write(folder.join("scan-exclusions.json"), r#"{"folders": ["D:\Games"]}"#).unwrap();
    crate::throwaway::set_throwaway(&context, &mut machine, true).unwrap();
    fs::write(folder.join("gpu-identity.json"), "a run's").unwrap();
    fs::write(folder.join("nvme-refused"), "").unwrap();
    let to = scratch.path("throwaway.guesthouse");
    exported(&context, &machine, &to).unwrap();
    let mut reader = Reader::new(BufReader::new(File::open(&to).unwrap())).unwrap();
    let mut notes = Vec::new();
    while let Some(member) = reader.next_file().unwrap() {
        let mut data = Vec::new();
        reader.copy_to(&member, &mut data).unwrap();
        if member.path.parent() == Some(Path::new(""))
            && crate::throwaway::is_guest_note(&member.path.to_string_lossy())
        {
            notes.push((member.path.display().to_string(), String::from_utf8_lossy(&data).into_owned()));
        }
    }
    assert_eq!(
        notes,
        [("gpu-identity.json".to_owned(), "as installed".to_owned())],
        "which folders Defender leaves out is asked of the guest where it lands"
    );
}

#[test]
fn an_export_is_refused_for_a_running_machine_a_layered_disk_or_a_taken_name() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let machine = exportable(&scratch, &context);
    let taken = scratch.write("taken.guesthouse", "mine");
    assert!(plan_export(&context, &machine, &taken).unwrap_err().0.contains("already there"));
    assert!(
        plan_export(&context, &machine, Path::new("relative.guesthouse"))
            .unwrap_err()
            .0
            .contains("full path")
    );
    let base = scratch.path("base.qcow2");
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
        .args(["rebase", "-u", "-f", "qcow2", "-F", "qcow2", "-b"])
        .arg(&base)
        .arg(&machine.disk)
        .status()
        .unwrap();
    assert!(rebased.success());
    let to = scratch.path("layered.guesthouse");
    assert!(plan_export(&context, &machine, &to).unwrap_err().0.contains("built on"));
    let installing = Machine { installing: true, ..machine };
    assert!(plan_export(&context, &installing, &to).unwrap_err().0.contains("installing"));
}

#[test]
fn what_a_killed_export_left_is_taken_away_and_anything_else_stays() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let machine = exportable(&scratch, &context);
    let to = scratch.path("again.guesthouse");
    let part = with_suffix(&to, PART_SUFFIX);
    let mut left = crate::carry::MAGIC.to_vec();
    left.extend_from_slice(b"half of a disk");
    fs::write(&part, &left).unwrap();
    exported(&context, &machine, &to).unwrap();
    assert!(to.exists() && !part.exists());
    let other = scratch.path("other.guesthouse");
    let theirs = scratch.write("other.guesthouse.part", "the user's own file");
    assert!(exported(&context, &machine, &other).unwrap_err().0.contains("in the way"));
    assert_eq!(fs::read_to_string(theirs).unwrap(), "the user's own file");
    let busy = scratch.path("busy.guesthouse");
    let busy_part = scratch.write("busy.guesthouse.part", &left);
    let held = File::open(&busy_part).unwrap();
    rustix::fs::flock(&held, rustix::fs::FlockOperation::NonBlockingLockExclusive).unwrap();
    assert!(exported(&context, &machine, &busy).unwrap_err().0.contains("in the way"), "one at work stays");
}
