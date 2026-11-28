// SPDX-License-Identifier: GPL-2.0-or-later
//! Asking a program what it can do before it is trusted with a machine: a graphics process
//! with no frame export would start a machine with no screen, and a QEMU without `pvg-fwd`
//! would fail with a message nobody can act on. Each check runs the program for a moment with
//! a harmless argument and reads what it says.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long a program gets to answer. They answer at once or not at all.
const ANSWER_WAIT: Duration = Duration::from_secs(5);

/// What was learned about each program, by its path, size and time of change: a program
/// answers the same until it is replaced, and asking runs it, which must not happen every
/// time a window is drawn.
fn cached(program: &Path, question: &str, ask: impl FnOnce() -> bool) -> bool {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static ANSWERS: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
    let version = std::fs::metadata(program)
        .map(|m| format!("{}:{:?}", m.len(), m.modified().ok()))
        .unwrap_or_default();
    let key = format!("{}|{question}|{version}", program.display());
    let answers = ANSWERS.get_or_init(Mutex::default);
    if let Some(known) = answers.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return *known;
    }
    let answer = ask();
    answers.lock().unwrap_or_else(|e| e.into_inner()).insert(key, answer);
    answer
}

/// What a program printed, and whether it ended well.
pub(crate) struct Ran {
    pub succeeded: bool,
    pub stdout: String,
    pub stderr: String,
}

/// What `program args` prints on both outputs, or `None` if it would not run or did not answer.
fn say(program: &Path, args: &[&str]) -> Option<String> {
    let ran = run_briefly(program, args)?;
    Some(ran.stdout + &ran.stderr)
}

/// Runs `program args` and waits for it, but never longer than `ANSWER_WAIT` in all: `None` if
/// it would not run or was still going then (it is killed). Every program the agent runs to
/// answer a request goes through here, so that a request cannot wait on a program for good.
///
/// The pipes are read on their own threads: a read that ends only at end of file can wait for
/// good when some other process holds the pipe's write end. That can happen: a pipe made just
/// before another thread starts a program, on macOS, may be inherited by that program, which
/// may live for hours. The program's own output is complete once it has exited, so a read
/// that is still going at the deadline is given up on and its thread left to end alone.
pub(crate) fn run_briefly(program: impl AsRef<std::ffi::OsStr>, args: &[&str]) -> Option<Ran> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let readers = [reader_of(child.stdout.take()?), reader_of(child.stderr.take()?)];
    let deadline = Instant::now() + ANSWER_WAIT;
    let status = loop {
        if let Some(status) = child.try_wait().ok()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let [stdout, stderr] =
        readers.map(|reader| reader.recv_timeout(deadline.saturating_duration_since(Instant::now())).ok());
    Some(Ran { succeeded: status.success(), stdout: stdout?, stderr: stderr? })
}

/// Reads `pipe` to its end on a thread of its own and sends what it read.
fn reader_of(mut pipe: impl Read + Send + 'static) -> std::sync::mpsc::Receiver<String> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        let _ = pipe.read_to_string(&mut text);
        let _ = sender.send(text);
    });
    receiver
}

/// Whether the graphics process can publish its frames (`--frames`). The stock build answers
/// an option it does not know with a usage line that has no `--frames`, which says so.
pub fn pvg_host_publishes_frames(program: &Path) -> bool {
    cached(program, "frames", || say(program, &["--help"]).is_some_and(|text| text.contains("--frames")))
}

/// Whether QEMU has a device called `device`, from its own list (`-device help`).
pub fn qemu_has_device(program: &Path, device: &str) -> bool {
    cached(program, device, || say(program, &["-device", "help"]).is_some_and(|text| text.contains(device)))
}

/// What the graphics process says to `status`: a line such as `ok picture 1024x768 presented 9
/// read 3 frames 803 refused 0 spoiled false published 803 skipped 0 dropped 0 failed 0`,
/// as JSON: every word followed by a number becomes a number, `picture` stays "WxH".
pub fn graphics_status(control: &Path) -> Result<serde_json::Value, String> {
    use std::io::{BufRead, BufReader, Write};
    let mut stream = std::os::unix::net::UnixStream::connect(control)
        .map_err(|e| format!("the graphics process is not answering: {e}"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    stream.write_all(b"status\n").map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).map_err(|e| e.to_string())?;
    Ok(parse_status(&line))
}

fn parse_status(line: &str) -> serde_json::Value {
    let mut numbers = serde_json::Map::new();
    let words: Vec<&str> = line.split_whitespace().collect();
    for pair in words.windows(2) {
        if let Ok(n) = pair[1].parse::<u64>() {
            numbers.insert(pair[0].to_string(), n.into());
        } else if let Ok(flag) = pair[1].parse::<bool>() {
            numbers.insert(pair[0].to_string(), flag.into());
        } else if pair[0] == "picture" {
            numbers.insert("picture".into(), pair[1].into());
        }
    }
    numbers.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn script(name: &str, body: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("gh-probe-{name}-{}", std::process::id()));
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn a_graphics_process_without_frames_is_told_by_its_usage_line() {
        let with = script("with", r#"echo "usage: x --socket P [--frames NAME]" >&2; exit 1"#);
        let without = script("without", r#"echo "usage: x --socket P [--window]" >&2; exit 1"#);
        assert!(pvg_host_publishes_frames(&with));
        assert!(!pvg_host_publishes_frames(&without));
        assert!(!pvg_host_publishes_frames(Path::new("/no/such/program")));
        let _ = (std::fs::remove_file(with), std::fs::remove_file(without));
    }

    #[test]
    fn a_qemu_is_asked_for_its_devices() {
        let has = script("has", r#"printf 'Misc devices:\nname "pvg-fwd", bus PCI\n'"#);
        let hasnt = script("hasnt", r#"printf 'Misc devices:\nname "apple-gfx-pci"\n'"#);
        assert!(qemu_has_device(&has, "pvg-fwd"));
        assert!(!qemu_has_device(&hasnt, "pvg-fwd"));
        let _ = (std::fs::remove_file(has), std::fs::remove_file(hasnt));
    }

    #[test]
    fn the_status_line_becomes_numbers() {
        let line = "ok picture 1024x768 presented 9 read 3 frames 803 interrupts 4 refused 0 spoiled false published 803 skipped 0 dropped 2 failed 0";
        let status = parse_status(line);
        assert_eq!(status["published"], 803);
        assert_eq!(status["dropped"], 2);
        assert_eq!(status["refused"], 0);
        assert_eq!(status["picture"], "1024x768");
        assert_eq!(status["spoiled"], false);
        assert_eq!(parse_status("ok spoiled true")["spoiled"], true);
    }

    #[test]
    fn a_pipe_another_process_keeps_open_does_not_hold_the_answer_up() {
        // The program is done at once; a process it left behind still holds its output.
        let leaves = script("leaves", "sleep 30 &\necho pvg-fwd");
        let started = Instant::now();
        assert!(!qemu_has_device(&leaves, "pvg-fwd"));
        assert!(started.elapsed() < ANSWER_WAIT + Duration::from_secs(2));
        let _ = std::fs::remove_file(leaves);
    }

    #[test]
    fn a_program_that_never_answers_is_given_up_on() {
        let slow = script("slow", "sleep 30");
        let started = Instant::now();
        assert!(!pvg_host_publishes_frames(&slow));
        assert!(started.elapsed() < ANSWER_WAIT + Duration::from_secs(2));
        let _ = std::fs::remove_file(slow);
    }
}
