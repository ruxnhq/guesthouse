//! What a Windows install's steps do today, held before their plain half moves: the steps it says
//! and their order, each wait's length and how often it looks, the rule that a disk which stops
//! growing means Setup is stuck, how long each guest script is given, every sentence, what an install
//! in progress leaves on disk, and that a failed install starts again from nothing. Time is a fake
//! clock's, so each length is read off it exactly.

use super::super::super::Task;
use super::*;
use crate::agent::GuestAgent;
use crate::tasks::read_task;
use guesthouse_basics::clock::Clock;
use std::sync::Arc;
use std::time::Duration;

/// A guest that grows the disk by a block on each ask it's told to, answers `finished` from a given
/// ask on, and keeps each script with the seconds it was given and the clock's reading.
struct TimedGuest {
    disk: String,
    clock: Arc<FakeClock>,
    grow_until: Option<u32>,
    finish_at: Option<u32>,
    serial: Option<PathBuf>,
    asked: Mutex<Vec<(String, u64, Duration)>>,
}

impl TimedGuest {
    fn new(disk: &str, clock: &Arc<FakeClock>) -> Self {
        TimedGuest {
            disk: disk.to_owned(),
            clock: clock.clone(),
            grow_until: None,
            finish_at: None,
            serial: None,
            asked: Mutex::default(),
        }
    }
}

impl GuestAgent for TimedGuest {
    fn ask(&self, _end: &crate::guest_end::GuestEnd, _token: &str, script: &str, seconds: u64) -> String {
        let mut asked = self.asked.lock().unwrap();
        asked.push((script.to_owned(), seconds, self.clock.now()));
        let count = asked.len() as u32;
        if self.grow_until.is_none_or(|last| count <= last) {
            use std::io::Write;
            let mut disk = fs::OpenOptions::new().append(true).open(&self.disk).unwrap();
            disk.write_all(&[1u8; 1 << 12]).unwrap();
            disk.sync_all().unwrap();
        }
        if let Some(serial) = &self.serial {
            // Windows' boot manager is named twice a boot: one more boot every second ask.
            let mut log = fs::OpenOptions::new().append(true).create(true).open(serial).unwrap();
            use std::io::Write;
            log.write_all(b"\"Windows Boot Manager\"\n").unwrap();
        }
        if self.finish_at.is_some_and(|at| count >= at) { "finished".to_owned() } else { String::new() }
    }
}

fn timed_task(
    scratch: &Scratch,
    guest: impl FnOnce(&str, &Arc<FakeClock>) -> TimedGuest,
) -> (Context, Machine, Arc<FakeClock>, Arc<TimedGuest>) {
    let mut context = scratch_context(scratch);
    with_finish_script(scratch);
    with_vm_running(&context);
    let clock = Arc::new(FakeClock::default());
    context.clock = clock.clone();
    let vm = machine(&context);
    let guest = Arc::new(guest(&vm.disk, &clock));
    context.agent = guest.clone();
    crate::agent::make_token(&context, &vm).unwrap();
    (context, vm, clock, guest)
}

/// Setup that never writes again is stuck once 600 s have gone with no growth, seen at the first look
/// after: 605 s, as it looks every 5 s. A disk that grew at a look starts the 600 s again from then.
#[test]
fn a_disk_quiet_for_more_than_ten_minutes_is_stuck_at_the_first_look_after() {
    let scratch = Scratch::new();
    let (context, vm, clock, guest) = timed_task(&scratch, |disk, clock| TimedGuest {
        grow_until: Some(0),
        ..TimedGuest::new(disk, clock)
    });
    fs::write(&vm.disk, vec![1u8; 1 << 16]).unwrap();
    let monitor = Arc::new(RecordingMonitor::default());
    let mut context = context;
    context.monitor = monitor.clone();
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm);
    let error = wait_for_setup(&mut task).unwrap_err();
    assert_eq!(
        error.0,
        "Windows Setup has not written to the disk for 10 minutes, so it is stuck or waiting for an answer. \
         The VM is still up: Show Screen has what it says."
    );
    assert_eq!(clock.now(), Duration::from_secs(605));
    let asked = guest.asked.lock().unwrap();
    assert_eq!(asked.len(), 121, "one look every 5 s from 0 s to 600 s");
    assert!(asked.iter().all(|(script, seconds, _)| script == "is it finished" && *seconds == 30));
    assert_eq!(asked[1].2 - asked[0].2, Duration::from_secs(5));
    let screen = &monitor.asked()[0];
    assert_eq!(screen.name, "screendump");
    let file = context.run_folder(None).join("stuck-install.ppm");
    assert_eq!(screen.arguments["filename"], file.display().to_string());
    assert_eq!(task.said.lock().unwrap().as_slice(), ["Copying Windows to the disk"]);
}

