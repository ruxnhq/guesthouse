use super::*;
use crate::actions::Task;
use crate::auto_snapshot::{AUTO_SNAPSHOT_DEFAULT_ON, before_start};
use crate::machine::load;
use crate::snapshots::take_snapshot;
use crate::vm_host::stand_in::RecordingHost;
use std::process::Command;

fn refused(context: &Context, words: &str) -> String {
    match said(context, words).1 {
        Err(Problem::Failed(failure)) => failure.0,
        other => panic!("{words:?} gave {other:?}"),
    }
}

/// A Windows machine with a real disk, so the snapshot code has something to work on.
fn with_disk(scratch: &Scratch, context: &Context) -> Machine {
    let disk = scratch.path("win.qcow2").display().to_string();
    assert!(
        Command::new("qemu-img")
            .args(["create", "-q", "-f", "qcow2", &disk, "1G"])
            .status()
            .unwrap()
            .success()
    );
    let vars = scratch.write("win.fd", "firmware").display().to_string();
    let machine = Machine { auto_snapshot: Some(true), ..Machine::new("win", "Win", &disk, &vars) };
    fs::create_dir_all(machine.folder(context)).unwrap();
    machine.save(context).unwrap();
    machine
}

#[test]
fn the_setting_is_shown_by_its_default_and_kept_when_changed() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    make(&context, "win", "Win", Kind::Windows);
    let word = if AUTO_SNAPSHOT_DEFAULT_ON { "on" } else { "off" };
    assert!(said(&context, "show win").0.contains(&format!("auto-snapshot {word}\n")));
    assert_eq!(said(&context, "set win auto-snapshot off").0, "auto-snapshot is now off.\n");
    assert_eq!(load(&context, "win").unwrap().auto_snapshot, Some(false));
    assert!(said(&context, "show win").0.contains("auto-snapshot off\n"));
    assert_eq!(said(&context, "set win auto-snapshot on").1, Ok(()));
    assert!(refused(&context, "set win auto-snapshot maybe").contains("is on or off"));
    make(&context, "plain", "Plain", Kind::Plain);
    assert!(refused(&context, "set plain auto-snapshot on").contains("only for a Windows machine"));
    assert!(!said(&context, "show plain").0.contains("auto-snapshot"));
}

#[test]
fn the_snapshots_are_listed_with_the_automatic_one_told_apart() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let machine = with_disk(&scratch, &context);
    assert_eq!(said(&context, "snapshots win").0, "Win has no snapshots.\n");
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", machine.clone());
    take_snapshot(&mut task, "Mine").unwrap();
    before_start(&mut task);
    let listing = said(&context, "snapshots win").0;
    let lines: Vec<&str> = listing.lines().collect();
    assert_eq!(lines.len(), 2, "{listing}");
    assert!(lines[0].contains(": Mine [guesthouse-"), "{listing}");
    assert!(
        lines[1].contains("Automatic: before ") && lines[1].ends_with("(taken by Guesthouse before a start)")
    );
}

#[test]
fn going_back_needs_a_start_that_failed_after_a_snapshot_and_a_yes() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let machine = with_disk(&scratch, &context);
    assert!(refused(&context, "go-back win --yes").contains("no start that failed after a snapshot"));
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "start", machine.clone());
    before_start(&mut task);
    let said_without_yes = refused(&context, "go-back win");
    assert!(
        said_without_yes.contains("is lost")
            && said_without_yes.ends_with("Run it again with --yes to go on.")
    );
    let status_text = said(&context, "status win").0;
    assert!(status_text.contains("guesthouse-vm go-back win goes back to it."), "{status_text}");
    assert!(usage_of("go-back").contains("go-back needs a machine."));
    assert!(usage_of("snapshots a b").contains("snapshots takes only a machine."));
}

/// A task program that only writes down what it was asked, as the real one would be.
fn recording_tasks(scratch: &Scratch, context: &mut Context) -> std::path::PathBuf {
    context.processes = "/proc".into();
    context.slot = slot_free_on_this_pc(context);
    context.make_run_folder(None).unwrap();
    let record = context.run_folder(None).join("guesthouse-task.json");
    let asked = scratch.path("asked");
    let script = format!(
        "#!/bin/sh\necho \"$@\" >> '{}'\necho '{{\"action\": \"'$1'\", \"machine\": \"win\"}}' > '{}'\n",
        asked.display(),
        record.display()
    );
    scratch.program("program/guesthouse-task", &script);
    asked
}

#[test]
fn snapshots_are_taken_gone_back_to_and_deleted_by_name_or_tag_with_a_yes() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    let machine = with_disk(&scratch, &context);
    let asked = recording_tasks(&scratch, &mut context);
    let (out, result) = said(&context, "take-snapshot win Before");
    assert_eq!(result, Ok(()), "{out}");
    assert!(out.ends_with("Win has a snapshot called Before.\n"), "{out}");
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "take_snapshot", machine.clone());
    take_snapshot(&mut task, "Mine").unwrap();
    let tag = crate::snapshots::list_snapshots(&context, &machine)[0].tag.clone();
    assert!(said(&context, "snapshots win").0.contains(&format!(": Mine [{tag}]")));
    assert!(refused(&context, "go-back win Mine").ends_with("Run it again with --yes to go on."));
    assert!(refused(&context, "delete-snapshot win Mine").ends_with("Run it again with --yes to go on."));
    assert!(refused(&context, "go-back win Nope --yes").contains("has no snapshot called Nope"));
    assert_eq!(said(&context, "go-back win Mine --yes").1, Ok(()));
    assert_eq!(said(&context, &format!("delete-snapshot win {tag} --yes")).1, Ok(()));
    let ran = fs::read_to_string(asked).unwrap();
    assert_eq!(ran, format!("take_snapshot win Before\ngo_back win {tag}\ndelete_snapshot win {tag}\n"));
}

#[test]
fn snapshots_change_only_where_the_apps_page_lets_them() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let machine = with_disk(&scratch, &context);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "take_snapshot", machine.clone());
    take_snapshot(&mut task, "Mine").unwrap();
    take_snapshot(&mut task, "Automatic: before changes").unwrap();
    let mut installing = load(&context, "win").unwrap();
    installing.installing = true;
    installing.save(&context).unwrap();
    for command in ["take-snapshot win X", "go-back win Mine --yes", "delete-snapshot win Mine --yes"] {
        assert_eq!(refused(&context, command), "Its install has not finished.", "{command}");
    }
    assert!(usage_of("take-snapshot win").contains("take-snapshot needs a machine and a name."));
    assert!(usage_of("delete-snapshot win").contains("delete-snapshot needs a machine and a snapshot."));
    assert!(usage_of("go-back a b c").contains("go-back takes only a machine."));
}
