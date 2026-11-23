use super::*;
use crate::actions::Task;
use crate::gpu_identity::{IdentityState, Shown, state_file, write_state};
use crate::snapshots::{go_back, snapshots_folder, take_snapshot};
use crate::testing::{Scratch, scratch_context};
use crate::vm_host::stand_in::RecordingHost;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

fn qcow2(path: &str) {
    assert!(
        Command::new("qemu-img")
            .args(["create", "-q", "-f", "qcow2", path, "1G"])
            .status()
            .unwrap()
            .success()
    );
}

/// A Windows machine with a real disk and firmware file, which has made no snapshot, and whose user
/// turned the snapshots on.
fn windows(context: &Context, scratch: &Scratch) -> Machine {
    let disk = scratch.path("win.qcow2").display().to_string();
    qcow2(&disk);
    let vars = scratch.write("win.fd", "firmware").display().to_string();
    let machine = Machine { auto_snapshot: Some(true), ..Machine::new("win", "Win", &disk, &vars) };
    machine.save(context).unwrap();
    machine
}

fn names(context: &Context, machine: &Machine) -> Vec<(String, bool)> {
    list_snapshots(context, machine).into_iter().map(|found| (found.name, found.automatic)).collect()
}

/// One start's worth of this: its snapshot, and its end if it succeeds.
fn start(context: &Context, machine: &Machine, succeeds: bool) -> Task<'static> {
    let host: &'static RecordingHost = Box::leak(Box::default());
    let context: &'static Context = Box::leak(Box::new(context.clone()));
    let mut task = Task::new(context, host, "start", machine.clone());
    before_start(&mut task);
    if succeeds {
        start_succeeded(&task);
    }
    task
}

fn restart_due(context: &Context, machine: &Machine) {
    let state = IdentityState { shown: Shown::Maker, restart_due: true, ..IdentityState::default() };
    write_state(context, machine, &state).unwrap();
}

#[test]
fn a_start_with_nothing_due_takes_no_snapshot_and_writes_nothing() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let machine = windows(&context, &scratch);
    let first = start(&context, &machine, true);
    assert_eq!(names(&context, &machine).len(), 1, "the first start meets this build for the first time");
    assert!(!first.auto.covers.is_empty());
    let record = record::read(&context, &machine);
    assert!(!record.pending && record.covered.len() == 1);
    let second = start(&context, &machine, true);
    assert_eq!(record::read(&context, &machine), record, "the record is as it was");
    assert_eq!(names(&context, &machine).len(), 1);
    assert!(second.auto.covers.is_empty() && second.auto.note.is_empty());
}

#[test]
fn a_due_change_gets_one_snapshot_named_for_it_that_replaces_the_last_and_leaves_the_users_own() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let machine = windows(&context, &scratch);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "take_snapshot", machine.clone());
    take_snapshot(&mut task, "Mine").unwrap();
    take_snapshot(&mut task, "Automatic: before the vendor ID, mine too").unwrap();
    start(&context, &machine, true);
    restart_due(&context, &machine);
    start(&context, &machine, true);
    let found = names(&context, &machine);
    let [first, second, auto] = &found[..] else { panic!("{found:?}") };
    assert_eq!(
        (first, second),
        (&("Mine".to_owned(), false), &("Automatic: before the vendor ID, mine too".to_owned(), false))
    );
    assert!(auto.1 && auto.0 == "Automatic: before the vendor ID", "{auto:?}");
    assert!(auto.0.chars().count() <= crate::snapshots::MAX_SNAPSHOT_NAME);
    let record = record::read(&context, &machine);
    assert_eq!(record.name, auto.0);
    assert!(Command::new("qemu-img").args(["check", "-q", &machine.disk]).status().unwrap().success());
}

#[test]
fn several_changes_are_named_as_changes() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let machine = windows(&context, &scratch);
    restart_due(&context, &machine);
    start(&context, &machine, false);
    let [(name, true)] = &names(&context, &machine)[..] else { panic!() };
    assert!(name == "Automatic: before changes", "{name}");
}

