//! The screen and the app windows as two modes of one machine: what each does about the other.
use super::windows_tests::{Rig, rig};
use super::*;
use crate::app_windows::modes::{self, Ending, ScreenMark};
use crate::app_windows::viewer_lock::{self, ViewerLock};
use crate::testing::Scratch;
use std::cell::RefCell;
use std::time::Duration;

/// A /proc with one process in it: what it is called and how it was started.
fn proc_with(scratch: &Scratch, pid: u32, comm: &str, words: &[&str]) -> std::path::PathBuf {
    let proc = scratch.path("proc");
    let folder = proc.join(pid.to_string());
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("comm"), format!("{comm}\n")).unwrap();
    fs::write(folder.join("cmdline"), words.join("\0") + "\0").unwrap();
    proc
}

/// The machine's viewer, as its lock and its recorded process id.
fn viewer_runs(rig: &Rig, pid: u32) -> ViewerLock {
    let lock = viewer_lock::take(&rig.context, crate::slots::first()).unwrap().expect("no viewer yet");
    fs::write(viewer_lock::lock_file(&rig.context, crate::slots::first()), pid.to_string()).unwrap();
    lock
}

fn free(rig: &Rig, ending: &Ending) -> Result<()> {
    modes::free_for_screen(&rig.context, &rig.machine, crate::slots::first(), ending)
}

#[test]
fn a_machine_with_no_viewer_is_left_alone() {
    let scratch = Scratch::new();
    let rig = rig(&scratch, Kind::Windows, true, &[]);
    let proc = scratch.path("proc");
    let terminate = |_: u32| -> std::io::Result<()> { panic!("nothing runs to end") };
    let ending = Ending { proc: &proc, terminate: &terminate, wait: Duration::from_millis(300) };
    assert_eq!(free(&rig, &ending), Ok(()));
    assert!(rig.agent.requests.lock().unwrap().is_empty(), "no helper is stopped for nothing");
}

#[test]
fn a_viewer_is_ended_and_waited_for_and_then_the_helper_is_stopped() {
    let scratch = Scratch::new();
    let rig = rig(&scratch, Kind::Windows, true, &[]);
    let proc = proc_with(&scratch, 4242, "guesthouse.real", &["/p/guesthouse.real", "--app-windows", "work"]);
    let lock = RefCell::new(Some(viewer_runs(&rig, 4242)));
    let asked = RefCell::new(Vec::new());
    // The viewer lets go of the machine when it is asked, as it does on SIGTERM.
    let terminate = |pid: u32| -> std::io::Result<()> {
        asked.borrow_mut().push(pid);
        lock.borrow_mut().take();
        Ok(())
    };
    let ending = Ending { proc: &proc, terminate: &terminate, wait: Duration::from_millis(300) };
    assert_eq!(free(&rig, &ending), Ok(()));
    assert_eq!(*asked.borrow(), [4242]);
    assert!(!viewer_lock::held(&rig.context, crate::slots::first()).unwrap());
    let requests = rig.agent.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].2, "$helperName = 'app-windows'\nscript", "the helper in the guest is ended");
}

#[test]
fn a_process_that_is_not_this_machines_viewer_is_never_signalled() {
    for (comm, words) in [
        ("sshd", &["/p/guesthouse.real", "--app-windows", "work"][..]),
        ("guesthouse.real", &["/p/guesthouse.real", "--app-windows", "other"]),
        ("guesthouse.real", &["/p/guesthouse.real"]),
        ("guesthouse.real", &["/p/guesthouse.real", "--app", "work"]),
    ] {
        let scratch = Scratch::new();
        let rig = rig(&scratch, Kind::Windows, true, &[]);
        let proc = proc_with(&scratch, 4242, comm, words);
        let _lock = viewer_runs(&rig, 4242);
        let terminate = |_: u32| -> std::io::Result<()> { panic!("{comm} {words:?} is not the viewer") };
        let ending = Ending { proc: &proc, terminate: &terminate, wait: Duration::from_millis(300) };
        let failure = free(&rig, &ending).unwrap_err();
        assert!(failure.0.contains("could not tell to end"), "{failure}");
        assert!(rig.agent.requests.lock().unwrap().is_empty());
    }
    // A lock file with no process id in it, or none that is there, is the same.
    let scratch = Scratch::new();
    let rig = rig(&scratch, Kind::Windows, true, &[]);
    let _lock = viewer_runs(&rig, 4242);
    let proc = scratch.path("proc");
    let terminate = |_: u32| -> std::io::Result<()> { panic!("no such process") };
    let ending = Ending { proc: &proc, terminate: &terminate, wait: Duration::from_millis(300) };
    assert!(free(&rig, &ending).is_err());
}

#[test]
fn a_viewer_that_does_not_end_is_said_and_the_helper_is_left_running() {
    let scratch = Scratch::new();
    let rig = rig(&scratch, Kind::Windows, true, &[]);
    let proc =
        proc_with(&scratch, 4242, "guesthouse.real", &["/p/guesthouse.real", "--app-windows", "Office PC"]);
    let _lock = viewer_runs(&rig, 4242);
    let terminate = |_: u32| -> std::io::Result<()> { Ok(()) };
    let ending = Ending { proc: &proc, terminate: &terminate, wait: Duration::from_millis(300) };
    let failure = free(&rig, &ending).unwrap_err();
    assert!(failure.0.starts_with("The app windows of Office PC did not end in"), "{failure}");
    assert!(
        rig.agent.requests.lock().unwrap().is_empty(),
        "the viewer still shows windows: the helper stays"
    );
}

