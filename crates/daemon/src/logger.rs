//! Log lines to standard error, which the script that starts the daemon sends to a file.
//!
//! That file is often in memory, in the run folder, and the verbose log grows by megabytes a
//! minute. So a log that is a file is kept under a limit: its start stays, the middle goes but for
//! the lines a [`Keeper`] keeps, and the newest part follows.

use log::{Level, Log, Metadata, Record};
use std::collections::VecDeque;
use std::fmt::Display;
use std::fs::File;
use std::io::{self, Seek, SeekFrom, Write};
use std::mem::ManuallyDrop;
use std::os::fd::FromRawFd;
use std::os::unix::fs::FileExt;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The most a log file holds.
const MOST_BYTES: u64 = 64 << 20;
/// What stays of its start, where the card, the record and the self-test are.
const START_BYTES: u64 = 8 << 20;
/// What is carried over of its newest part when the middle goes.
const NEWEST_BYTES: u64 = 8 << 20;
/// The most that is kept of the middle's lines worth keeping, the newest of them. A tester's verbose
/// logs of 341 and 440 MB had 0.32 and 0.42 MB of them.
const MIDDLE_BYTES: u64 = 4 << 20;
/// How much of the middle is read at a time while its lines are picked.
const READ_BYTES: usize = 1 << 20;

/// The levels a line starts with, with debug and trace last.
const LEVELS: [&[u8]; 5] = [b"ERROR", b"WARN", b"INFO", b"DEBUG", b"TRACE"];
/// Where in [`LEVELS`] the debug levels start.
const FIRST_DEBUG_LEVEL: usize = 3;
/// How far into a verbose line its time ends.
const TIME_END: usize = 24;
/// What the core's line that names a process's guest program says, after `process` and its handle.
const PROGRAM_NAMED: &[u8] = b" is guest pid ";

/// Picks the lines of a log worth keeping where the lines around them go: every line that isn't a
/// debug line, which the daemon writes sparingly, and the debug lines that name a guest program, as
/// the rest of the log names processes by handle alone. A line that starts with no level is part of
/// the line before it and goes with it. Lines before the first level go.
#[derive(Debug, Default)]
pub struct Keeper {
    keeping: bool,
}

impl Keeper {
    pub fn keeps(&mut self, line: &[u8]) -> bool {
        if let Some(level) = LEVELS.iter().position(|level| line.starts_with(level)) {
            self.keeping = level < FIRST_DEBUG_LEVEL || names_a_program(line);
        }
        self.keeping
    }
}

/// Whether a line starts a record of the log, with its level, rather than going on with the one
/// before it.
pub fn starts_a_record(line: &[u8]) -> bool {
    LEVELS.iter().any(|level| line.starts_with(level))
}

/// Whether a debug line is the core's line that names the guest program behind a process.
fn names_a_program(line: &[u8]) -> bool {
    // "DEBUG [  463.771] process 0x40181cc0 is guest pid 11900, Heaven.exe", or with no time.
    let message = match line.iter().take(TIME_END).position(|&byte| byte == b']') {
        Some(at) => &line[at + 1..],
        None => line.get(LEVELS[FIRST_DEBUG_LEVEL].len()..).unwrap_or_default(),
    };
    let message = message.trim_ascii_start();
    message.starts_with(b"process ")
        && message.windows(PROGRAM_NAMED.len()).any(|bytes| bytes == PROGRAM_NAMED)
}