#[test]
fn a_failed_start_keeps_the_snapshot_that_came_before_it_and_offers_it() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let machine = windows(&context, &scratch);
    assert_eq!(restore_point(&context, &machine), None);
    start(&context, &machine, false);
    let offered = restore_point(&context, &machine).unwrap();
    assert_eq!(names(&context, &machine), [(offered.name.clone(), true)]);
    // Windows may already hold part of a change, so another try must not snapshot that disk.
    restart_due(&context, &machine);
    fs::write(&machine.vars, "after the failure").unwrap();
    start(&context, &machine, false);
    assert_eq!(restore_point(&context, &machine), Some(offered.clone()));
    assert_eq!(names(&context, &machine).len(), 1);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "go_back", machine.clone());
    assert_eq!(fs::read_to_string(&machine.vars).unwrap(), "after the failure");
    go_back(&mut task, &offered.tag).unwrap();
    assert_eq!(fs::read_to_string(&machine.vars).unwrap(), "firmware");
    assert_eq!(restore_point(&context, &machine), None, "going back ends the offer");
    assert!(record::read(&context, &machine).covered.is_empty(), "and what it had met is due again");
    start(&context, &machine, true);
    assert_eq!(restore_point(&context, &machine), None, "a start that succeeds ends it too");
    assert!(!record::read(&context, &machine).pending);
}

#[test]
fn going_back_puts_the_notes_about_the_guest_back_with_the_disk() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let mut machine = windows(&context, &scratch);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "go_back", machine.clone());
    take_snapshot(&mut task, "Before").unwrap();
    let tag = list_snapshots(&context, &machine)[0].tag.clone();
    // Windows then took a package for its maker's number, and the disk moved to NVMe.
    restart_due(&context, &machine);
    machine.nvme = true;
    machine.save_disk_place(&context).unwrap();
    task.machine = machine.clone();
    fs::write(machine.folder(&context).join("nvme-trial"), "").unwrap();
    go_back(&mut task, &tag).unwrap();
    assert!(!state_file(&context, &machine).exists(), "no package, so Microsoft's number");
    assert!(!machine.folder(&context).join("nvme-trial").exists());
    assert!(!crate::machine::load(&context, "win").unwrap().nvme, "and the disk is back on SATA");
    assert!(!task.machine.nvme);
}

#[test]
fn what_was_noted_is_kept_by_a_snapshot_and_a_snapshot_from_before_leaves_notes_alone() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let machine = windows(&context, &scratch);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "go_back", machine.clone());
    restart_due(&context, &machine);
    take_snapshot(&mut task, "Has a package").unwrap();
    let tag = list_snapshots(&context, &machine)[0].tag.clone();
    fs::remove_file(state_file(&context, &machine)).unwrap();
    go_back(&mut task, &tag).unwrap();
    assert!(read_identity_shown(&context, &machine));
    // A snapshot made by an older build has no notes, so going back to it changes none.
    fs::remove_dir_all(snapshots_folder(&context, &machine).join(format!("{tag}.state"))).unwrap();
    fs::remove_file(state_file(&context, &machine)).unwrap();
    go_back(&mut task, &tag).unwrap();
    assert!(!state_file(&context, &machine).exists());
}

#[test]
fn going_back_to_before_a_recipes_steps_says_they_have_not_run() {
    use crate::recipe::{Outcome, read_outcome, write_outcome};
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let machine = windows(&context, &scratch);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "go_back", machine.clone());
    write_outcome(&context, &machine, &Outcome::Pending).unwrap();
    take_snapshot(&mut task, "Before the steps").unwrap();
    let tag = list_snapshots(&context, &machine)[0].tag.clone();
    write_outcome(&context, &machine, &Outcome::Done).unwrap();
    go_back(&mut task, &tag).unwrap();
    assert_eq!(read_outcome(&context, &machine), Outcome::Pending, "the disk has nothing they installed");
}

fn read_identity_shown(context: &Context, machine: &Machine) -> bool {
    crate::gpu_identity::read_state(context, machine).shown == Shown::Maker
}

#[test]
fn the_switch_and_the_kind_of_machine_decide_whether_a_snapshot_is_taken() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let machine = windows(&context, &scratch);
    let off = Machine { auto_snapshot: Some(false), ..machine.clone() };
    start(&context, &off, false);
    let plain = Machine { kind: Kind::Plain, ..machine.clone() };
    start(&context, &plain, false);
    let linux = Machine { kind: Kind::Linux, ..machine.clone() };
    start(&context, &linux, false);
    let installing = Machine { installing: true, ..machine.clone() };
    start(&context, &installing, false);
    assert!(names(&context, &machine).is_empty() && restore_point(&context, &machine).is_none());
    assert!(!snapshots_folder(&context, &machine).exists());
    let on = Machine { auto_snapshot: Some(true), ..machine };
    start(&context, &on, true);
    assert_eq!(names(&context, &on).len(), 1);
}

