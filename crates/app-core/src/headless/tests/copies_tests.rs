use super::*;
use crate::library::{NewMachine, create};
use crate::remote_access::Access;

fn small_plain(scratch: &Scratch, context: &Context) -> Machine {
    let new = NewMachine {
        name: "Office PC".into(),
        iso: scratch.write("disc.iso", "disc").display().to_string(),
        kind: Kind::Plain,
        windows_disc: false,
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

#[test]
fn show_says_plainly_when_the_guest_refused_its_new_token() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let machine = small_plain(&scratch, &context);
    let folder = machine.folder(&context);
    fs::write(folder.join("guest.token"), "0123456789abcdef0123456789abcdef\n").unwrap();
    fs::write(folder.join("guest.token.next"), "fedcba9876543210fedcba9876543210\n").unwrap();
    assert!(!said(&context, "show office-pc").0.contains("refused"));
    fs::write(folder.join(crate::token_change::REFUSED_FILE), "ERROR: denied.").unwrap();
    let shown = said(&context, "show office-pc").0;
    assert!(
        shown.contains("Its guest refused a new agent token") && shown.contains("ERROR: denied."),
        "{shown}"
    );
}

#[test]
fn duplicate_is_checked_like_the_other_commands() {
    assert!(usage_of("duplicate").contains("duplicate needs a machine."));
    assert!(usage_of("duplicate a --name").contains("--name needs a value."));
    assert!(usage_of("duplicate a --yes").contains("duplicate has no option --yes."));
}

#[test]
fn duplicate_says_what_it_costs_and_hands_the_name_to_the_task() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    context.processes = "/proc".into();
    context.slot = slot_free_on_this_pc(&context);
    small_plain(&scratch, &context);
    let asked = scratch.path("asked");
    scratch.program("program/guesthouse-task", &format!("#!/bin/sh\necho \"$@\" > '{}'\n", asked.display()));
    let (out, result) = said(&context, "duplicate office-pc --name Spare --no-wait");
    assert_eq!(result, Ok(()));
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[0].starts_with("This copies ") || lines[0].starts_with("Its disk of "), "{out}");
    assert_eq!(lines[1], "Copying Office PC to Spare. list shows it once it is made.");
    let wait = || {
        (0..100).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(20));
            asked.exists()
        })
    };
    assert!(wait());
    assert_eq!(fs::read_to_string(&asked).unwrap(), "duplicate office-pc Spare\n");
}

#[test]
fn a_running_machine_is_not_duplicated() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let machine = small_plain(&scratch, &context);
    running(&context, &machine, 60);
    let refusal = said(&context, "duplicate office-pc").1.unwrap_err().to_string();
    assert_eq!(refusal, "Shut the VM down first.");
}

#[test]
fn a_throwaway_copy_is_asked_for_by_name_and_a_machine_made_one_by_set() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    context.processes = "/proc".into();
    context.slot = slot_free_on_this_pc(&context);
    small_plain(&scratch, &context);
    let asked = scratch.path("asked");
    scratch.program("program/guesthouse-task", &format!("#!/bin/sh\necho \"$@\" > '{}'\n", asked.display()));
    let (_, result) = said(&context, "duplicate office-pc --throwaway --no-wait");
    assert_eq!(result, Ok(()));
    let wait = || {
        (0..100).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(20));
            asked.exists()
        })
    };
    assert!(wait());
    assert_eq!(fs::read_to_string(&asked).unwrap(), "duplicate office-pc Office PC Copy throwaway\n");
    let (out, result) = said(&context, "set office-pc throwaway on");
    assert_eq!((out.as_str(), result), ("throwaway is now on.\n", Ok(())));
    assert!(said(&context, "show office-pc").0.contains("throwaway on"));
}

#[test]
fn export_and_import_are_checked_and_an_import_says_what_it_made() {
    assert!(usage_of("export a").contains("export needs a machine and a file."));
    assert!(usage_of("import").contains("import needs a file."));
    let (here, there) = (Scratch::new(), Scratch::new());
    let (from, to) = (scratch_context(&here), scratch_context(&there));
    let machine = small_plain(&here, &from);
    let file = here.path("office.guesthouse");
    let host = crate::vm_host::stand_in::RecordingHost::default();
    let mut task = crate::actions::Task::new(&from, &host, crate::export::EXPORT_TASK, machine);
    crate::export::export(&mut task, &file.to_string_lossy()).unwrap();
    let (out, result) = said(&to, &format!("import {} --name Moved", file.display()));
    assert_eq!(result, Ok(()));
    assert!(out.starts_with("Reading ") && out.ends_with("Moved is made, as moved.\n"), "{out}");
}

#[test]
fn a_copy_deleted_when_it_stops_must_be_a_throwaway_and_is_handed_on() {
    assert!(usage_of("duplicate a --delete-when-stopped").contains("is for a --throwaway copy"));
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    context.processes = "/proc".into();
    context.slot = slot_free_on_this_pc(&context);
    small_plain(&scratch, &context);
    let asked = scratch.path("asked");
    scratch.program("program/guesthouse-task", &format!("#!/bin/sh\necho \"$@\" > '{}'\n", asked.display()));
    let (_, result) = said(&context, "duplicate office-pc --throwaway --delete-when-stopped --no-wait");
    assert_eq!(result, Ok(()));
    let wait = || {
        (0..100).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(20));
            asked.exists()
        })
    };
    assert!(wait());
    assert_eq!(
        fs::read_to_string(&asked).unwrap(),
        "duplicate office-pc Office PC Copy throwaway delete-when-stopped\n"
    );
}
