//! The snapshot rules as they are today, held before they move: which names a snapshot may have and
//! the words of each refusal, which refusal comes first, the tag a snapshot is kept under, the bytes
//! of the file beside it, and which files beside a disk's snapshots are listed and how they read.

use super::*;
use crate::actions::Task;
use crate::machine::Kind;
use crate::testing::{Scratch, scratch_context};
use crate::vm_host::stand_in::RecordingHost;

const NEEDS_A_NAME: &str = "A snapshot needs a name of up to 60 characters.";
const ALREADY: &str = "There is already a snapshot with that name.";
const INSTALLING: &str = "Windows has not finished installing here.";

/// A stopped machine with a qcow2 disk of its own and a firmware file.
fn machine(context: &Context, scratch: &Scratch, nvme: bool) -> Machine {
    let disk = scratch.path("pins.qcow2").display().to_string();
    let made = Command::new("qemu-img").args(["create", "-q", "-f", "qcow2", &disk, "1G"]).status().unwrap();
    assert!(made.success());
    let firmware = scratch.write("pins.fd", "firmware").display().to_string();
    let machine = Machine { kind: Kind::Plain, nvme, ..Machine::new("pins", "Pins", &disk, &firmware) };
    machine.save(context).unwrap();
    machine
}

fn names(context: &Context, machine: &Machine) -> Vec<String> {
    list_snapshots(context, machine).into_iter().map(|snapshot| snapshot.name).collect()
}

fn refused(result: Result<()>) -> String {
    result.unwrap_err().0
}

#[test]
fn the_rules_kept_today() {
    assert_eq!(SNAPSHOT_TAG, "guesthouse-");
    assert_eq!(MAX_SNAPSHOT_NAME, 60);
}

#[test]
fn each_name_is_taken_or_refused_in_these_words() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let machine = machine(&context, &scratch, false);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "take_snapshot", machine.clone());
    let sixty = "x".repeat(60);
    let sixty_wide = "é".repeat(60);
    for name in
        ["a", " padded ", &sixty, &sixty_wide, "a\nb", "\t tabbed \t", "\u{3000}wide space", "A", "a\u{7f}"]
    {
        take_snapshot(&mut task, name).unwrap_or_else(|failure| panic!("{name:?}: {failure}"));
    }
    for name in [
        "",
        "   ",
        "\n",
        "\t",
        "\u{3000}",
        &"x".repeat(61),
        &"é".repeat(61),
        &format!(" {} ", "y".repeat(61)),
    ] {
        assert_eq!(refused(take_snapshot(&mut task, name)), NEEDS_A_NAME, "{name:?}");
    }
    for name in ["a", " a ", "padded", &sixty, "a\nb", "tabbed", "wide space"] {
        assert_eq!(refused(take_snapshot(&mut task, name)), ALREADY, "{name:?}");
    }
    // A name Guesthouse gives a snapshot of its own may repeat one, and one the person gives may not
    // then repeat it.
    make_snapshot(&mut task, "a", true).unwrap();
    make_snapshot(&mut task, "Automatic", true).unwrap();
    make_snapshot(&mut task, "Automatic", true).unwrap();
    assert_eq!(refused(take_snapshot(&mut task, "Automatic")), ALREADY);
    assert_eq!(
        names(&context, &machine),
        [
            "a",
            "padded",
            sixty.as_str(),
            sixty_wide.as_str(),
            "a\nb",
            "tabbed",
            "wide space",
            "A",
            "a\u{7f}",
            "a",
            "Automatic",
            "Automatic"
        ]
    );
}

#[test]
fn the_name_is_judged_before_the_machine_is() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let machine = machine(&context, &scratch, false);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "take_snapshot", machine.clone());
    take_snapshot(&mut task, "kept").unwrap();
    task.machine.installing = true;
    assert_eq!(refused(take_snapshot(&mut task, " ")), NEEDS_A_NAME);
    assert_eq!(refused(take_snapshot(&mut task, "kept")), ALREADY);
    assert_eq!(refused(take_snapshot(&mut task, "new")), INSTALLING);
    assert_eq!(make_snapshot(&mut task, "kept", true).unwrap_err().0, INSTALLING);
    assert_eq!(make_snapshot(&mut task, "", true).unwrap_err().0, NEEDS_A_NAME);
}

