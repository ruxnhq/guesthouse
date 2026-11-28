use super::*;
use crate::events::Kept;
use guesthouse_remote_view_core::ctl;
use guesthouse_remote_view_core::ctl_wire::{answer_lines, failed_line, read_request};
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};

/// For a request whose timeout is what is tested.
const SHORT: Duration = Duration::from_millis(200);
/// For a request that is answered: long, so a loaded machine doesn't fail it.
const LONG: Duration = Duration::from_secs(10);

/// What the guest does with each request: the lines it says back.
type Respond = dyn Fn(Id, &[String]) -> Vec<String> + Send + Sync;

/// A guest on the far end of a port: it keeps what it was sent, and says back what `respond` makes
/// of each request, through the handle as the reader would, on the writing thread.
struct FakeGuest {
    ctl: GuestCtl,
    written: Mutex<Vec<String>>,
    respond: Box<Respond>,
    /// Held while set, as a port the guest doesn't read.
    stuck: AtomicBool,
}

impl LineWriter for FakeGuest {
    fn write_line(&self, line: &[u8]) -> io::Result<()> {
        while self.stuck.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(5));
        }
        let line = String::from_utf8(line.to_vec()).unwrap();
        assert!(line.ends_with('\n'), "{line:?}");
        let line = line.trim_end().to_string();
        self.written.lock().unwrap().push(line.clone());
        if let Some(Ok((id, words))) = read_request(&line) {
            for said in (self.respond)(id, &words) {
                assert!(self.ctl.take_line(&said), "{said}");
            }
        }
        Ok(())
    }
}

fn guest(
    respond: impl Fn(Id, &[String]) -> Vec<String> + Send + Sync + 'static,
) -> (GuestCtl, Arc<FakeGuest>) {
    let ctl = GuestCtl::new();
    let fake = Arc::new(FakeGuest {
        ctl: ctl.clone(),
        written: Mutex::default(),
        respond: Box::new(respond),
        stuck: AtomicBool::new(false),
    });
    ctl.attach(fake.clone(), true);
    (ctl, fake)
}

fn status_json() -> String {
    let devices: Vec<String> = (0..40)
        .map(|at| format!("{{\"name\":\"Zo\u{eb}'s phone {at}\",\"fingerprint\":\"{:064x}\"}}", at))
        .collect();
    format!("{{\"pairing\":true,\"code\":\"123456\",\"devices\":[{}]}}", devices.join(","))
}

#[test]
fn a_request_comes_back_with_the_guests_whole_answer() {
    let said = status_json();
    assert!(said.len() > 4000);
    let answer = said.clone();
    let (ctl, fake) = guest(move |id, words| {
        assert_eq!(words, ctl::status());
        answer_lines(id, 0, answer.as_bytes()).unwrap()
    });
    assert_eq!(ctl.ask(&ctl::status(), LONG), Ok(Answer { code: 0, text: said }));
    assert_eq!(*fake.written.lock().unwrap(), ["guesthouse: ctl ask 1 status"]);
}

#[test]
fn each_request_has_its_own_number_and_an_old_answer_is_not_taken_for_it() {
    let (ctl, _fake) = guest(|id, _| {
        // The answer to the request before comes late, before this one's.
        let mut lines = if id > 1 { answer_lines(id - 1, 0, b"stale").unwrap() } else { Vec::new() };
        lines.extend(answer_lines(id, 0, format!("answer {id}").as_bytes()).unwrap());
        lines
    });
    assert_eq!(ctl.ask(&ctl::pair_open(), LONG).unwrap().text, "answer 1");
    assert_eq!(ctl.ask(&ctl::pair_close(), LONG).unwrap().text, "answer 2");
}