static STARTED: OnceLock<Instant> = OnceLock::new();

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
/// says what happened, the middle's lines a [`Keeper`] keeps follow, the newest of them up to
/// `middle` bytes, and then the last `newest` bytes from their first whole line. The file is written
/// where it ends afterwards. A file that can't be read back keeps its start alone.
fn keep_within(file: &mut File, most: u64, start: u64, newest: u64, middle: u64) -> io::Result<bool> {
    let size = file.stream_position()?;
    if size < most || start.saturating_add(newest) >= size {
        return Ok(false);
    }
    let mut tail = vec![0u8; newest as usize];
    let read = file.read_exact_at(&mut tail, size - newest).is_ok();
    let whole = if read { tail.iter().position(|&byte| byte == b'\n').map_or(0, |at| at + 1) } else { 0 };
    // The middle runs up to the newest part's first whole line.
    let middle_end = size - newest + whole as u64;
    let kept =
        if read { kept_lines(file, start, middle_end, middle).unwrap_or_default() } else { Kept::default() };
    file.set_len(start)?;
    file.seek(SeekFrom::Start(start))?;
    let dropped = (middle_end - start).saturating_sub(kept.bytes.len() as u64);
    if kept.lines == 0 {
        writeln!(file, "\nINFO  the log passed {most} bytes, so its middle was dropped: {dropped} bytes")?;
    } else {
        let older = match kept.let_go {
            0 => String::new(),
            bytes => format!(", the newest of them, as {bytes} bytes of older ones didn't fit"),
        };
        let which = match kept.lines {
            1 => "1 line below that isn't a debug line or names".to_owned(),
            lines => format!("{lines} lines below that aren't debug lines or name"),
        };
        writeln!(
            file,
            "\nINFO  the log passed {most} bytes, so its middle was dropped: {dropped} bytes, but for its \
             {which} a guest program{older}"
        )?;
        file.write_all(&kept.bytes)?;
        writeln!(file, "INFO  the log's newest part follows")?;
    }
    if read {
        file.write_all(&tail[whole..])?;
    }
    Ok(true)
}

/// The lines of a log's middle that are kept when the rest of it goes.
#[derive(Debug, Default)]
struct Kept {
    /// The lines, oldest first, each with its line break.
    bytes: Vec<u8>,
    lines: usize,
    /// Bytes of older lines worth keeping that were let go again, as the newer ones filled the room.
    let_go: u64,
}

/// The kept lines, the newest that fit in their room, by record: a line with a level and the lines
/// with none after it, which go together.
#[derive(Debug, Default)]
struct Records {
    records: VecDeque<Vec<u8>>,
    held: u64,
    let_go: u64,
}

impl Records {
    fn push(&mut self, line: &[u8], room: u64) {
        let continues = !starts_a_record(line);
        match self.records.back_mut() {
            Some(record) if continues => record.extend_from_slice(line),
            _ => self.records.push_back(line.to_vec()),
        }
        self.held += line.len() as u64;
        while self.held > room {
            let Some(oldest) = self.records.pop_front() else { break };
            self.held -= oldest.len() as u64;
            self.let_go += oldest.len() as u64;
        }
    }

    fn into_kept(self) -> Kept {
        let bytes: Vec<u8> = self.records.into_iter().flatten().collect();
        let lines = bytes.iter().filter(|&&byte| byte == b'\n').count();
        Kept { bytes, lines, let_go: self.let_go }
    }
}

/// Reads the log from `from` up to `to`, where a line starts, and gives the lines a [`Keeper`] keeps,
/// the newest that fit in `room` bytes. A line that starts before `from` was cut there, and goes.
fn kept_lines(file: &File, from: u64, to: u64, room: u64) -> io::Result<Kept> {
    let mut keeper = Keeper::default();
    let mut records = Records::default();
    let mut before = [0u8; 1];
    let mut cut_line = from > 0 && file.read_exact_at(&mut before, from - 1).is_ok() && before[0] != b'\n';
    let mut buffer = vec![0u8; READ_BYTES];
    let mut pending = Vec::new();
    let mut at = from;
    while at < to {
        let length = (to - at).min(READ_BYTES as u64) as usize;
        file.read_exact_at(&mut buffer[..length], at)?;
        at += length as u64;
        let mut rest = &buffer[..length];
        while let Some(end) = rest.iter().position(|&byte| byte == b'\n') {
            // A line read whole is looked at where it lies; only one split between reads is copied.
            let line = if pending.is_empty() {
                &rest[..=end]
            } else {
                pending.extend_from_slice(&rest[..=end]);
                &pending[..]
            };
            if !std::mem::take(&mut cut_line) && keeper.keeps(line) {
                records.push(line, room);
            }
            pending.clear();
            rest = &rest[end + 1..];
        }
        pending.extend_from_slice(rest);
    }
    Ok(records.into_kept())
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
        let _ = keep_within(&mut file, MOST_BYTES, START_BYTES, NEWEST_BYTES, MIDDLE_BYTES);
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
        started: verbose.then(started),
        since_checked: error_is_a_file().then(|| Mutex::new(0)),
    };
    if log::set_logger(Box::leak(Box::new(logger))).is_ok() {
        log::set_max_level(level.to_level_filter());
    }
}

