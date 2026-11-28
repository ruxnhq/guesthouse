use super::*;
use std::cell::{Cell, RefCell};

const VIEWER: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const PHONE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const IDLE: &str = r#"{"pairing":false,"clipboard":true,"files":true,"devices":[]}"#;

/// A guest whose host says `status` from a line the test changes, and `ok` to everything else
/// unless told otherwise, changing what it says next as a real host would; and a host that writes
/// down every line asked, logged and held.
#[derive(Default)]
struct Fake {
    status: RefCell<String>,
    /// Answers for commands other than `status`, by their words joined.
    answers: Vec<(&'static str, &'static str)>,
    asked: RefCell<Vec<String>>,
    logged: RefCell<Vec<String>>,
    held: Cell<Option<HostWide>>,
    /// Every `held` kept, with how many commands had gone out when it was.
    kept: RefCell<Vec<(Option<HostWide>, usize)>>,
    /// Commands the host says `ok` to and doesn't do, this many times, by their words joined.
    ignored: RefCell<Vec<(&'static str, usize)>>,
    /// Lines whose answer is lost on the way, once each: the command still runs when `ran`.
    lost: RefCell<Vec<(&'static str, bool)>>,
}

impl Fake {
    fn saying(status: &str) -> Fake {
        Fake { status: RefCell::new(status.to_string()), ..Fake::default() }
    }

    /// What a command it took changes in what the host says next, as a real host's would.
    fn obey(&self, words: &Words) {
        let mut status = self.status.borrow_mut();
        let Ok(serde_json::Value::Object(mut fields)) = serde_json::from_str(&status) else { return };
        let words: Vec<&str> = words.iter().map(String::as_str).collect();
        match words.as_slice() {
            ["pair", on] => fields.insert("pairing".into(), (*on == "on").into()),
            ["allow", what, on] => fields.insert((*what).into(), (*on == "on").into()),
            _ => return,
        };
        *status = serde_json::Value::Object(fields).to_string();
    }

    fn asked(&self) -> Vec<String> {
        self.asked.borrow().clone()
    }

    /// The commands asked, without the readings.
    fn sent(&self) -> Vec<String> {
        self.asked().into_iter().filter(|line| line != "status").collect()
    }
}

impl Guest for Fake {
    fn ctl(&self, words: &Words) -> Result<String, Unanswered> {
        let line = words.join(" ");
        self.asked.borrow_mut().push(line.clone());
        let lost = self.lost.borrow().iter().position(|(lost, _)| *lost == line);
        let lost = lost.map(|at| self.lost.borrow_mut().remove(at).1);
        if lost == Some(false) {
            return Err(Unanswered("no answer".into()));
        }
        if line == "status" {
            return Ok(self.status.borrow().clone());
        }
        let answer = self.answers.iter().find(|(asked, _)| *asked == line).map(|(_, said)| *said);
        let mut ignored = self.ignored.borrow_mut();
        let ignore = ignored.iter_mut().find(|(asked, times)| *asked == line && *times > 0);
        match (answer, ignore) {
            (Some(_), _) => {}
            (None, Some((_, times))) => *times -= 1,
            (None, None) => self.obey(words),
        }
        if lost.is_some() {
            return Err(Unanswered("the answer was cut short".into()));
        }
        Ok(answer.unwrap_or("ok").to_string())
    }

    fn log(&self, line: &str) {
        self.logged.borrow_mut().push(line.to_string());
    }

    fn held(&self) -> Option<HostWide> {
        self.held.get()
    }

    fn keep_held(&self, held: Option<HostWide>) {
        self.held.set(held);
        self.kept.borrow_mut().push((held, self.sent().len()));
    }
}

/// A host with nowhere to keep `held`, as the Windows host is.
struct Forgetful(Fake);

impl Guest for Forgetful {
    fn ctl(&self, words: &Words) -> Result<String, Unanswered> {
        self.0.ctl(words)
    }

    fn log(&self, line: &str) {
        self.0.log(line)
    }
}

fn driver<'a>(
    sessions: &'a Sessions,
    policy: &'a Policy,
    guest: &'a dyn Guest,
    now: &'a dyn Fn() -> Millis,
) -> Driver<'a> {
    Driver { sessions, machine: "win", policy, guest, now }
}

