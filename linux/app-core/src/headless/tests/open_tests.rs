use super::windows_tests::{Rig, rig, rig_with, start_process};
use super::*;
use crate::app_windows::viewer_lock::{self, ViewerLock};
use crate::testing::Scratch;
use std::cell::RefCell;
use std::fs;
use std::time::Duration;

thread_local! {
    /// The locks of the viewers a test started, which are other processes in life and so outlive the call.
    static HELD: RefCell<Vec<ViewerLock>> = const { RefCell::new(Vec::new()) };
}

/// A pretend viewer that holds the machine's lock, as the real one does while it lives.
struct Holding;

impl ViewerProcess for Holding {
    fn ended(&mut self) -> Option<String> {
        None
    }
}

/// A viewer that is gone as soon as it is started, with what the system would say.
struct Gone(&'static str);

impl ViewerProcess for Gone {
    fn ended(&mut self) -> Option<String> {
        Some(self.0.to_owned())
    }
}

fn opening<'a>(program: &'a str, start_menu: bool) -> Opening<'a> {
    Opening { machine: "work", program, start_menu }
}

/// Opens a program with a viewer that holds the lock, and counts how many viewers were started.
fn open_counting(
    rig: &Rig,
    program: &str,
    start_menu: bool,
    started: &RefCell<u32>,
) -> (String, std::result::Result<(), Failure>) {
    let context = rig.context.clone();
    let mut start = |_: &Machine| -> Result<Box<dyn ViewerProcess>> {
        *started.borrow_mut() += 1;
        let lock = viewer_lock::take(&context, crate::slots::first()).unwrap().expect("no viewer yet");
        HELD.with(|held| held.borrow_mut().push(lock));
        Ok(Box::new(Holding))
    };
    let mut out = Vec::new();
    let result = open_program(&rig.context, &opening(program, start_menu), &mut start, &mut out);
    (String::from_utf8(out).unwrap(), result)
}

fn open_with(rig: &Rig, start: StartViewer) -> (String, std::result::Result<(), Failure>) {
    let mut out = Vec::new();
    let result = open_program(&rig.context, &opening(r"C:\Windows\notepad.exe", false), start, &mut out);
    (String::from_utf8(out).unwrap(), result)
}

#[test]
fn a_running_machine_gets_the_program_and_one_viewer_that_a_second_launch_reuses() {
    let scratch = Scratch::new();
    let rig = rig(&scratch, Kind::Windows, true, &[]);
    let started = RefCell::new(0);
    let (out, result) = open_counting(&rig, r"C:\Windows\notepad.exe", false, &started);
    assert_eq!(result, Ok(()));
    assert_eq!(
        out,
        "C:\\Windows\\notepad.exe was started in Office PC.\nOpened the app windows viewer for Office PC.\n"
    );
    // The viewer is still alive, so the lock is held, and nothing opens a second one.
    let (out, result) = open_counting(&rig, r"C:\Windows\calc.exe", false, &started);
    assert_eq!(result, Ok(()));
    assert_eq!(
        out,
        "C:\\Windows\\calc.exe was started in Office PC.\n\
         Office PC's app windows are already shown by the viewer that is open.\n"
    );
    assert_eq!(*started.borrow(), 1);
    let paths: Vec<String> =
        rig.helper.requests().iter().map(|request| request["path"].as_str().unwrap().to_owned()).collect();
    assert_eq!(paths, [r"C:\Windows\notepad.exe", r"C:\Windows\calc.exe"]);
    assert!(rig.agent.requests.lock().unwrap().is_empty());
}

#[test]
fn a_start_menu_program_is_launched_by_its_id() {
    let scratch = Scratch::new();
    let rig = rig(&scratch, Kind::Windows, true, &[]);
    let started = RefCell::new(0);
    let (_, result) = open_counting(&rig, "Accessories/Paint", true, &started);
    assert_eq!(result, Ok(()));
    let seen = rig.helper.requests();
    assert_eq!((seen[0]["t"].as_str(), seen[0]["app"].as_str()), (Some("launch"), Some("Accessories/Paint")));
    assert!(seen[0]["path"].is_null());
}