#[test]
fn growth_starts_the_ten_minutes_again() {
    let scratch = Scratch::new();
    // It grows at each of the first 10 looks, the last at 45 s, so it's stuck at 45 + 605 s.
    let (context, vm, clock, _) = timed_task(&scratch, |disk, clock| TimedGuest {
        grow_until: Some(10),
        ..TimedGuest::new(disk, clock)
    });
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm);
    assert!(wait_for_setup(&mut task).unwrap_err().0.contains("10 minutes"));
    assert_eq!(clock.now(), Duration::from_secs(50 + 605));
}

/// An install that writes all the time but never ends is given two hours.
#[test]
fn setup_that_never_ends_is_given_two_hours() {
    let scratch = Scratch::new();
    let (context, vm, clock, guest) = timed_task(&scratch, TimedGuest::new);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm);
    let error = wait_for_setup(&mut task).unwrap_err();
    assert_eq!(error.0, "Windows setup did not finish in 2 hours. Show Screen says how far it got.");
    assert_eq!(clock.now(), Duration::from_secs(2 * 3600));
    assert_eq!(guest.asked.lock().unwrap().len(), 2 * 3600 / 5);
}

/// What the install says it is doing follows the boot manager's lines in the serial log: one step a
/// boot, the last kept.
#[test]
fn the_steps_shown_follow_the_boots() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    with_finish_script(&scratch);
    with_vm_running(&context);
    let clock = Arc::new(FakeClock::default());
    context.clock = clock.clone();
    let vm = machine(&context);
    // The guest writes where the install reads.
    let serial = context.run_folder(None).join("serial.log");
    fs::create_dir_all(serial.parent().unwrap()).unwrap();
    context.agent = Arc::new(TimedGuest {
        finish_at: Some(12),
        serial: Some(serial),
        ..TimedGuest::new(&vm.disk, &clock)
    });
    crate::agent::make_token(&context, &vm).unwrap();
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm);
    wait_for_setup(&mut task).unwrap();
    assert_eq!(
        task.said.lock().unwrap().as_slice(),
        [
            "Copying Windows to the disk",
            "Windows is setting up its devices",
            "Windows is getting ready",
            "Windows is finishing its setup",
        ]
    );
}

#[test]
fn a_vm_gone_during_setup_says_so_and_its_last_words() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    with_finish_script(&scratch);
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", machine(&context));
    assert_eq!(wait_for_setup(&mut task).unwrap_err().0, "The VM stopped during the install.");
    let folder = context.run_folder(None);
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join(crate::vm_args::OUTPUT_FILE), "qemu: it went\n").unwrap();
    assert_eq!(wait_for_setup(&mut task).unwrap_err().0, "The VM stopped during the install. qemu: it went");
}

/// The firmware is given 30 s to name the disc, looked for every 200 ms, and then a space is sent
/// every 500 ms for 6 s, so the disc boots.
#[test]
fn the_boot_prompt_is_passed_with_a_space_every_half_second_for_six_seconds() {
    let scratch = Scratch::new();
    let mut context = scratch_context(&scratch);
    let clock = Arc::new(FakeClock::default());
    context.clock = clock.clone();
    let monitor = Arc::new(RecordingMonitor::default());
    context.monitor = monitor.clone();
    let host = RecordingHost::default();
    let task = Task::new(&context, &host, "install", machine(&context));
    pass_boot_prompt(&task);
    assert_eq!(clock.now(), Duration::from_secs(30 + 6));
    let keys = monitor.asked();
    assert_eq!(keys.len(), 12);
    assert!(keys.iter().all(|key| key.name == "send-key"));
    assert_eq!(keys[0].arguments, serde_json::json!({"keys": [{"type": "qcode", "data": "spc"}]}));
    // Each look for the firmware's line is 200 ms apart, and each key 500 ms.
    struct Sleeps(Mutex<Vec<Duration>>);
    impl Clock for Sleeps {
        fn now(&self) -> Duration {
            self.0.lock().unwrap().iter().sum()
        }
        fn sleep(&self, time: Duration) {
            self.0.lock().unwrap().push(time);
        }
    }
    let sleeps = Arc::new(Sleeps(Mutex::default()));
    let mut timed = scratch_context(&scratch);
    timed.clock = sleeps.clone();
    timed.monitor = Arc::new(RecordingMonitor::default());
    let timed_task = Task::new(&timed, &host, "install", machine(&timed));
    pass_boot_prompt(&timed_task);
    let slept = sleeps.0.lock().unwrap().clone();
    let mut expected = vec![Duration::from_millis(200); 150];
    expected.extend([Duration::from_millis(500); 12]);
    assert_eq!(slept, expected);
    // With the firmware's line already there, the keys start at once.
    let serial = context.run_folder(None).join("serial.log");
    fs::create_dir_all(serial.parent().unwrap()).unwrap();
    fs::write(&serial, "BdsDxe: starting Boot0001\n").unwrap();
    let before = clock.now();
    pass_boot_prompt(&task);
    assert_eq!(clock.now() - before, Duration::from_secs(6));
}