#[test]
fn the_screen_mark_is_one_per_machine_and_goes_with_its_holder() {
    let scratch = Scratch::new();
    let rig = rig(&scratch, Kind::Windows, true, &[]);
    let slot = crate::slots::first();
    assert!(!modes::screen_open(&rig.context, slot).unwrap());
    let mark: Option<ScreenMark> = modes::mark_screen(&rig.context, slot).unwrap();
    assert!(mark.is_some());
    assert!(modes::screen_open(&rig.context, slot).unwrap());
    assert!(modes::mark_screen(&rig.context, slot).unwrap().is_none(), "a second screen is turned away");
    drop(mark);
    // Another test's child, forked while the mark was held, keeps its copy of the lock until it has
    // become its own program, so the mark is looked at until it is free.
    let free = (0..200).any(|_| {
        let open = modes::screen_open(&rig.context, slot).unwrap();
        if open {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        !open
    });
    assert!(free);
}

#[test]
fn a_look_at_the_locks_never_turns_away_the_one_that_takes_them() {
    let scratch = Scratch::new();
    let rig = rig(&scratch, Kind::Windows, true, &[]);
    let slot = crate::slots::first();
    let looking = std::sync::atomic::AtomicBool::new(true);
    std::thread::scope(|threads| {
        threads.spawn(|| {
            while looking.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = viewer_lock::held(&rig.context, slot);
                let _ = modes::screen_open(&rig.context, slot);
            }
        });
        // The looking ends however this ends, a failed assertion too, or the scope would wait for ever.
        struct Stop<'a>(&'a std::sync::atomic::AtomicBool);
        impl Drop for Stop<'_> {
            fn drop(&mut self) {
                self.0.store(false, std::sync::atomic::Ordering::Relaxed);
            }
        }
        let _stop = Stop(&looking);
        for _ in 0..50 {
            let lock = viewer_lock::take(&rig.context, slot).unwrap();
            assert!(lock.is_some(), "a viewer was turned away by a look");
            drop(lock);
            let mark = modes::mark_screen(&rig.context, slot).unwrap();
            assert!(mark.is_some(), "a screen was turned away by a look");
        }
    });
}

#[test]
fn opening_an_app_while_the_screen_is_open_is_refused_in_words_and_starts_nothing() {
    let scratch = Scratch::new();
    let rig = rig(&scratch, Kind::Windows, true, &[]);
    let _mark = modes::mark_screen(&rig.context, crate::slots::first()).unwrap().unwrap();
    let mut start = |_: &Machine| -> Result<Box<dyn ViewerProcess>> { panic!("no viewer beside the screen") };
    let mut out = Vec::new();
    let opening = Opening { machine: "work", program: "notepad.exe", start_menu: false };
    let failure = open_program(&rig.context, &opening, &mut start, &mut out).unwrap_err();
    assert_eq!(
        failure.0,
        "The screen of Office PC is open in Guesthouse, and a machine shows one or the other. \
         Close the screen and open the app again."
    );
    assert!(rig.helper.requests().is_empty(), "the program was not started either");
}

#[test]
fn the_viewer_records_its_process_for_whoever_has_to_end_it() {
    let scratch = Scratch::new();
    let rig = rig(&scratch, Kind::Windows, true, &[]);
    let slot = crate::slots::first();
    assert_eq!(viewer_lock::recorded_pid(&rig.context.run_folder(Some(slot))), None);
    let lock = viewer_lock::take(&rig.context, slot).unwrap().unwrap();
    lock.record_pid().unwrap();
    assert_eq!(viewer_lock::recorded_pid(&rig.context.run_folder(Some(slot))), Some(std::process::id()));
}

/// QEMU and the daemon may write anything in the run folder, so a lock and a pid there are not the
/// viewer's.
#[test]
fn a_lock_in_the_run_folder_is_not_the_viewers() {
    let scratch = Scratch::new();
    let rig = rig(&scratch, Kind::Windows, true, &[]);
    let proc = proc_with(&scratch, 4242, "guesthouse.real", &["/p/guesthouse.real", "--app-windows", "work"]);
    rig.context.make_run_folder(None).unwrap();
    let file = rig.context.run_folder(None).join("app-windows-viewer.lock");
    fs::write(&file, "4242").unwrap();
    let held = fs::File::open(&file).unwrap();
    rustix::fs::flock(&held, rustix::fs::FlockOperation::NonBlockingLockExclusive).unwrap();
    let terminate =
        |_: u32| -> std::io::Result<()> { panic!("a lock in the run folder is not the viewer's") };
    let ending = Ending { proc: &proc, terminate: &terminate, wait: Duration::from_millis(300) };
    assert_eq!(free(&rig, &ending), Ok(()));
}