#[test]
fn requests_from_two_threads_go_one_at_a_time_and_each_gets_its_own() {
    let (ctl, _fake) = guest(|id, words| answer_lines(id, 0, words.join(" ").as_bytes()).unwrap());
    let other = ctl.clone();
    let there = thread::spawn(move || other.ask(&ctl::pair_accept(), LONG));
    let here = ctl.ask(&ctl::pair_decline(), LONG);
    assert_eq!(here.unwrap().text, "pair decline");
    assert_eq!(there.join().unwrap().unwrap().text, "pair accept");
}

#[test]
fn words_outside_the_grammar_are_never_written() {
    let (ctl, fake) = guest(|_, _| panic!("nothing is sent"));
    for words in [ctl::share(false), ctl::device_drop(), vec!["status;calc".to_string()]] {
        assert!(matches!(ctl.ask(&words, SHORT), Err(CtlError::NotAllowed(_))), "{words:?}");
    }
    assert!(fake.written.lock().unwrap().is_empty());
}

#[test]
fn a_guest_that_says_nothing_back_is_told_apart_from_one_that_said_part() {
    let (silent, _fake) = guest(|_, _| Vec::new());
    assert_eq!(silent.ask(&ctl::status(), SHORT), Err(CtlError::Silent));
    let (partial, _fake) = guest(|id, _| answer_lines(id, 0, &[b'x'; 3000]).unwrap()[..1].to_vec());
    assert_eq!(partial.ask(&ctl::status(), SHORT), Err(CtlError::Incomplete));
}

#[test]
fn the_guests_reason_for_running_nothing_is_kept() {
    let (ctl, _fake) = guest(|id, _| vec![failed_line(id, Failure::Missing)]);
    assert_eq!(ctl.ask(&ctl::status(), LONG), Err(CtlError::Guest(Failure::Missing)));
}

#[test]
fn a_garbled_answer_is_an_error_not_an_answer() {
    let (ctl, _fake) = guest(|id, _| {
        vec![
            format!("guesthouse: ctl answer {id} 0 1/2 QUJD"),
            format!("guesthouse: ctl answer {id} 1 2/2 QUJD"),
        ]
    });
    assert!(matches!(ctl.ask(&ctl::status(), LONG), Err(CtlError::Garbled(_))));
}

#[test]
fn the_machine_stopping_ends_a_request_at_once() {
    let (ctl, _fake) = guest(|_, _| Vec::new());
    let stopper = ctl.clone();
    let started = Instant::now();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        stopper.end();
    });
    assert_eq!(ctl.ask(&ctl::status(), Duration::from_secs(30)), Err(CtlError::Stopped));
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(ctl.ask(&ctl::status(), SHORT), Err(CtlError::Stopped));
    assert!(!ctl.send_line("guesthouse: shutdown", SHORT));
}

#[test]
fn the_port_closing_ends_a_request_at_once_and_a_new_port_is_used_after() {
    let (ctl, fake) = guest(|_, _| Vec::new());
    let closer = ctl.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        closer.detach();
    });
    assert_eq!(ctl.ask(&ctl::status(), Duration::from_secs(30)), Err(CtlError::PortClosed));
    assert_eq!(ctl.ask(&ctl::status(), SHORT), Err(CtlError::NoPort));
    ctl.attach(fake.clone(), true);
    assert_eq!(ctl.ask(&ctl::status(), SHORT), Err(CtlError::Silent));
}

#[test]
fn a_port_that_takes_nothing_holds_nobody_past_their_time() {
    let (ctl, fake) = guest(|id, _| answer_lines(id, 0, b"ok").unwrap());
    fake.stuck.store(true, Ordering::SeqCst);
    let started = Instant::now();
    assert_eq!(ctl.ask(&ctl::status(), SHORT), Err(CtlError::Busy));
    // The stuck write still holds the port, so the next waits for it and gives up in its own time.
    assert!(!ctl.send_line("guesthouse: shutdown", SHORT));
    assert!(started.elapsed() < Duration::from_secs(5));
    fake.stuck.store(false, Ordering::SeqCst);
    assert_eq!(ctl.ask(&ctl::status(), LONG).unwrap().text, "ok");
}

