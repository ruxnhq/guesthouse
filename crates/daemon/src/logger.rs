//! Log lines to standard error, which the script that starts the daemon sends to a file.
//!
//! That file is often in memory, in the run folder, and the verbose log grows by megabytes a
//! minute. So a log that is a file is kept under a limit: its start stays, the middle goes, and
//! the newest part follows.

use log::{Level, Log, Metadata, Record};
use std::fmt::Display;
use std::fs::File;
use std::io::{self, Seek, SeekFrom, Write};
use std::mem::ManuallyDrop;
use std::os::fd::FromRawFd;
use std::os::unix::fs::FileExt;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The most a log file holds.
const MOST_BYTES: u64 = 64 << 20;
/// What stays of its start, where the card, the record and the self-test are.
const START_BYTES: u64 = 8 << 20;
/// What is carried over of its newest part when the middle goes.
const NEWEST_BYTES: u64 = 8 << 20;

struct StandardError {
    /// When the daemon started, if lines are to say how long after it they come. Only the verbose log
    /// has times, as other output is read by people and scripts that expect it as it is.
    started: Option<Instant>,
    /// Bytes written since the file was last checked, if standard error is a file.
    since_checked: Option<Mutex<u64>>,
}

/// How many bytes go by between looks at the file's size.
const CHECK_EVERY: u64 = 1 << 20;

/// Cuts the middle out of a log that has passed `most` bytes: the first `start` bytes stay, a line
/// says what happened, and the last `newest` bytes follow from their first whole line. The file is
/// written where it ends afterwards. A file that can't be read back keeps its start alone.
fn keep_within(file: &mut File, most: u64, start: u64, newest: u64) -> io::Result<bool> {
    let size = file.stream_position()?;
    if size < most || start.saturating_add(newest) >= size {
        return Ok(false);
    }
    let mut tail = vec![0u8; newest as usize];
    let read = file.read_exact_at(&mut tail, size - newest).is_ok();
    file.set_len(start)?;
    file.seek(SeekFrom::Start(start))?;
    writeln!(
        file,
        "\nINFO  the log passed {most} bytes, so its middle was dropped: {} bytes",
        size - start - newest
    )?;
    if read {
        let whole = tail.iter().position(|&byte| byte == b'\n').map_or(0, |at| at + 1);
        file.write_all(&tail[whole..])?;
    }
    Ok(true)
}

impl Log for StandardError {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            match self.started {
                Some(started) => {
                    eprintln!("{}", timed_line(started.elapsed(), record.level(), record.args()))
                }
                None => eprintln!("{:<5} {}", record.level(), record.args()),
            }
            self.keep_small();
        }
    }

    fn flush(&self) {}
}

impl StandardError {
    /// Looks at the file's size now and then, and cuts its middle out when it has grown too big.
    fn keep_small(&self) {
        let Some(since) = &self.since_checked else { return };
        let Ok(mut since) = since.lock() else { return };
        // A line is rarely longer than this, and the limit has megabytes to spare.
        *since += 200;
        if *since < CHECK_EVERY {
            return;
        }
        *since = 0;
        // SAFETY: standard error is open for as long as the process lives, and the file made from
        // it here is never dropped, so it is never closed.
        let mut file = ManuallyDrop::new(unsafe { File::from_raw_fd(2) });
        let _ = keep_within(&mut file, MOST_BYTES, START_BYTES, NEWEST_BYTES);
    }
}

/// Whether standard error is a file, which is what can grow without end.
fn error_is_a_file() -> bool {
    // SAFETY: as in `keep_small`.
    let file = ManuallyDrop::new(unsafe { File::from_raw_fd(2) });
    file.metadata().is_ok_and(|metadata| metadata.is_file())
}

/// A line with the time since the daemon started after its level, so the level stays first. The time
/// is in seconds to the millisecond, and a fixed width keeps the lines aligned.
fn timed_line(elapsed: Duration, level: Level, message: impl Display) -> String {
    format!("{level:<5} [{:>9.3}] {message}", elapsed.as_secs_f64())
}

/// Starts logging. Debug lines cost time on the message path, so they stay off unless asked for.
pub fn start(verbose: bool) {
    let level = if verbose { Level::Debug } else { Level::Info };
    let logger = StandardError {
        started: verbose.then(Instant::now),
        since_checked: error_is_a_file().then(|| Mutex::new(0)),
    };
    if log::set_logger(Box::leak(Box::new(logger))).is_ok() {
        log::set_max_level(level.to_level_filter());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log_file(name: &str, bytes: &[u8]) -> File {
        let path = std::env::temp_dir().join(format!("guesthouse-logger-{}-{name}", std::process::id()));
        let mut file =
            File::options().read(true).write(true).create(true).truncate(true).open(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        file.write_all(bytes).unwrap();
        file
    }

    fn all_of(file: &File) -> String {
        let mut text = vec![0u8; file.metadata().unwrap().len() as usize];
        file.read_exact_at(&mut text, 0).unwrap();
        String::from_utf8(text).unwrap()
    }

    #[test]
    fn a_log_past_its_limit_keeps_its_start_and_its_newest_whole_lines() {
        let lines: String = (0..100).map(|line| format!("line {line:03}\n")).collect();
        let mut file = log_file("cut", lines.as_bytes());
        assert!(keep_within(&mut file, 500, 27, 20).unwrap());
        let text = all_of(&file);
        assert!(text.starts_with("line 000\nline 001\nline 002\n\nINFO  the log passed 500 bytes"), "{text}");
        assert!(text.ends_with("dropped: 853 bytes\nline 098\nline 099\n"), "{text}");
        // What is written next follows the newest part.
        file.write_all(b"next\n").unwrap();
        assert!(all_of(&file).ends_with("line 099\nnext\n"));
    }

    #[test]
    fn a_log_under_its_limit_is_left_alone() {
        let mut file = log_file("small", b"one\ntwo\n");
        assert!(!keep_within(&mut file, 500, 4, 4).unwrap());
        assert_eq!(all_of(&file), "one\ntwo\n");
    }

    #[test]
    fn a_log_that_cannot_be_read_back_keeps_its_start() {
        let path = std::env::temp_dir().join(format!("guesthouse-logger-{}-blind", std::process::id()));
        let mut file = File::create(&path).unwrap();
        file.write_all(&[b'x'; 600]).unwrap();
        assert!(keep_within(&mut file, 500, 100, 100).unwrap());
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(text.starts_with(&"x".repeat(100)) && text.ends_with("dropped: 400 bytes\n"), "{text}");
    }

    #[test]
    fn a_line_has_its_level_then_the_time_in_milliseconds() {
        let line = timed_line(Duration::from_millis(123_456), Level::Debug, "wait on gpu");
        assert_eq!(line, "DEBUG [  123.456] wait on gpu");
    }

    #[test]
    fn times_line_up_from_the_start_to_hours_in() {
        let early = timed_line(Duration::from_millis(5_010), Level::Error, "queue");
        assert_eq!(early, "ERROR [    5.010] queue");
        let late = timed_line(Duration::from_secs(36_000), Level::Info, "queue");
        assert_eq!(late, "INFO  [36000.000] queue");
        assert_eq!(early.find(']'), late.find(']'));
    }
}