#[test]
fn opening_pairing_reads_first_then_sends_its_commands_in_order_and_reads_again() {
    let (sessions, policy, guest) = (Sessions::default(), Policy::linux(), Fake::saying(IDLE));
    let clock = || 1_000_000;
    let driver = driver(&sessions, &policy, &guest, &clock);
    assert_eq!(driver.open_pairing(120), None);
    let asked = guest.asked();
    assert_eq!(asked.first().map(String::as_str), Some("status"), "{asked:?}");
    assert_eq!(guest.sent(), ["allow clipboard off", "allow files off", "pair on"]);
    assert_eq!(asked.last().map(String::as_str), Some("status"), "read again after the commands: {asked:?}");
    let view = sessions.with("win", |session| session.pairing.as_ref().unwrap().view());
    assert!(view.open);
    assert_eq!(view.until_ms, Some(1_000_000 + 120_000));
}

#[test]
fn pairing_cannot_open_before_the_host_answers() {
    let (sessions, policy, guest) = (Sessions::default(), Policy::linux(), Fake::saying(""));
    let clock = || 5;
    let driver = driver(&sessions, &policy, &guest, &clock);
    assert_eq!(driver.open_pairing(120).as_deref(), Some(crate::pairing::NOT_READ_YET));
    assert!(guest.sent().is_empty(), "{:?}", guest.sent());
    sessions.with("win", |session| {
        assert_eq!(session.reason.as_deref(), Some("The host said nothing."));
        assert_eq!(session.read_at, Some(5));
    });
}

/// `held` is written down after the commands that hold the switches off have gone out, before
/// anything else is asked, so a host that dies there still owes them back.
#[test]
fn what_is_held_is_kept_as_soon_as_its_commands_have_gone() {
    let (sessions, policy, guest) = (Sessions::default(), Policy::linux(), Fake::saying(IDLE));
    let clock = || 1_000;
    driver(&sessions, &policy, &guest, &clock).open_pairing(120);
    let both = Some(HostWide { clipboard: true, files: true });
    assert!(guest.kept.borrow().contains(&(both, 3)), "{:?}", guest.kept.borrow());
    assert_eq!(guest.held.get(), both);
}

/// An earlier run died with pairing open and the switches held off. This run's first reading
/// closes pairing and gives both back, and has nothing left to keep.
#[test]
fn a_run_that_starts_on_a_machine_left_pairing_gives_back_what_the_last_run_held() {
    let open = r#"{"pairing":true,"clipboard":false,"files":false,"devices":[]}"#;
    let (sessions, policy, guest) = (Sessions::default(), Policy::linux(), Fake::saying(open));
    guest.held.set(Some(HostWide { clipboard: true, files: true }));
    let clock = || 10_000;
    driver(&sessions, &policy, &guest, &clock).started();
    let sent = guest.sent();
    for line in ["pair off", "allow clipboard on", "allow files on"] {
        assert!(sent.iter().any(|sent| sent == line), "{line} in {sent:?}");
    }
    assert_eq!(guest.held.get(), None);
}

/// A host with no file for `held`, with Guesthouse's own viewer on its list, runs the same
/// driver: the host-wide switches are held as on Linux, with nothing kept, and the viewer is no
/// device.
#[test]
fn a_windows_host_with_nowhere_to_keep_held_runs_the_same_driver() {
    let status = format!(
        r#"{{"pairing":false,"clipboard":true,"files":true,"devices":[{{"fingerprint":"{VIEWER}","name":"Guesthouse","connected":true}}]}}"#
    );
    let (sessions, policy) = (Sessions::default(), Policy::windows(VIEWER));
    let guest = Forgetful(Fake::saying(&status));
    let clock = || 1_000;
    let driver = driver(&sessions, &policy, &guest, &clock);
    assert_eq!(driver.open_pairing(60), None);
    assert_eq!(guest.0.sent(), ["allow clipboard off", "allow files off", "pair on"]);
    assert!(guest.0.kept.borrow().is_empty());
    let shown = sessions
        .with("win", |session| session.shown("win", &policy, ShareState::Listening, Some(7793), None));
    assert!(shown.devices.is_empty(), "{:?}", shown.devices);
    assert!(shown.pairing);
    assert_eq!(driver.device(&policy.device_revoke(PHONE).unwrap()), Ok(()));
    assert_eq!(guest.0.sent().last().map(String::as_str), Some(format!("revoke {PHONE}").as_str()));
}