#[test]
fn a_throwaway_takes_none_and_offers_no_way_back_as_each_start_is_clean() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let machine = Machine { throwaway: true, ..windows(&context, &scratch) };
    machine.save(&context).unwrap();
    restart_due(&context, &machine);
    let task = start(&context, &machine, false);
    assert!(names(&context, &machine).is_empty(), "the clean disk gains no snapshot");
    assert!(task.auto.covers.is_empty() && task.auto.note.is_empty());
    let left = record::Record {
        tag: "guesthouse-0a1b2c3d".into(),
        name: "Before".into(),
        pending: true,
        covered: vec![],
    };
    record::write(&context, &machine, &left).unwrap();
    assert_eq!(restore_point(&context, &machine), None, "a failed run is undone by the next start");
}

#[test]
fn the_default_is_the_constant_and_a_choice_is_kept_only_when_made() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let machine = Machine::new("a", "A", "", "");
    assert_eq!(machine.auto_snapshot_on(), AUTO_SNAPSHOT_DEFAULT_ON);
    machine.save(&context).unwrap_or_default();
    let text = serde_json::to_string(&machine).unwrap();
    assert!(!text.contains("auto_snapshot"), "an older build can still read it");
    let off = Machine { auto_snapshot: Some(false), ..machine };
    assert!(
        !off.auto_snapshot_on() && serde_json::to_string(&off).unwrap().contains("\"auto_snapshot\":false")
    );
}

#[test]
fn a_disk_that_cannot_hold_a_snapshot_starts_with_no_word_and_no_new_try() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let disk = scratch.path("raw.img").display().to_string();
    assert!(
        Command::new("qemu-img").args(["create", "-q", "-f", "raw", &disk, "1M"]).status().unwrap().success()
    );
    let vars = scratch.write("raw.fd", "firmware").display().to_string();
    let machine = Machine { auto_snapshot: Some(true), ..Machine::new("raw", "Raw", &disk, &vars) };
    machine.save(&context).unwrap();
    let task = start(&context, &machine, true);
    assert!(task.auto.note.is_empty(), "the settings say so instead of every start");
    assert!(restore_point(&context, &machine).is_none() && !record::read(&context, &machine).pending);
    assert!(!record::read(&context, &machine).covered.is_empty(), "it is not tried at every start");
}

#[test]
fn a_snapshot_that_fails_is_said_and_the_start_goes_on_and_asks_again_next_time() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let machine = windows(&context, &scratch);
    let mut holder = Command::new("qemu-io")
        .args(["-f", "qcow2", &machine.disk])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = holder.stdin.take().unwrap();
    writeln!(input, "read 0 512").unwrap();
    let mut line = String::new();
    BufReader::new(holder.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let task = start(&context, &machine, true);
    assert!(
        task.auto.note.starts_with("No snapshot was taken before this start: qemu-img refused"),
        "{}",
        task.auto.note
    );
    assert!(task.auto.covers.is_empty());
    assert!(restore_point(&context, &machine).is_none());
    assert!(record::read(&context, &machine).covered.is_empty());
    assert!(
        !snapshots_folder(&context, &machine).read_dir().is_ok_and(|mut left| left.next().is_some()),
        "nothing left behind"
    );
    drop(input);
    holder.wait().unwrap();
    start(&context, &machine, true);
    assert_eq!(names(&context, &machine).len(), 1, "the next start has the disk free and takes it");
}

#[test]
fn removing_an_entry_deletes_the_automatic_snapshot_and_keeps_the_users_in_the_way() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let machine = windows(&context, &scratch);
    start(&context, &machine, true);
    assert_eq!(names(&context, &machine).len(), 1);
    crate::library::remove_entry(&context, &machine).unwrap();
    assert!(
        Command::new("qemu-img").args(["snapshot", "-l", &machine.disk]).output().unwrap().stdout.is_empty()
    );
    let again = windows(&context, &scratch);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "take_snapshot", again.clone());
    take_snapshot(&mut task, "Mine").unwrap();
    start(&context, &again, true);
    assert!(crate::library::remove_entry(&context, &again).unwrap_err().0.contains("snapshots first"));
    assert_eq!(names(&context, &again), [("Mine".to_owned(), false)], "only its own went");
}