#[test]
fn only_a_guest_that_answers_is_asked() {
    let ctl = GuestCtl::new();
    assert_eq!(ctl.ask(&ctl::status(), SHORT), Err(CtlError::NoPort));
    let fake = Arc::new(FakeGuest {
        ctl: ctl.clone(),
        written: Mutex::default(),
        respond: Box::new(|_, _| panic!("an Ubuntu guest is never asked")),
        stuck: AtomicBool::new(false),
    });
    ctl.attach(fake.clone(), false);
    assert_eq!(ctl.ask(&ctl::status(), SHORT), Err(CtlError::NotWindows));
    // It is still asked to shut down.
    assert!(ctl.send_line("guesthouse: shutdown", LONG));
    assert_eq!(*fake.written.lock().unwrap(), ["guesthouse: shutdown"]);
}

#[test]
fn the_channels_lines_never_reach_the_sink_whatever_they_are() {
    let ctl = GuestCtl::new();
    let mut said = String::new();
    said.push_str("guesthouse: card: ok\r\n");
    for line in answer_lines(3, 0, status_json().as_bytes()).unwrap() {
        said.push_str(&line);
        said.push_str("\r\n");
        said.push_str("guesthouse: screen: host.log a line between\r\n");
    }
    said.push_str("guesthouse: ctl ask 4 status\n");
    said.push_str("guesthouse: ctl failed 4 missing\n");
    said.push_str("guesthouse: ctl answer zz 123456 code\n");
    said.push_str("guesthouse: ctl \x1b]0;123456\x07\n");
    said.push_str("guesthouse: report done\n");
    let kept = Kept::default();
    let mut handed = Vec::new();
    read_lines(Cursor::new(said.into_bytes()), &mut |line: &str| handed.push(line.to_string()), &kept, &ctl);
    let events = kept.0.lock().unwrap();
    let lines: Vec<&str> = events
        .iter()
        .map(|event| match event {
            Event::Guest(line) => line.as_str(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(lines[0], "guesthouse: card: ok");
    assert_eq!(lines.last(), Some(&"guesthouse: report done"));
    assert!(
        lines[1..lines.len() - 1].iter().all(|line| *line == "guesthouse: screen: host.log a line between")
    );
    assert!(lines.iter().all(|line| !line.contains("ctl") && !line.contains("123456")), "{lines:?}");
    assert_eq!(handed, lines);
}

#[test]
fn an_answer_read_from_the_port_reaches_the_request_waiting() {
    let (ctl, fake) = guest(|_, _| Vec::new());
    let reader = ctl.clone();
    let reading = thread::spawn(move || {
        // Waits for the request to be written, then the guest's answer is read off the port.
        while fake.written.lock().unwrap().is_empty() {
            thread::sleep(Duration::from_millis(5));
        }
        let mut said = String::from("guesthouse: clock: now\n");
        for line in answer_lines(1, 0, b"{\"pairing\":false}").unwrap() {
            said.push_str(&line);
            said.push('\n');
        }
        let kept = Kept::default();
        read_lines(Cursor::new(said.into_bytes()), &mut |_: &str| {}, &kept, &reader);
        kept.0.lock().unwrap().len()
    });
    assert_eq!(ctl.ask(&ctl::status(), LONG).unwrap().text, "{\"pairing\":false}");
    assert_eq!(reading.join().unwrap(), 1);
}

#[test]
fn the_guests_report_answers_in_the_channels_words() {
    use guesthouse_remote_view_core::ctl_wire::{ANSWER, ASK, CHUNK, FAILED, MAX_ANSWER};
    let report = crate::guest::REPORT;
    for said in [
        format!("$ask = '{ASK}'"),
        format!("\"{ANSWER}$Id $Code $($n + 1)/$total $($chunks[$n])\""),
        format!("\"{FAILED}$Id toolong\""),
        format!("$at += {CHUNK}"),
        format!("$Bytes.Length -gt {MAX_ANSWER}"),
        format!("@('ctl', '--port', '{}')", guesthouse_remote_view_core::PORT),
    ] {
        assert!(report.contains(&said), "{said}");
    }
    for failure in [Failure::Refused, Failure::Missing, Failure::Timeout, Failure::Error] {
        assert!(report.contains(&format!("'{}'", failure.word())), "{failure:?}");
    }
}

#[test]
fn the_guests_report_runs_the_program_its_screen_installs() {
    let screen = guesthouse_remote_view_core::SCRIPT;
    assert!(screen.contains(r"$folder = 'C:\Program Files\remoteman host'"));
    assert!(screen.contains("$hostExe = Join-Path $folder 'remoteman-host.exe'"));
    assert!(
        crate::guest::REPORT.contains(r"$ctlHost = 'C:\Program Files\remoteman host\remoteman-host.exe'")
    );
}

/// The shared driver's way in: the guest's words come back as they are, a request that can't have
/// run is a refusal, and whatever may have run is unanswered.
#[test]
fn the_driver_asks_through_the_channel_and_is_told_what_may_have_run() {
    use guesthouse_remote_view_core::session::Guest as _;
    let (ctl, _fake) = guest(|id, words| {
        let said: &[u8] = match words[0].as_str() {
            "status" => br#"{"pairing":false}"#,
            "revoke" => return vec![failed_line(id, Failure::Timeout)],
            "pair" => return vec![failed_line(id, Failure::Refused)],
            _ => b"error: not now",
        };
        answer_lines(id, 1, said).unwrap()
    });
    assert_eq!(ctl.ctl(&ctl::status()).unwrap(), r#"{"pairing":false}"#);
    assert_eq!(ctl.ctl(&ctl::host_allow("files", false).unwrap()).unwrap(), "error: not now");
    assert_eq!(ctl.ctl(&ctl::pair_open()).unwrap(), format!("error: {}", Failure::Refused.sentence()));
    let nothing = vec!["sharing".to_string(), "off".to_string()];
    assert!(ctl.ctl(&nothing).unwrap().starts_with("error: "), "never written, so never run");
    let timed_out = ctl.ctl(&ctl::device_revoke(&"a".repeat(64)).unwrap()).unwrap_err();
    assert_eq!(timed_out.0, Failure::Timeout.sentence());
    ctl.end();
    assert_eq!(ctl.ctl(&ctl::status()).unwrap_err().0, CtlError::Stopped.to_string());
    assert_eq!(GuestCtl::new().ctl(&ctl::status()).unwrap_err().0, CtlError::NoPort.to_string());
}

/// A reading that nobody answers holds the channel for a few seconds, not for a command's time.
#[test]
fn a_status_nobody_answers_gives_up_sooner_than_a_command() {
    use guesthouse_remote_view_core::session::Guest as _;
    let (ctl, _fake) = guest(|_, _| Vec::new());
    let began = Instant::now();
    assert_eq!(ctl.ctl(&ctl::status()).unwrap_err().0, CtlError::Silent.to_string());
    let took = began.elapsed();
    assert!(took >= STATUS_WAIT && took < ANSWER_WAIT, "{took:?}");
}

/// A guest that never ends a line, and then goes away with an error.
struct NoLineEnd(usize);

impl Read for NoLineEnd {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.0 == 0 {
            return Err(io::ErrorKind::ConnectionReset.into());
        }
        let count = buffer.len().min(self.0);
        buffer[..count].fill(b'a');
        self.0 -= count;
        Ok(count)
    }
}

#[test]
fn a_line_that_never_ends_is_taken_in_pieces_and_not_held_whole() {
    let (ctl, kept) = (GuestCtl::new(), Kept::default());
    let mut handed = Vec::new();
    read_lines(NoLineEnd(LINE_LIMIT * 10), &mut |line: &str| handed.push(line.len()), &kept, &ctl);
    assert!(handed.len() >= 9, "{} pieces", handed.len());
}