#[test]
fn a_command_the_host_refuses_is_logged_and_the_steps_go_on() {
    let (sessions, policy) = (Sessions::default(), Policy::linux());
    let guest = Fake { answers: vec![("pair on", "error: not now")], ..Fake::saying(IDLE) };
    let clock = || 1_000;
    driver(&sessions, &policy, &guest, &clock).open_pairing(120);
    assert_eq!(*guest.logged.borrow(), ["ctl pair on: error: not now"]);
}

#[test]
fn the_clock_is_the_one_passed_in() {
    let (sessions, policy, guest) = (Sessions::default(), Policy::linux(), Fake::saying(IDLE));
    let now = Cell::new(1_000_000);
    let clock = || now.get();
    let driver = driver(&sessions, &policy, &guest, &clock);
    driver.open_pairing(10);
    assert_eq!(driver.poll_ms(), Some(crate::pairing::POLL_MS));
    // Ten seconds later the time is up: a reading is asked for before pairing closes.
    now.set(1_010_000);
    let before = guest.sent().len();
    driver.step(StepEvent::Plain(Event::Tick));
    let after = &guest.sent()[before..];
    assert_eq!(after, ["pair off", "allow clipboard on", "allow files on"], "closed and given back");
    assert_eq!(driver.poll_ms(), None);
    sessions.with("win", |session| assert_eq!(session.read_at, Some(1_010_000)));
}