#[test]
fn what_is_due_is_read_from_the_machines_files() {
    use crate::display::stand_in::fake_package;
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    let machine = windows(&context, &scratch);
    let signatures = |context: &Context| -> Vec<String> {
        risks::pending(context, &machine).into_iter().map(|due| due.signature).collect()
    };
    assert_eq!(signatures(&context).len(), 1, "only the build's own files");
    assert!(signatures(&context)[0].starts_with("files:"));
    fake_package(&scratch, "build-7");
    assert!(signatures(&context).contains(&"display:build-7".to_owned()));
    crate::display::write_state(
        &context,
        &machine,
        &crate::display::DisplayState { package: "build-7".into(), ..Default::default() },
    )
    .unwrap();
    assert!(!signatures(&context).iter().any(|due| due.starts_with("display:")), "installed already");
    scratch.write("program/guest/nvme-ready.ps1", "ready");
    assert!(signatures(&context).contains(&"nvme:move".to_owned()));
    fs::write(machine.folder(&context).join("nvme-refused"), "no").unwrap();
    assert!(
        !signatures(&context).iter().any(|due| due.starts_with("nvme:")),
        "a machine that went back stays"
    );
    // A package to stage needs a script and a QEMU that can show the number.
    scratch.write("program/guest/gpu-identity.ps1", "stage");
    assert!(!signatures(&context).iter().any(|due| due.starts_with("vendor-id:")), "no QEMU that can");
    let qemu = scratch.program("qemu", "#!/bin/sh\necho 'vendor-id=<uint16>'\n");
    context.paths.qemu = qemu;
    assert!(signatures(&context).iter().any(|due| due.starts_with("vendor-id:stage:")));
    restart_due(&context, &machine);
    assert!(signatures(&context).contains(&"vendor-id:restart".to_owned()));
    let done = IdentityState { shown: Shown::Maker, ..IdentityState::default() };
    write_state(&context, &machine, &done).unwrap();
    assert!(!signatures(&context).iter().any(|due| due.starts_with("vendor-id:")), "already on its number");
}

#[test]
fn a_start_that_works_ends_the_offer_though_what_failed_it_is_gone_and_whatever_the_switch_says() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let machine = windows(&context, &scratch);
    start(&context, &machine, true);
    restart_due(&context, &machine);
    start(&context, &machine, false);
    assert!(restore_point(&context, &machine).is_some());
    // The failed start fell back to Microsoft's number, so nothing is due now.
    write_state(&context, &machine, &IdentityState::default()).unwrap();
    let after = start(&context, &machine, true);
    assert!(after.auto.covers.is_empty(), "nothing was due");
    assert_eq!(restore_point(&context, &machine), None);
    assert!(!record::read(&context, &machine).pending);
    // And the same with the switch turned off after a failure.
    restart_due(&context, &machine);
    start(&context, &machine, false);
    assert!(restore_point(&context, &machine).is_some());
    let off = Machine { auto_snapshot: Some(false), ..machine.clone() };
    start(&context, &off, true);
    assert_eq!(restore_point(&context, &machine), None);
}

#[test]
fn a_new_host_driver_of_any_maker_is_a_new_change() {
    use crate::gpu_maker::GpuMaker;
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let mut machine = windows(&context, &scratch);
    let files = |machine: &Machine| -> String {
        risks::pending(&context, machine)
            .into_iter()
            .map(|due| due.signature)
            .find(|s| s.starts_with("files:"))
            .unwrap()
    };
    for (maker, pci, file) in
        [(GpuMaker::Amd, "0000:7c:00.0", "amd.json"), (GpuMaker::Intel, "0000:03:00.0", "intel.json")]
    {
        machine.use_windows_card(maker, pci);
        let before = files(&machine);
        let setup = context.paths.setup_folder().join(file);
        fs::create_dir_all(setup.parent().unwrap()).unwrap();
        fs::write(&setup, r#"{"driver_folder": "/d", "driver_version": "32.0.1"}"#).unwrap();
        let after = files(&machine);
        assert!(after.ends_with(":32.0.1") && after != before, "{maker:?}: {after}");
    }
}