/// The check of the finished guest is given 60 s, and says the end of what the guest said, up to
/// 300 characters, or that the agent went quiet.
#[test]
fn the_check_says_the_end_of_what_windows_said() {
    let scratch = Scratch::new();
    let (mut context, vm) = install_setup(&scratch);
    let said = format!("{}the end", "é".repeat(400));
    let guest = Arc::new(ScriptedAgent::answering(&[&said, ""]));
    context.agent = guest.clone();
    with_vm_running(&context);
    crate::agent::make_token(&context, &vm).unwrap();
    let host = RecordingHost::default();
    let task = Task::new(&context, &host, "install", vm);
    let error = check_guest(&task).unwrap_err().0;
    let tail: String = said.chars().rev().take(300).collect::<Vec<_>>().into_iter().rev().collect();
    assert_eq!(error, format!("Windows is missing something: {tail}"));
    assert_eq!(check_guest(&task).unwrap_err().0, "Windows is missing something: the agent went quiet");
    assert_eq!(task.said.lock().unwrap().as_slice(), ["Checking Windows", "Checking Windows"]);
}

/// Each guest script is given this long, in seconds.
#[test]
fn each_script_is_given_its_seconds() {
    struct Seconds(Mutex<Vec<(String, u64)>>);
    impl GuestAgent for Seconds {
        fn ask(&self, end: &crate::guest_end::GuestEnd, token: &str, script: &str, seconds: u64) -> String {
            self.0.lock().unwrap().push((script.lines().next().unwrap_or("").to_owned(), seconds));
            InstalledGuest.ask(end, token, script, seconds)
        }
    }
    let scratch = Scratch::new();
    let (mut context, vm) = install_setup(&scratch);
    let guest = Arc::new(Seconds(Mutex::default()));
    context.agent = guest.clone();
    let host = InstallHost { inner: RecordingHost::default(), context: &context };
    let mut task = Task::new(&context, &host, "install", vm);
    assert!(install(&mut task).is_err());
    let asked = guest.0.lock().unwrap().clone();
    let given =
        |first: &str| asked.iter().find(|(line, _)| line.starts_with(first)).map(|(_, seconds)| *seconds);
    assert_eq!(given("is it finished"), Some(30));
    assert_eq!(given("$expected = "), Some(60));
    assert_eq!(given("Stop-Computer -Force"), Some(crate::agent::QUICK_SECONDS));
}

/// The steps an install says, in order, to its end, and that Windows is given 120 s to shut down.
#[test]
fn an_install_says_these_steps_in_this_order() {
    let scratch = Scratch::new();
    let (context, vm) = install_setup(&scratch);
    let clock = Arc::new(FakeClock::default());
    let mut context = context;
    context.clock = clock.clone();
    struct StopTime(Arc<FakeClock>, Mutex<Option<Duration>>);
    impl GuestAgent for StopTime {
        fn ask(&self, end: &crate::guest_end::GuestEnd, token: &str, script: &str, seconds: u64) -> String {
            if script == "Stop-Computer -Force" {
                *self.1.lock().unwrap() = Some(self.0.now());
            }
            InstalledGuest.ask(end, token, script, seconds)
        }
    }
    let guest = Arc::new(StopTime(clock.clone(), Mutex::default()));
    context.agent = guest.clone();
    let host = InstallHost { inner: RecordingHost::default(), context: &context };
    let mut task = Task::new(&context, &host, "install", vm);
    let error = install(&mut task).unwrap_err();
    assert_eq!(error.0, "Windows is installed but would not shut down. Try Force Off, then Start.");
    let stopped_at = guest.1.lock().unwrap().unwrap();
    assert_eq!(clock.now() - stopped_at, Duration::from_secs(120));
    assert_eq!(
        task.said.lock().unwrap().as_slice(),
        [
            "Preparing the install disc",
            "Starting the installer",
            "Copying Windows to the disk",
            "Waiting for the desktop",
            "Checking Windows",
            "Preparing NVIDIA's vendor ID",
            "Shutting down Windows",
        ]
    );
}