#[test]
fn a_snapshot_is_kept_under_a_tag_with_its_file_beside_it_in_these_bytes() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    for (nvme, automatic, name, bytes) in [
        (false, false, "first", r#"{"auto":false,"name":"first","nvme":false}"#),
        (
            false,
            true,
            "Automatic: before \"x\"",
            r#"{"auto":true,"name":"Automatic: before \"x\"","nvme":false}"#,
        ),
        (true, false, " Ünï\tcødé ", r#"{"auto":false,"name":"Ünï\tcødé","nvme":true}"#),
    ] {
        let machine = machine(&context, &scratch, nvme);
        let host = RecordingHost::default();
        let mut task = Task::new(&context, &host, "take_snapshot", machine.clone());
        let tag = make_snapshot(&mut task, name, automatic).unwrap();
        let hex = tag.strip_prefix("guesthouse-").unwrap();
        assert!(hex.len() == 8 && hex.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')), "{tag}");
        assert_eq!(fs::read_to_string(snapshot_file(&context, &machine, &tag, ".json")).unwrap(), bytes);
        let listed = list_snapshots(&context, &machine);
        let [snapshot] = &listed[..] else { panic!("{listed:?}") };
        assert_eq!(
            (snapshot.tag.as_str(), snapshot.name.as_str(), snapshot.automatic),
            (tag.as_str(), name.trim(), automatic)
        );
        delete_tag(&context, &machine, &tag).unwrap();
        assert!(crate::machine::sorted_names(&snapshots_folder(&context, &machine)).is_empty());
        fs::remove_dir_all(machine.folder(&context)).unwrap();
    }
}

#[test]
fn what_lies_beside_a_disks_snapshots_is_read_so() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let machine = machine(&context, &scratch, false);
    fs::create_dir_all(snapshots_folder(&context, &machine)).unwrap();
    // Each tag the disk holds, what is written beside it, and whether firmware is beside it.
    let cases: [(&str, &str, bool); 9] = [
        ("guesthouse-0000000a", r#"{"name": "x"}"#, true),
        ("guesthouse-0000000b", "{}", true),
        ("guesthouse-0000000c", r#"{"name": "y", "auto": true, "nvme": true, "more": 1}"#, true),
        ("guesthouse-0000000d", "not json", true),
        ("guesthouse-0000000e", r#"{"name": "no firmware"}"#, false),
        ("guesthouse-0000000f", r#"{"name": 5, "auto": "yes"}"#, true),
        ("by-hand", r#"{"name": "by hand"}"#, true),
        ("guesthouse-ABCDEF01", r#"{"name": "upper"}"#, true),
        ("guesthouse-00000010", "[1]", true),
    ];
    for (tag, saved, firmware) in cases {
        let made = Command::new("qemu-img").args(["snapshot", "-c", tag, &machine.disk]).status().unwrap();
        assert!(made.success());
        fs::write(snapshot_file(&context, &machine, tag, ".json"), saved).unwrap();
        if firmware {
            fs::write(snapshot_file(&context, &machine, tag, ".fd"), "firmware").unwrap();
        }
    }
    // Files beside the disk with no snapshot in it.
    fs::write(snapshot_file(&context, &machine, "guesthouse-00000011", ".json"), r#"{"name": "gone"}"#)
        .unwrap();
    fs::write(snapshot_file(&context, &machine, "guesthouse-00000011", ".fd"), "firmware").unwrap();
    let mut found: Vec<(String, String, bool)> = list_snapshots(&context, &machine)
        .into_iter()
        .map(|snapshot| (snapshot.tag, snapshot.name, snapshot.automatic))
        .collect();
    found.sort();
    let owned = |tag: &str, name: &str, automatic| (tag.to_owned(), name.to_owned(), automatic);
    assert_eq!(
        found,
        [
            owned("by-hand", "by hand", false),
            owned("guesthouse-0000000a", "x", false),
            owned("guesthouse-0000000b", "", false),
            owned("guesthouse-0000000c", "y", true),
            owned("guesthouse-0000000f", "", false),
            owned("guesthouse-00000010", "", false),
            owned("guesthouse-ABCDEF01", "upper", false),
        ]
    );
}