#[test]
fn a_reading_keeps_the_hosts_fingerprint_through_a_failed_one() {
    let with = format!(r#"{{"fingerprint":"{PHONE}","pairing":false,"devices":[]}}"#);
    let (sessions, policy, guest) = (Sessions::default(), Policy::linux(), Fake::saying(&with));
    let clock = || 1;
    let driver = driver(&sessions, &policy, &guest, &clock);
    assert!(driver.read().is_some());
    *guest.status.borrow_mut() = "no pipe".to_string();
    assert!(driver.read().is_none());
    *guest.status.borrow_mut() = r#"{"pairing":false,"devices":[]}"#.to_string();
    assert!(driver.read().is_some());
    sessions.with("win", |session| {
        assert_eq!(session.fingerprint.as_deref(), Some(PHONE));
        assert_eq!(session.reason, None, "a good reading clears the reason");
    });
    *guest.status.borrow_mut() = ctl::STOPPING.to_string();
    assert!(driver.read().is_none());
    sessions.with("win", |session| assert_eq!(session.reason, None, "stopping is no failure"));
}

#[test]
fn a_device_command_gives_the_hosts_refusal_and_reads_only_after_one_it_took() {
    let (sessions, policy) = (Sessions::default(), Policy::linux());
    let guest = Fake { answers: vec![("drop", "error: nobody is connected")], ..Fake::saying(IDLE) };
    let clock = || 1;
    let driver = driver(&sessions, &policy, &guest, &clock);
    assert_eq!(driver.device(&ctl::device_drop()), Err("error: nobody is connected".to_string()));
    assert_eq!(guest.asked(), ["drop"]);
    assert_eq!(driver.device(&ctl::device_revoke(PHONE).unwrap()), Ok(()));
    assert_eq!(guest.asked()[1..], [format!("revoke {PHONE}"), "status".to_string()]);
    let stopping = Fake { answers: vec![("drop", ctl::STOPPING)], ..Fake::saying(IDLE) };
    let driver = Driver { guest: &stopping, ..driver };
    assert_eq!(driver.device(&ctl::device_drop()), Err(HOST_STOPPING.to_string()));
}

#[test]
fn a_reading_is_stale_after_two_seconds_or_when_the_clock_went_back() {
    let session = Session { read_at: Some(10_000), ..Session::default() };
    assert!(!session.stale(10_000));
    assert!(!session.stale(11_999));
    assert!(session.stale(12_000));
    assert!(session.stale(9_999));
    assert!(Session::default().stale(0));
}

#[test]
fn the_screen_shows_the_hosts_own_failure_before_a_failed_reading() {
    let session = Session { reason: Some("no pipe".into()), ..Session::default() };
    let policy = Policy::linux();
    let shown = session.shown("win", &policy, ShareState::Failed, Some(7792), Some("port taken".into()));
    assert_eq!(shown.reason.as_deref(), Some("port taken"));
    assert_eq!(shown.port, Some(7792));
    let shown = session.shown("win", &policy, ShareState::Starting, None, None);
    assert_eq!(shown.reason.as_deref(), Some("no pipe"));
    assert!(!shown.pairing);
}

#[test]
fn sessions_are_kept_by_machine_and_forgotten_when_removed() {
    let sessions = Sessions::default();
    sessions.with("one", |session| session.busy = true);
    sessions.with("two", |_| {});
    let mut machines = sessions.machines();
    machines.sort();
    assert_eq!(machines, ["one", "two"]);
    sessions.remove("one");
    assert!(!sessions.with("one", |session| session.busy));
}

/// The hang this bound is for: a host that says `ok` to `pair off` and keeps saying pairing is
/// open. The step ends, having sent it no more than allowed, says so, and the give-back that went
/// out is not owed again.
#[test]
fn a_guest_that_never_shows_a_command_taken_ends_the_step_within_the_bound() {
    let open = r#"{"pairing":true,"clipboard":false,"files":false,"devices":[]}"#;
    let (sessions, policy) = (Sessions::default(), Policy::linux());
    let guest = Fake { ignored: RefCell::new(vec![("pair off", usize::MAX)]), ..Fake::saying(open) };
    guest.held.set(Some(HostWide { clipboard: true, files: true }));
    let clock = || 10_000;
    let driver = driver(&sessions, &policy, &guest, &clock);
    sessions.with("win", |session| session.pairing = Some(driver.fresh_pairing()));
    let why = driver.step(StepEvent::Read).expect("the step says why it stopped");
    assert!(why.starts_with("The guest did not take pair off."), "{why}");
    let offs = guest.sent().iter().filter(|line| *line == "pair off").count();
    // Once with the give-back, then on its own once and once more.
    assert_eq!(offs, 1 + SAME_COMMANDS_PER_STEP, "{:?}", guest.sent());
    assert_eq!(guest.held.get(), None, "the give-back went out");
    assert!(guest.logged.borrow().contains(&why));
    // The next reading asks again, and is bounded again.
    assert!(driver.step(StepEvent::Read).is_some());
}

#[test]
fn a_guest_that_shows_it_on_the_second_reading_ends_the_step_well() {
    let open = r#"{"pairing":true,"clipboard":true,"files":true,"devices":[]}"#;
    let (sessions, policy) = (Sessions::default(), Policy::linux());
    let guest = Fake { ignored: RefCell::new(vec![("pair off", 1)]), ..Fake::saying(open) };
    let clock = || 10_000;
    let driver = driver(&sessions, &policy, &guest, &clock);
    sessions.with("win", |session| session.pairing = Some(driver.fresh_pairing()));
    assert_eq!(driver.step(StepEvent::Read), None);
    assert_eq!(guest.sent(), ["pair off", "pair off"]);
    assert!(guest.logged.borrow().is_empty());
}

/// The answer to `allow files off` is lost: the clipboard went off and the files may have. The step
/// stops before `pair on`, both are owed back, and the next step reads before it acts.
#[test]
fn an_unanswered_command_stops_the_step_and_keeps_owing_what_may_be_off() {
    let (sessions, policy) = (Sessions::default(), Policy::linux());
    let guest = Fake { lost: RefCell::new(vec![("allow files off", true)]), ..Fake::saying(IDLE) };
    let clock = || 1_000_000;
    let driver = driver(&sessions, &policy, &guest, &clock);
    let why = driver.open_pairing(120).expect("the step says why it stopped");
    assert!(why.starts_with("The guest did not answer allow files off: the answer was cut short."), "{why}");
    assert_eq!(guest.sent(), ["allow clipboard off", "allow files off"], "pair on never went");
    let both = Some(HostWide { clipboard: true, files: true });
    assert_eq!(guest.held.get(), both);
    assert!(!sessions.with("win", |session| session.pairing.as_ref().unwrap().view().open));
    assert!(sessions.with("win", |session| session.unsure));

    // The next step reads first, so it opens on what the host says now.
    let asked = guest.asked().len();
    assert_eq!(driver.step(StepEvent::Plain(Event::Open { seconds: 120 })), None);
    assert_eq!(guest.asked()[asked], "status");
    assert_eq!(guest.sent().last().map(String::as_str), Some("pair on"));
    assert!(!sessions.with("win", |session| session.unsure));
    assert_eq!(guest.held.get(), both, "still owed, and given back when pairing closes");
    driver.step(StepEvent::Plain(Event::Stop));
    assert_eq!(guest.held.get(), None);
    assert_eq!(guest.sent()[guest.sent().len() - 3..], ["pair off", "allow clipboard on", "allow files on"]);
}

/// A give-back whose answer is lost is owed still, and written down as owed.
#[test]
fn what_is_held_is_kept_across_a_step_that_stops_short() {
    let (sessions, policy) = (Sessions::default(), Policy::linux());
    let guest = Fake::saying(IDLE);
    let clock = || 1_000_000;
    let driver = driver(&sessions, &policy, &guest, &clock);
    driver.open_pairing(120);
    let both = Some(HostWide { clipboard: true, files: true });
    assert_eq!(guest.held.get(), both);
    guest.lost.borrow_mut().push(("allow clipboard on", false));
    let why = driver.step(StepEvent::Plain(Event::Stop)).unwrap();
    assert!(why.starts_with("The guest did not answer allow clipboard on: no answer."), "{why}");
    assert_eq!(guest.held.get(), both, "nothing given back is forgotten");
    assert_eq!(guest.kept.borrow().last().unwrap().0, both);
}

#[test]
fn nothing_is_sent_while_the_guest_still_does_not_answer() {
    let (sessions, policy) = (Sessions::default(), Policy::linux());
    let guest = Fake { lost: RefCell::new(vec![("pair on", false)]), ..Fake::saying(IDLE) };
    let clock = || 1_000_000;
    let driver = driver(&sessions, &policy, &guest, &clock);
    assert!(driver.open_pairing(120).is_some());
    guest.lost.borrow_mut().push(("status", false));
    let sent = guest.sent().len();
    assert_eq!(driver.step(StepEvent::Plain(Event::Open { seconds: 120 })).as_deref(), Some(STILL_UNSURE));
    assert_eq!(guest.sent().len(), sent, "{:?}", guest.sent());
}

#[test]
fn a_device_command_that_is_not_answered_says_so_and_the_next_step_reads_first() {
    let (sessions, policy) = (Sessions::default(), Policy::linux());
    let guest = Fake { lost: RefCell::new(vec![("drop", false)]), ..Fake::saying(IDLE) };
    let clock = || 1;
    let driver = driver(&sessions, &policy, &guest, &clock);
    let why = driver.device(&ctl::device_drop()).unwrap_err();
    assert!(why.starts_with("The guest did not answer drop: no answer."), "{why}");
    assert_eq!(guest.asked(), ["drop"]);
    assert!(sessions.with("win", |session| session.unsure));
}

/// A status that names a new device at every reading, with one waiting at every other, so that
/// each round asks for something new: the step still ends, within the rounds a step may send.
#[test]
fn a_status_that_names_a_new_device_at_every_reading_still_ends() {
    struct Restless {
        readings: Cell<u32>,
        sent: Cell<usize>,
    }
    impl Guest for Restless {
        fn ctl(&self, words: &Words) -> Result<String, Unanswered> {
            if words.join(" ") != "status" {
                self.sent.set(self.sent.get() + 1);
                return Ok("ok".into());
            }
            let n = self.readings.get();
            self.readings.set(n + 1);
            let devices: Vec<String> = (0..=n)
                .map(|i| format!(r#"{{"fingerprint":"{:064x}","name":"d{i}"}}"#, u64::from(i) + 1))
                .collect();
            let pending = if n.is_multiple_of(2) {
                format!(
                    r#","pending":{{"name":"p","fingerprint":"{:064x}","code":"1","seconds":60}}"#,
                    1000 + n
                )
            } else {
                String::new()
            };
            Ok(format!(
                r#"{{"pairing":false,"clipboard":true,"files":true,"devices":[{}]{pending}}}"#,
                devices.join(",")
            ))
        }
        fn log(&self, _line: &str) {}
    }
    let (sessions, policy) = (Sessions::default(), Policy::linux());
    let guest = Restless { readings: Cell::new(0), sent: Cell::new(0) };
    let clock = || 1_000_000;
    let driver = driver(&sessions, &policy, &guest, &clock);
    for event in [StepEvent::Read, StepEvent::Plain(Event::Open { seconds: 120 }), StepEvent::Read] {
        let before = guest.readings.get();
        driver.step(event);
        let read = guest.readings.get() - before;
        assert!(read as usize <= ROUNDS_PER_STEP + 1, "{read} readings in one step");
    }
    assert!(guest.sent.get() > 0);
}