/// When the log's clock started, which the verbose log's times count from, so other files can give
/// times that match them.
pub fn started() -> Instant {
    *STARTED.get_or_init(Instant::now)
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
        assert!(keep_within(&mut file, 500, 27, 20, 100).unwrap());
        let text = all_of(&file);
        assert!(text.starts_with("line 000\nline 001\nline 002\n\nINFO  the log passed 500 bytes"), "{text}");
        // Up to the newest part's first whole line, of which "7\n" is the end of a line it cut.
        assert!(text.ends_with("dropped: 855 bytes\nline 098\nline 099\n"), "{text}");
        // What is written next follows the newest part.
        file.write_all(b"next\n").unwrap();
        assert!(all_of(&file).ends_with("line 099\nnext\n"));
    }

    #[test]
    fn a_log_under_its_limit_is_left_alone() {
        let mut file = log_file("small", b"one\ntwo\n");
        assert!(!keep_within(&mut file, 500, 4, 4, 4).unwrap());
        assert_eq!(all_of(&file), "one\ntwo\n");
    }

    #[test]
    fn a_log_that_cannot_be_read_back_keeps_its_start() {
        let path = std::env::temp_dir().join(format!("guesthouse-logger-{}-blind", std::process::id()));
        let mut file = File::create(&path).unwrap();
        file.write_all(&[b'x'; 600]).unwrap();
        assert!(keep_within(&mut file, 500, 100, 100, 100).unwrap());
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(text.starts_with(&"x".repeat(100)) && text.ends_with("dropped: 400 bytes\n"), "{text}");
    }

    /// A verbose log's start, a middle of debug lines with the lines worth keeping among them, and an
    /// end, as the daemon writes them.
    fn verbose_log() -> String {
        let debug = |at: u32| format!("DEBUG [{at:>9}.000] message Some(QueryAdapter) process=0x400012c0\n");
        let mut log = String::from("INFO  [    0.000] Intel card at /dev/dri/renderD128\n");
        log.extend((1..40).map(debug));
        log.push_str("DEBUG [   40.000] process 0x40077400 is guest pid 1756, msedge.exe\n");
        log.extend((41..80).map(debug));
        log.push_str("INFO  [   80.000] intel context process=0x40077400 node=1 node_name=video\n");
        log.push_str("WARN  [   81.000] intel work: queue 0x400e93c0 in space 7 stops: it would run on\n");
        log.push_str("  and a second line of the same warning\n");
        log.extend((82..160).map(debug));
        log.push_str("ERROR [  160.000] queue 0x40007600 did not reach fence value 0x2a46 in time\n");
        log.extend((161..400).map(debug));
        log
    }

    #[test]
    fn a_cut_keeps_the_middles_warnings_and_summaries_and_the_names_of_programs() {
        let log = verbose_log();
        let mut file = log_file("kept", log.as_bytes());
        assert!(keep_within(&mut file, 4_000, 200, 1_000, 1_000).unwrap());
        let text = all_of(&file);
        let (start, rest) = text.split_once("\nINFO  the log passed 4000 bytes").unwrap();
        assert!(start.starts_with("INFO  [    0.000] Intel card"));
        let (note, rest) = rest.split_once('\n').unwrap();
        assert!(
            note.ends_with("but for its 5 lines below that aren't debug lines or name a guest program"),
            "{note}"
        );
        let (kept, newest) = rest.split_once("INFO  the log's newest part follows\n").unwrap();
        assert_eq!(
            kept,
            "DEBUG [   40.000] process 0x40077400 is guest pid 1756, msedge.exe\n\
             INFO  [   80.000] intel context process=0x40077400 node=1 node_name=video\n\
             WARN  [   81.000] intel work: queue 0x400e93c0 in space 7 stops: it would run on\n  \
             and a second line of the same warning\n\
             ERROR [  160.000] queue 0x40007600 did not reach fence value 0x2a46 in time\n"
        );
        assert!(newest.starts_with("DEBUG [") && log.ends_with(newest), "{newest}");
        let dropped: usize =
            note.split("dropped: ").nth(1).unwrap().split(' ').next().unwrap().parse().unwrap();
        let notes = "\nINFO  the log passed 4000 bytes".len() + note.len() + 1;
        assert_eq!(text.len(), log.len() - dropped + notes + "INFO  the log's newest part follows\n".len());
    }

    #[test]
    fn a_full_room_keeps_the_newest_records_whole_and_says_how_much_older_went() {
        let mut file = log_file("room", verbose_log().as_bytes());
        // Room for the warning with its second line and the error, not for the context before them.
        assert!(keep_within(&mut file, 4_000, 200, 1_000, 230).unwrap());
        let text = all_of(&file);
        assert!(
            text.contains("but for its 3 lines below") && text.contains("as 141 bytes of older ones"),
            "{text}"
        );
        assert!(
            text.contains("program, the newest of them, as 141 bytes of older ones didn't fit\nWARN  "),
            "{text}"
        );
        assert!(!text.contains("msedge.exe") && !text.contains("node_name=video"), "{text}");
        // A record bigger than the room goes whole, never its first line alone.
        let mut file = log_file("small-room", verbose_log().as_bytes());
        assert!(keep_within(&mut file, 4_000, 200, 1_000, 100).unwrap());
        let text = all_of(&file);
        assert!(
            !text.contains("a second line") && text.contains("but for its 1 line below that isn't"),
            "{text}"
        );
    }

    #[test]
    fn what_a_cut_kept_is_kept_again_by_the_next() {
        let mut file = log_file("twice", verbose_log().as_bytes());
        assert!(keep_within(&mut file, 4_000, 200, 1_000, 1_000).unwrap());
        let more: String =
            (400..600).map(|at| format!("DEBUG [{at:>9}.000] wait on gpu for 0x40007600\n")).collect();
        file.write_all(more.as_bytes()).unwrap();
        assert!(keep_within(&mut file, 4_000, 200, 1_000, 1_000).unwrap());
        let text = all_of(&file);
        assert!(text.contains("msedge.exe") && text.contains("did not reach fence value"), "{text}");
        assert_eq!(text.matches("the log passed 4000 bytes").count(), 2, "{text}");
    }

    #[test]
    fn a_line_with_no_level_goes_with_the_line_before_it() {
        let mut keeper = Keeper::default();
        assert!(!keeper.keeps(b"a line before any level\n"));
        assert!(keeper.keeps(b"WARN  [    1.000] a warning\n"));
        assert!(keeper.keeps(b"  that goes on here\n"));
        assert!(!keeper.keeps(b"DEBUG [    2.000] message Some(SubmitToContext) process=0x40077400\n"));
        assert!(!keeper.keeps(b"  and so does this\n"));
        assert!(keeper.keeps(b"INFO  a line from a log with no times\n"));
    }

    #[test]
    fn only_the_debug_lines_that_name_a_guest_program_are_kept() {
        let mut keeper = Keeper::default();
        assert!(keeper.keeps(b"DEBUG [  463.771] process 0x40181cc0 is guest pid 11900, Heaven.exe\n"));
        assert!(keeper.keeps(b"DEBUG [    9.863] process 0x40001080 is guest pid 952, which gave no name\n"));
        assert!(keeper.keeps(b"DEBUG process 0x40001080 is guest pid 952, csrss.exe\n"));
        assert!(!keeper.keeps(b"DEBUG [  467.385] intel process-gone process=0x40181cc0\n"));
        assert!(
            !keeper.keeps(b"DEBUG [  314.874] allocation 0x400eec80 in process 0x400ecbc0: 0x8000 bytes\n")
        );
    }

    #[test]
    fn a_line_has_its_level_then_the_time_in_milliseconds() {
        let line = timed_line(Duration::from_millis(123_456), Level::Debug, "wait on gpu");
        assert_eq!(line, "DEBUG [  123.456] wait on gpu");
    }

    #[test]
    fn the_limits_are_64_mib_with_8_of_its_start_8_of_its_newest_and_4_of_the_middle() {
        assert_eq!(
            (MOST_BYTES, START_BYTES, NEWEST_BYTES, MIDDLE_BYTES),
            (64 << 20, 8 << 20, 8 << 20, 4 << 20)
        );
        // Its size is looked at once a mebibyte has gone by, counted at 200 bytes a line.
        assert_eq!((CHECK_EVERY, READ_BYTES), (1 << 20, 1 << 20));
    }

    #[test]
    fn a_log_is_cut_from_its_limit_on_and_only_if_more_than_its_start_and_newest_part() {
        let mut file = log_file("at-limit", &[b'x'; 500]);
        assert!(keep_within(&mut file, 500, 100, 100, 100).unwrap(), "a log at its limit is cut");
        let mut file = log_file("below-limit", &[b'x'; 499]);
        assert!(!keep_within(&mut file, 500, 100, 100, 100).unwrap());
        assert_eq!(all_of(&file).len(), 499);
        let mut file = log_file("no-middle", &[b'x'; 500]);
        assert!(!keep_within(&mut file, 500, 250, 250, 100).unwrap(), "nothing between start and newest");
        assert_eq!(all_of(&file).len(), 500);
    }

    /// A level is the line's first bytes, so a longer word that starts with one counts as it, and
    /// anything else, even a level in lower case or after a space, is no level.
    #[test]
    fn a_level_is_the_start_of_the_line() {
        for line in
            ["ERROR x", "WARN x", "INFO x", "DEBUG x", "TRACE x", "WARNING x", "INFORMATION", "ERRORS"]
        {
            assert!(starts_a_record(line.as_bytes()), "{line}");
        }
        for line in ["warn x", " WARN x", "Info x", "host: WARN x", "", "\n", "WAR"] {
            assert!(!starts_a_record(line.as_bytes()), "{line}");
        }
        let mut keeper = Keeper::default();
        assert!(keeper.keeps(b"WARNING x\n") && keeper.keeps(b"INFORMATION x\n"));
        assert!(!keeper.keeps(b"DEBUGGER x\n") && !keeper.keeps(b"TRACE x\n"));
        assert!(!keeper.keeps(b"host: a line with a prefix and no level\n"), "goes with the trace line");
        assert!(keeper.keeps(b"ERROR x\n") && keeper.keeps(b"host: a line with a prefix and no level\n"));
    }

    /// A whole log before and after a cut, byte for byte.
    #[test]
    fn a_cut_log_byte_for_byte() {
        let log = "INFO  first\n\
                   WARN  a warning the start cuts across\n\
                   host: a line with no level after it\n\
                   WARNING a word that starts with a level\n\
                   host: going on with it\n\
                   DEBUG a detail\n\
                   host: going on with the detail\n\
                   DEBUGGER a word that starts with a debug level\n\
                   TRACE a finer detail\n \
                   ERROR after a space, so going on with the trace\n\
                   error in lower case\n\
                   INFORMATION kept\n\
                   DEBUG process 0x40001080 is guest pid 7, a.exe\n\
                   DEBUG the newest part starts in this line\n\
                   INFO  two\n\
                   INFO  three\n";
        let mut file = log_file("golden", log.as_bytes());
        let newest = "INFO  two\nINFO  three\n".len() as u64 + 3;
        assert!(keep_within(&mut file, 100, 17, newest, 1_000).unwrap());
        assert_eq!(
            all_of(&file),
            "INFO  first\nWARN \n\
             INFO  the log passed 100 bytes, so its middle was dropped: 294 bytes, but for its 4 lines below \
             that aren't debug lines or name a guest program\n\
             WARNING a word that starts with a level\n\
             host: going on with it\n\
             INFORMATION kept\n\
             DEBUG process 0x40001080 is guest pid 7, a.exe\n\
             INFO  the log's newest part follows\n\
             INFO  two\n\
             INFO  three\n"
        );
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