/// An install in progress is the machine's file saying so and the task's file saying the step, as the
/// app reads them; one that failed stays one, and the next run starts from nothing, with a new disk
/// and a new answer disc.
#[test]
fn an_install_in_progress_is_on_disk_and_a_failed_one_starts_again_from_nothing() {
    let scratch = Scratch::new();
    let (mut context, vm) = install_setup(&scratch);
    context.agent = Arc::new(ScriptedAgent::default());
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", vm.clone());
    assert!(install(&mut task).unwrap_err().0.starts_with("The VM stopped during the install."));
    let saved: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(vm.folder(&context).join("machine.json")).unwrap()).unwrap();
    assert_eq!(saved["installing"], true);
    assert_eq!(saved["iso"], vm.iso.as_str());
    let record = read_task(&context, None);
    assert_eq!((record.action.as_str(), record.step.as_str()), ("install", "Starting the installer"));
    assert_eq!(record.machine, vm.id);
    assert_eq!(record.pid, std::process::id());
    let text = fs::read_to_string(context.run_folder(None).join("guesthouse-task.json")).unwrap();
    let pid = std::process::id().to_string();
    assert_eq!(
        text.replace(&pid, "PID").replace(&vm.id, "ID"),
        "{\n  \"action\": \"install\",\n  \"machine\": \"ID\",\n  \"pid\": PID,\n  \"step\": \"Starting the installer\",\n  \"error\": \"\"\n}\n"
    );
    // Something written over the disk is gone at the next run, and the answer disc is made again.
    let answer = answer_disc_path(&context, &vm);
    fs::write(&answer, "an old disc").unwrap();
    let before = fs::read(&vm.disk).unwrap();
    let mut task = Task::new(&context, &host, "install", load(&context, &vm.id).unwrap());
    assert!(install(&mut task).is_err());
    assert_ne!(fs::read(&answer).unwrap(), b"an old disc");
    assert_eq!(fs::read(&vm.disk).unwrap().len(), before.len());
    assert!(load(&context, &vm.id).unwrap().installing);
    assert_eq!(task.said.lock().unwrap().first().map(String::as_str), Some("Preparing the install disc"));
}

#[test]
fn every_refusal_before_an_install() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let elsewhere = Machine {
        installing: true,
        ..Machine::new("elsewhere", "Elsewhere", "/nowhere/disk", "/nowhere/vars")
    };
    let host = RecordingHost::default();
    let mut task = Task::new(&context, &host, "install", elsewhere);
    assert_eq!(
        install(&mut task).unwrap_err().0,
        "This machine's disk is not in its folder, so nothing will install over it."
    );
    let (context, mut vm) = install_setup(&scratch);
    vm.iso = scratch.path("gone.iso").display().to_string();
    let mut task = Task::new(&context, &host, "install", vm.clone());
    assert_eq!(install(&mut task).unwrap_err().0, format!("{} is missing.", vm.iso));
    assert_eq!(
        password_is_filled(
            "x <Value>GUESTHOUSE_PASSWORD</Value>".replace("GUESTHOUSE_PASSWORD", PASSWORD_MARK)
        )
        .unwrap_err()
        .0,
        "The answer file has no password in it, so nothing was installed."
    );
}

/// A Windows whose desktop never comes up ends the install with this.
#[test]
fn a_desktop_that_never_comes_up_ends_the_install() {
    struct NoDesktop;
    impl GuestAgent for NoDesktop {
        fn ask(&self, end: &crate::guest_end::GuestEnd, token: &str, script: &str, seconds: u64) -> String {
            if script == DESKTOP_SCRIPT {
                "0".to_owned()
            } else {
                InstalledGuest.ask(end, token, script, seconds)
            }
        }
    }
    let scratch = Scratch::new();
    let (mut context, vm) = install_setup(&scratch);
    context.agent = Arc::new(NoDesktop);
    let host = InstallHost { inner: RecordingHost::default(), context: &context };
    let mut task = Task::new(&context, &host, "install", vm);
    assert_eq!(install(&mut task).unwrap_err().0, "Windows installed, but its desktop never came up.");
    assert_eq!(task.said.lock().unwrap().last().map(String::as_str), Some("Waiting for the desktop"));
}