#[test]
fn a_machine_that_is_off_is_started_and_then_waited_for() {
    let scratch = Scratch::new();
    let rig = rig_with(&scratch, Kind::Windows, true, &[], true, false);
    let asked = scratch.path("asked");
    let go = scratch.path("go");
    scratch.program(
        "program/guesthouse-task",
        &format!(
            "#!/bin/sh\necho \"$@\" > '{}'\nwhile [ ! -f '{}' ]; do sleep 0.05; done\n",
            asked.display(),
            go.display()
        ),
    );
    // The pretend task's work: once it was asked, the machine is there.
    let (context, machine, port) = (rig.context.clone(), rig.machine.clone(), rig.port);
    let (asked_file, go_file) = (asked.clone(), go.clone());
    let worker = std::thread::spawn(move || {
        for _ in 0..200 {
            if asked_file.exists() {
                let qemu = start_process(&context, &machine, port);
                fs::write(&go_file, "").unwrap();
                return Some(qemu);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        None
    });
    let started = RefCell::new(0);
    let (out, result) = open_counting(&rig, r"C:\Windows\notepad.exe", false, &started);
    let _qemu = worker.join().unwrap();
    assert_eq!(result, Ok(()), "{out}");
    assert_eq!(fs::read_to_string(&asked).unwrap().trim(), "start work", "the same task `start` runs");
    assert!(out.contains("Office PC is running."), "{out}");
    assert_eq!(rig.helper.requests().len(), 1, "then the program was started");
    assert_eq!(*started.borrow(), 1);
}

#[test]
fn a_machine_that_cannot_do_this_is_not_started_for_it() {
    let scratch = Scratch::new();
    let rig = rig_with(&scratch, Kind::Windows, false, &["started\r\n"], true, false);
    let asked = scratch.path("asked");
    scratch.program("program/guesthouse-task", &format!("#!/bin/sh\necho \"$@\" > '{}'\n", asked.display()));
    let started = RefCell::new(0);
    let (_, result) = open_counting(&rig, r"C:\Windows\notepad.exe", false, &started);
    assert_eq!(
        result.unwrap_err().0,
        "App windows is off for Office PC. guesthouse-vm set work app-windows on turns it on, and the machine \
         gets what it needs at its next start."
    );
    assert!(!asked.exists(), "no start was asked");
    assert_eq!((*started.borrow(), rig.helper.connections()), (0, 0));
    assert!(rig.agent.requests.lock().unwrap().is_empty());

    let other = Scratch::new();
    let plain = rig_with(&other, Kind::Plain, true, &[], true, false);
    let (_, result) = open_counting(&plain, "x", false, &started);
    assert_eq!(
        result.unwrap_err().0,
        "Office PC is not a Windows machine, and only Windows has a helper for this."
    );
    assert_eq!(*started.borrow(), 0);
}

#[test]
fn a_machine_that_is_stopping_is_not_started_over() {
    let scratch = Scratch::new();
    let rig = rig(&scratch, Kind::Windows, true, &[]);
    let record = crate::tasks::TaskRecord {
        action: "stop".into(),
        machine: "work".into(),
        pid: std::process::id(),
        ..Default::default()
    };
    crate::tasks::write_task(&rig.context, None, &record).unwrap();
    let fake = crate::processes::stand_in::fake_process;
    fake(
        &rig.context,
        std::process::id(),
        &[&rig.context.paths.binary("guesthouse-task").display().to_string()],
        &[],
        "",
    );
    let started = RefCell::new(0);
    let (_, result) = open_counting(&rig, "x", false, &started);
    assert_eq!(result.unwrap_err().0, "Office PC is stopping, so wait for it to be running.");
    assert_eq!(*started.borrow(), 0);
}

#[test]
fn a_viewer_that_ends_at_once_is_reported_with_its_reason() {
    let scratch = Scratch::new();
    let rig = rig(&scratch, Kind::Windows, true, &[]);
    let mut start = |_: &Machine| -> Result<Box<dyn ViewerProcess>> { Ok(Box::new(Gone("exit status: 1"))) };
    let (_, result) = open_with(&rig, &mut start);
    assert_eq!(
        result.unwrap_err().0,
        "The app windows viewer for Office PC ended before it showed anything: exit status: 1. \
         guesthouse --app-windows work says why on its own."
    );
    assert_eq!(rig.helper.requests().len(), 1, "the program was started all the same");
}

#[test]
fn a_viewer_that_loses_a_race_for_the_lock_is_not_a_failure() {
    let scratch = Scratch::new();
    let rig = rig(&scratch, Kind::Windows, true, &[]);
    // Another launch's viewer got in between: this one's ended at once, and the lock is theirs.
    let rival = RefCell::new(None);
    let context = rig.context.clone();
    let mut start = |_: &Machine| -> Result<Box<dyn ViewerProcess>> {
        *rival.borrow_mut() = viewer_lock::take(&context, crate::slots::first()).unwrap();
        Ok(Box::new(Gone("exit status: 0")))
    };
    let (out, result) = open_with(&rig, &mut start);
    assert_eq!(result, Ok(()));
    assert!(out.ends_with("Opened the app windows viewer for Office PC.\n"), "{out}");
}

#[test]
fn a_viewer_that_never_takes_the_lock_is_reported() {
    struct Idle;
    impl ViewerProcess for Idle {
        fn ended(&mut self) -> Option<String> {
            None
        }
    }
    let scratch = Scratch::new();
    let rig = rig(&scratch, Kind::Windows, true, &[]);
    let mut start = |_: &Machine| -> Result<Box<dyn ViewerProcess>> { Ok(Box::new(Idle)) };
    let (_, result) = open_with(&rig, &mut start);
    assert_eq!(result.unwrap_err().0, "The app windows viewer for Office PC did not start.");
}

#[test]
fn a_helper_that_never_answers_is_said_and_no_viewer_is_started() {
    let scratch = Scratch::new();
    let rig = super::windows_tests::rig_listening(&scratch, Kind::Windows, true, &["started\r\n"], false);
    let started = RefCell::new(0);
    let (_, result) = open_counting(&rig, "x", false, &started);
    assert!(result.is_err());
    assert_eq!(*started.borrow(), 0);
}

#[test]
fn a_guest_with_nobody_logged_in_yet_is_waited_for_until_the_patience_is_up() {
    let scratch = Scratch::new();
    let answers = ["nobody is logged in"; 40];
    let rig = super::windows_tests::rig_listening(&scratch, Kind::Windows, true, &answers, false);
    let started = RefCell::new(0);
    let (_, result) = open_counting(&rig, "x", false, &started);
    assert_eq!(
        result.unwrap_err().0,
        "Nobody is logged in to Office PC, so there is no session for the app windows helper."
    );
    assert!(rig.agent.requests.lock().unwrap().len() >= 2, "it asked again while it waited");
    assert_eq!(*started.borrow(), 0);
}

#[test]
fn the_viewer_lock_is_one_per_machine_and_goes_with_its_holder() {
    let scratch = Scratch::new();
    let rig = rig(&scratch, Kind::Windows, true, &[]);
    let slot = crate::slots::first();
    assert!(!viewer_lock::held(&rig.context, slot).unwrap());
    let first = viewer_lock::take(&rig.context, slot).unwrap();
    assert!(first.is_some());
    assert!(viewer_lock::take(&rig.context, slot).unwrap().is_none(), "a second viewer is turned away");
    assert!(viewer_lock::held(&rig.context, slot).unwrap());
    drop(first);
    // Another test's child, forked while the lock was held, keeps its copy of the lock until it has
    // become its own program, so the lock is looked at until it is free.
    let free = (0..200).any(|_| {
        let held = viewer_lock::held(&rig.context, slot).unwrap();
        if held {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        !held
    });
    assert!(free);
}

#[test]
fn the_words_after_apps_and_launcher_are_checked() {
    assert!(usage_of("apps").contains("apps needs a machine."));
    assert!(usage_of("apps a b").contains("apps takes only a machine."));
    assert!(usage_of("launcher a").contains("launcher needs a machine and list, add or remove."));
    assert!(usage_of("launcher a add").contains("launcher add needs a program."));
    assert!(usage_of("launcher a list x").contains("launcher list takes no program."));
    assert!(usage_of("launcher a frob x").contains("launcher has no action frob."));
    assert!(usage_of("app a b --start-menu c").contains("app takes only a machine and a program."));
    assert!(
        USAGE.contains("apps MACHINE") && USAGE.contains("launcher MACHINE list|add PROGRAM|remove PROGRAM")
    );
}
