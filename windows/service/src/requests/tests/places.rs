//! Where a machine's disks are kept: chosen as it's made, found again, refused when the user may not
//! keep it there, and moved.

use super::*;
use guesthouse_remote_view_core::revoked;

/// A folder of the test's own, in which machines are kept, such as a drive of their own.
fn drive(setup: &Setup) -> PathBuf {
    let drive = setup.root.join("drive");
    fs::create_dir_all(&drive).unwrap();
    drive
}

fn placed(setup: &Setup, name: &str, folder: &Path) -> NewMachine {
    NewMachine { folder: Some(folder.to_owned()), ..new_machine(setup, name) }
}

fn ask_as(service: &Arc<Service<Pretend>>, request: Request, readable: bool) -> Vec<Answer> {
    let (answers, received) = channel();
    service.answer(request, &mut Window { answers, readable, gone: false });
    received.try_iter().collect()
}

/// A machine's events so far.
fn history(service: &Arc<Service<Pretend>>, name: &str) -> Vec<Event> {
    service.machines().watch(name).unwrap().0
}

/// Waits for a machine's events to say `text`, which is what a background job ends with.
fn wait_for_event(service: &Arc<Service<Pretend>>, name: &str, text: &str) {
    for _ in 0..400 {
        if history(service, name).iter().any(|event| match event {
            Event::Note(said) | Event::Problem(said) => said.contains(text),
            _ => false,
        }) {
            wait_idle(service, name);
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("{name} never said {text}: {:?}", history(service, name));
}

fn refused_with(answers: &[Answer]) -> &str {
    match answers {
        [Answer::Refused { reason }] => reason,
        other => panic!("{other:?}"),
    }
}

/// The names in a folder, sorted.
fn names(folder: &Path) -> Vec<String> {
    let mut found: Vec<String> = fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    found.sort();
    found
}

#[test]
fn a_machine_made_in_a_chosen_folder_has_its_disks_there_and_its_settings_at_home() {
    let setup = setup("placed", Pretend::default());
    let drive = drive(&setup);
    let answers = ask(&setup.service, Request::Create { machine: placed(&setup, "win", &drive) });
    assert_eq!(answers.last(), Some(&Answer::Done { reason: None }));

    let there = drive.join("win");
    assert!(
        there.join("win.vhdx").is_file(),
        "the disk is in the chosen folder, in one of the machine's name"
    );
    let home = setup.root.join("machines").join("win");
    assert!(!home.join("win.vhdx").exists());
    assert_eq!(names(&home), ["events.log", "location.json", "machine.json"]);
    let shown = &status(&setup.service)[0];
    assert_eq!((shown.folder.as_path(), shown.problem.as_deref()), (there.as_path(), None));

    // The service finds it again as it starts, by what's in the machine's own folder.
    let again = Service::new(Store::new(setup.root.join("machines")), Pretend::default());
    assert_eq!(status(&again)[0].folder, there);
    assert_eq!(again.machines().store.disk("win"), there.join("win.vhdx"));
    assert_eq!(again.machines().store.account("win").unwrap().1, "secret");

    // Deleting it deletes the disks where they are, and the machine's own folder.
    assert_eq!(ask(&again, Request::Delete { name: "win".into() }), [Answer::Ok]);
    assert!(!there.exists() && !home.exists());
    assert!(drive.exists(), "the folder it was made in is the user's, and stays");
}

#[test]
fn no_folder_or_an_empty_one_keeps_a_machine_where_machines_are_kept() {
    let setup = setup("default-place", Pretend::default());
    ask(&setup.service, Request::Create { machine: new_machine(&setup, "one") });
    let empty = NewMachine { folder: Some(PathBuf::new()), ..new_machine(&setup, "two") };
    ask(&setup.service, Request::Create { machine: empty });
    for name in ["one", "two"] {
        let home = setup.root.join("machines").join(name);
        assert!(home.join(format!("{name}.vhdx")).is_file());
        assert!(!home.join("location.json").exists());
    }
    assert_eq!(status(&setup.service)[1].folder, setup.root.join("machines").join("two"));
}

#[test]
fn a_folder_the_user_may_not_keep_a_machine_in_makes_no_machine() {
    let setup = setup("place-rules", Pretend::default());
    let drive = drive(&setup);
    fs::create_dir_all(drive.join("link")).unwrap();
    fs::create_dir_all(drive.join("taken").join("win")).unwrap();
    fs::write(drive.join("a file"), b"x").unwrap();
    let refused = |folder: PathBuf, readable: bool| {
        let answers =
            ask_as(&setup.service, Request::Create { machine: placed(&setup, "win", &folder) }, readable);
        refused_with(&answers).to_owned()
    };
    assert!(refused(PathBuf::from("drive"), true).contains("own drives"), "not a relative one");
    assert!(refused(PathBuf::from(r"\\server\share"), true).contains("own drives"), "not a remote one");
    assert!(refused(setup.root.join("machines"), true).contains("service's own folder"));
    assert!(refused(setup.root.join("machines").join("sub"), true).contains("service's own folder"));
    assert!(refused(drive.clone(), false).contains("may not read"), "one the user can't open");
    assert!(refused(drive.join("link"), true).contains("link"), "not a link");
    assert!(refused(drive.join("missing"), true).contains("isn't a folder"));
    assert!(refused(drive.join("a file"), true).contains("isn't a folder"));
    assert!(refused(drive.join("taken"), true).contains("there already"), "not over a folder that's there");
    assert!(status(&setup.service).is_empty());
    assert!(names(&setup.root.join("machines")).is_empty(), "nothing was left behind");
    assert_eq!(names(&drive), ["a file", "link", "taken"]);
}

#[test]
fn a_machine_whose_folder_was_made_by_someone_else_is_listed_with_why_and_is_not_touched() {
    let setup = setup("untrusted", Pretend::default());
    let drive = drive(&setup);
    ask(&setup.service, Request::Create { machine: placed(&setup, "win", &drive) });
    let distrust = Store::new(setup.root.join("machines"))
        .with_hold(Arc::new(|_| Err("It was made by a user, not by an administrator.".into())));
    let service = Service::new(distrust, Pretend::default());
    let shown = &status(&service)[0];
    assert!(shown.problem.as_deref().unwrap().contains("made by a user"), "{:?}", shown.problem);
    for request in [
        Request::Start { name: "win".into() },
        Request::Grow { name: "win".into(), size_gb: 512 },
        Request::Snapshot { name: "win".into(), label: "x".into() },
        Request::Move { name: "win".into(), folder: PathBuf::new() },
    ] {
        assert!(refused_with(&ask(&service, request)).contains("made by a user"));
    }
    // Forgetting it leaves the folder, which isn't trusted to be deleted.
    assert_eq!(ask(&service, Request::Delete { name: "win".into() }), [Answer::Ok]);
    assert!(status(&service).is_empty());
    assert!(drive.join("win").join("win.vhdx").exists());
}

#[test]
fn a_machine_whose_drive_is_gone_is_listed_with_why_and_is_back_with_the_drive() {
    let setup = setup("gone", Pretend::default());
    let drive = drive(&setup);
    ask(&setup.service, Request::Create { machine: placed(&setup, "win", &drive) });
    let away = setup.root.join("drive-away");
    fs::rename(&drive, &away).unwrap();

    let service = Service::new(Store::new(setup.root.join("machines")), Pretend::default());
    let shown = &status(&service)[0];
    assert_eq!(shown.name, "win");
    assert_eq!(shown.memory_mb, 8192, "its settings are its own folder's, so they're there");
    assert!(shown.problem.as_deref().unwrap().contains("isn't there"), "{:?}", shown.problem);
    assert!(refused_with(&ask(&service, Request::Start { name: "win".into() })).contains("isn't there"));
    assert!(service.work.booted.lock().unwrap().is_empty());

    fs::rename(&away, &drive).unwrap();
    assert_eq!(status(&service)[0].problem, None, "it's looked for again");
    assert_eq!(ask(&service, Request::Start { name: "win".into() }), [Answer::Ok]);
    wait_for(&service, "win", State::Running);
    assert_eq!(service.work.booted.lock().unwrap()[0].disk, drive.join("win").join("win.vhdx"));
    ask(&service, Request::Stop { name: "win".into() });
    wait_for(&service, "win", State::Off);
}

/// A machine that's set up and has two snapshots, so its disks are a chain, and a file of each kind.
fn with_snapshots(setup: &Setup, name: &str) {
    set_up(setup, name);
    let service = &setup.service;
    for label in ["one", "two"] {
        let answer = ask(service, Request::Snapshot { name: name.into(), label: label.into() });
        assert_eq!(answer, [Answer::Ok]);
        wait_for_event(service, name, &format!("Took the snapshot {label}."));
    }
}

#[test]
fn a_stopped_machine_is_moved_with_its_snapshots_and_back_again() {
    let setup = setup("move", Pretend::default());
    let service = &setup.service;
    let drive = drive(&setup);
    with_snapshots(&setup, "win");
    let home = setup.root.join("machines").join("win");
    let before = names(&home);
    assert!(before.contains(&"snapshots".to_owned()) && before.contains(&"win.account.txt".to_owned()));

    let to = Request::Move { name: "win".into(), folder: drive.clone() };
    assert_eq!(ask(service, to), [Answer::Ok]);
    wait_for_event(service, "win", "Moved its disks to");
    let there = drive.join("win");
    assert_eq!(status(service)[0].folder, there);
    assert_eq!(names(&home), ["events.log", "location.json", "machine.json"], "the old copy is gone");
    let mut after = names(&there);
    after.sort();
    assert_eq!(
        after,
        before
            .iter()
            .filter(|name| !["events.log", "machine.json"].contains(&name.as_str()))
            .cloned()
            .collect::<Vec<_>>()
    );
    // Each disk made on another reads from the new place, and not the old.
    let child = fs::read_to_string(there.join("win.vhdx")).unwrap();
    assert!(child.starts_with(&format!("on {}", there.join("snapshots").display())), "{child}");
    let second = fs::read_to_string(there.join("snapshots").join("2.vhdx")).unwrap();
    assert!(second.starts_with(&format!("on {}", there.join("snapshots").display())), "{second}");
    assert_eq!(status(service)[0].snapshots.len(), 2);
    let notes = history(service, "win");
    assert!(notes.iter().any(|event| matches!(event, Event::Note(said) if said.starts_with("Copying "))));
    assert!(
        notes.iter().any(|event| matches!(event, Event::Note(said) if said.starts_with("Checked the copy")))
    );

    // It's found there by a service that starts afresh, and boots from there.
    let again = Service::new(Store::new(setup.root.join("machines")), Pretend::default());
    assert_eq!(status(&again)[0].folder, there);
    assert_eq!(ask(&again, Request::Start { name: "win".into() }), [Answer::Ok]);
    wait_for(&again, "win", State::Running);
    ask(&again, Request::Stop { name: "win".into() });
    wait_for(&again, "win", State::Off);
    drop(again);

    // And back to where machines are kept, with the empty folder.
    let back = Request::Move { name: "win".into(), folder: PathBuf::new() };
    assert_eq!(ask(service, back), [Answer::Ok]);
    wait_for_event(service, "win", &format!("Moved its disks to {}", home.display()));
    assert_eq!(status(service)[0].folder, home);
    let mut now = names(&home);
    now.sort();
    assert_eq!(now, before);
    assert!(!there.exists(), "the folder it was moved from is gone");
    let child = fs::read_to_string(home.join("win.vhdx")).unwrap();
    assert!(child.starts_with(&format!("on {}", home.join("snapshots").display())), "{child}");
}

#[test]
fn a_move_keeps_to_the_services_rules() {
    let setup = setup("move-rules", Pretend::default());
    let service = &setup.service;
    let drive = drive(&setup);
    fs::create_dir_all(drive.join("link")).unwrap();
    set_up(&setup, "win");
    let there = |folder: &Path| Request::Move { name: "win".into(), folder: folder.to_owned() };
    let home = setup.root.join("machines").join("win");

    assert!(
        refused_with(&ask(service, Request::Move { name: "nobody".into(), folder: drive.clone() }))
            .contains("no machine")
    );
    assert!(
        refused_with(&ask(service, there(&setup.root.join("machines")))).contains("already"),
        "to where it is"
    );
    assert!(refused_with(&ask(service, there(Path::new("drive")))).contains("own drives"));
    assert!(refused_with(&ask(service, there(&home))).contains("inside"), "into its own folder");
    assert!(refused_with(&ask_as(service, there(&drive), false)).contains("may not read"));
    assert!(refused_with(&ask(service, there(&drive.join("link")))).contains("link"));
    assert!(refused_with(&ask(service, there(&drive.join("missing")))).contains("isn't a folder"));
    fs::create_dir_all(drive.join("win")).unwrap();
    assert!(
        refused_with(&ask(service, there(&drive))).contains("there already"),
        "not over a folder that's there"
    );
    fs::remove_dir(drive.join("win")).unwrap();
    assert_eq!(status(service)[0].folder, home, "none of it moved anything");
    assert!(names(&drive).iter().all(|name| name == "link"));

    // Moved back to where machines are kept, it has to be empty of anything but the machine's own.
    assert_eq!(ask(service, there(&drive)), [Answer::Ok]);
    wait_for_event(service, "win", "Moved its disks to");
    fs::write(home.join("stray"), b"x").unwrap();
    let back = Request::Move { name: "win".into(), folder: PathBuf::new() };
    assert!(refused_with(&ask(service, back)).contains("aren't the machine's own"));
}

#[test]
fn a_running_machine_isnt_moved() {
    let setup = setup("move-running", Pretend::default());
    let service = &setup.service;
    let drive = drive(&setup);
    set_up(&setup, "win");
    ask(service, Request::Start { name: "win".into() });
    wait_for(service, "win", State::Running);
    let answers = ask(service, Request::Move { name: "win".into(), folder: drive.clone() });
    assert!(refused_with(&answers).contains("running"), "{answers:?}");
    assert!(names(&drive).is_empty());
    ask(service, Request::Stop { name: "win".into() });
    wait_for(service, "win", State::Off);
}

#[test]
fn a_move_needs_room_for_every_file_on_the_other_drive() {
    let setup = setup("move-room", Pretend { free_space: Some(1024), ..Pretend::default() });
    let service = &setup.service;
    let drive = drive(&setup);
    set_up(&setup, "win");
    let answers = ask(service, Request::Move { name: "win".into(), folder: drive.clone() });
    assert!(refused_with(&answers).contains("isn't room"), "{answers:?}");
    assert!(names(&drive).is_empty());
}

#[test]
fn a_move_that_fails_leaves_the_machine_as_it_was_and_no_copy() {
    let setup = setup("move-fails", Pretend::default());
    let service = &setup.service;
    let drive = drive(&setup);
    with_snapshots(&setup, "win");
    let home = setup.root.join("machines").join("win");
    let before = names(&home);
    let disk_before = fs::read_to_string(home.join("win.vhdx")).unwrap();
    service.work.disks.fail_set_parent.store(true, Ordering::SeqCst);

    assert_eq!(ask(service, Request::Move { name: "win".into(), folder: drive.clone() }), [Answer::Ok]);
    wait_for_event(service, "win", "is where it was.");
    assert!(names(&drive).is_empty(), "the partial copy is deleted, and the folder it was made in is left");
    assert_eq!(names(&home), before);
    assert_eq!(fs::read_to_string(home.join("win.vhdx")).unwrap(), disk_before);
    assert_eq!(status(service)[0].folder, home);
    assert!(!home.join("location.json").exists());
    assert!(
        history(service, "win")
            .iter()
            .any(|event| matches!(event, Event::Problem(said) if said.contains("can't be opened")))
    );

    // It still starts, from where it was, and the move can be asked again.
    assert_eq!(ask(service, Request::Start { name: "win".into() }), [Answer::Ok]);
    wait_for(service, "win", State::Running);
    ask(service, Request::Stop { name: "win".into() });
    wait_for(service, "win", State::Off);
    service.work.disks.fail_set_parent.store(false, Ordering::SeqCst);
    assert_eq!(ask(service, Request::Move { name: "win".into(), folder: drive.clone() }), [Answer::Ok]);
    wait_for_event(service, "win", "Moved its disks to");
}

#[test]
fn a_move_that_cant_keep_its_record_leaves_the_machine_as_it_was_and_no_copy() {
    let setup = setup("move-record", Pretend::default());
    let service = &setup.service;
    let drive = drive(&setup);
    set_up(&setup, "win");
    // A folder where the record's temporary file goes, so it can't be written.
    fs::create_dir(setup.root.join("machines").join("win").join("location.new.json")).unwrap();
    assert_eq!(ask(service, Request::Move { name: "win".into(), folder: drive.clone() }), [Answer::Ok]);
    wait_for_event(service, "win", "is where it was.");
    assert!(names(&drive).is_empty());
    assert!(setup.root.join("machines").join("win").join("win.vhdx").is_file());
    assert!(!setup.root.join("machines").join("win").join("location.json").exists());
}

#[test]
fn a_drive_that_goes_while_the_service_runs_is_a_problem_at_once_and_is_back_with_the_drive() {
    let setup = setup("vanishes", Pretend::default());
    let service = &setup.service;
    let drive = drive(&setup);
    ask(service, Request::Create { machine: placed(&setup, "win", &drive) });
    assert_eq!(status(service)[0].problem, None);
    let away = setup.root.join("drive-away");
    fs::rename(&drive, &away).unwrap();

    let shown = &status(service)[0];
    assert!(shown.problem.as_deref().unwrap().contains("isn't there"), "{:?}", shown.problem);
    assert_eq!(shown.folder, drive.join("win"), "it still says where the disks should be");
    assert!(refused_with(&ask(service, Request::Start { name: "win".into() })).contains("isn't there"));
    fs::rename(&away, &drive).unwrap();
    assert_eq!(status(service)[0].problem, None);
}

#[test]
fn a_place_that_cant_be_trusted_is_never_read() {
    let setup = setup("never-read", Pretend::default());
    let drive = drive(&setup);
    let trusted = Arc::new(AtomicBool::new(true));
    let hold = {
        let trusted = Arc::clone(&trusted);
        Arc::new(move |_: &Path| {
            if trusted.load(Ordering::SeqCst) {
                Ok(Box::new(()) as Held)
            } else {
                Err("It's owned by a user.".to_owned())
            }
        })
    };
    let service = Service::new(Store::new(setup.root.join("machines")).with_hold(hold), Pretend::default());
    ask(&service, Request::Create { machine: placed(&setup, "win", &drive) });
    let disk = service.machines().store.disk("win");
    Record { installed: true, ..Record::default() }.save(&disk).unwrap();
    assert!(status(&service)[0].set_up);
    assert!(matches!(
        ask(&service, Request::Account { name: "win".into() }).as_slice(),
        [Answer::Account { .. }]
    ));

    trusted.store(false, Ordering::SeqCst);
    let shown = &status(&service)[0];
    assert!(shown.problem.as_deref().unwrap().contains("owned by a user"));
    assert!(!shown.set_up, "its record in the place isn't read");
    let account = ask(&service, Request::Account { name: "win".into() });
    assert!(!format!("{account:?}").contains("secret"), "{account:?}");
    let sent = ask(&service, Request::ReportFiles { machine: Some("win".into()) });
    let named: Vec<&str> = sent
        .iter()
        .filter_map(|answer| match answer {
            Answer::ReportPart { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    assert!(named.contains(&"machine.json"), "its own folder's files are still its: {named:?}");
    assert!(
        named.iter().all(|name| !name.ends_with("guesthouse.json") && !name.contains("snapshots")),
        "{named:?}"
    );
}

#[cfg(unix)]
#[test]
fn a_link_in_a_machines_folder_stops_a_move() {
    let setup = setup("move-link", Pretend::default());
    let service = &setup.service;
    let drive = drive(&setup);
    set_up(&setup, "win");
    std::os::unix::fs::symlink("/etc", setup.root.join("machines").join("win").join("elsewhere")).unwrap();
    let answers = ask(service, Request::Move { name: "win".into(), folder: drive.clone() });
    assert!(refused_with(&answers).contains("is a link"), "{answers:?}");
    assert!(names(&drive).is_empty());
}

/// The remote view's switch and its list of removed devices are the service's, kept at home with
/// the settings: a move takes them nowhere, and they're there after a move out and back and a
/// service that starts again. Deleting the machine takes them with it.
#[test]
fn a_moved_machine_keeps_its_remote_view_switch_and_removed_devices_at_home() {
    let setup = setup("move-view", Pretend::default());
    let service = &setup.service;
    let drive = drive(&setup);
    ask(service, Request::Create { machine: new_machine(&setup, "win") });
    let disk = service.machines().store.disk("win");
    Record { installed: true, ..Record::default() }.save(&disk).unwrap();
    let share = Request::Share { name: "win".into(), on: Some(true), pairing: None, pairing_seconds: None };
    assert!(matches!(ask(service, share).as_slice(), [Answer::Share { .. }]));
    let home = setup.root.join("machines").join("win");
    let phone = "cd".repeat(32);
    revoked::add(&home, &phone).unwrap();

    assert_eq!(ask(service, Request::Move { name: "win".into(), folder: drive.clone() }), [Answer::Ok]);
    wait_for_event(service, "win", "Moved its disks to");
    for own in ["remote-view.json", revoked::FILE] {
        assert!(names(&home).contains(&own.to_owned()), "{own}");
        assert!(!names(&drive.join("win")).contains(&own.to_owned()), "{own}");
    }
    assert_eq!(revoked::removed(&home), std::slice::from_ref(&phone));
    let again = Service::new(Store::new(setup.root.join("machines")), Pretend::default());
    let listed = |service: &Arc<Service<Pretend>>| {
        let machine = &status(service)[0];
        (machine.remote_view, machine.remote_port)
    };
    assert_eq!(listed(&again), (true, Some(7792)));
    drop(again);

    let back = Request::Move { name: "win".into(), folder: PathBuf::new() };
    assert_eq!(ask(service, back), [Answer::Ok]);
    wait_for_event(service, "win", &format!("Moved its disks to {}", home.display()));
    let again = Service::new(Store::new(setup.root.join("machines")), Pretend::default());
    assert_eq!(listed(&again), (true, Some(7792)));
    assert_eq!(revoked::removed(&home), [phone]);
    drop(again);
    assert_eq!(ask(service, Request::Delete { name: "win".into() }), [Answer::Ok]);
    assert!(!home.exists());
}
