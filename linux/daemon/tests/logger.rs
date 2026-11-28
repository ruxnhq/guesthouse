//! The daemon's log lines as they reach standard error. Each case starts the logger in a child process
//! of its own, as a logger can only be started once, and with standard error a pipe or a file, as a
//! file is kept under a limit and a pipe is not. They hold what the logger does today, so it can move
//! into a crate other hosts can use without anyone seeing a difference.

#[cfg(target_os = "linux")]
mod linux {
    use guesthouse_daemon::logger;
    use std::env;
    use std::fs::{self, File};
    use std::io::{Read, Seek, SeekFrom};
    use std::process::{self, Command, Output, Stdio};

    const CASE_VARIABLE: &str = "DAEMON_LOGGER_CASE";
    const LINES_VARIABLE: &str = "DAEMON_LOGGER_LINES";

    /// The size a log that is a file is kept under, and what stays of its start.
    const MOST_BYTES: u64 = 64 << 20;
    const START_BYTES: usize = 8 << 20;
    /// A size check comes once 1 MiB has gone by, at 200 bytes a line whatever its length.
    const LINES_TO_A_CHECK: usize = 5_243;

    /// What the child does: start the logger, then write lines.
    fn child(case: &str) {
        match case {
            "plain" => {
                logger::start(false);
                every_level();
            }
            "verbose" => {
                logger::start(true);
                every_level();
            }
            "twice" => {
                logger::start(false);
                logger::start(true);
                log::debug!("a debug line");
                log::info!("an info line");
            }
            "lines" => {
                logger::start(false);
                let lines: usize = env::var(LINES_VARIABLE).unwrap().parse().unwrap();
                for line in 0..lines {
                    log::info!("line {line:05}");
                }
            }
            _ => panic!("no case {case}"),
        }
    }

    fn every_level() {
        log::error!("an error");
        log::warn!("a warning");
        log::info!("some news");
        log::info!("a message of\ntwo lines");
        log::debug!("a detail");
        log::trace!("a finer detail");
    }

    fn run(case: &str, lines: usize, stderr: Stdio) -> Output {
        let output = Command::new(env::current_exe().unwrap())
            .env_clear()
            .env(CASE_VARIABLE, case)
            .env(LINES_VARIABLE, lines.to_string())
            .stderr(stderr)
            .output()
            .unwrap();
        assert!(output.status.success(), "{case}: {}", String::from_utf8_lossy(&output.stderr));
        output
    }

    fn piped(case: &str) -> String {
        String::from_utf8(run(case, 0, Stdio::piped()).stderr).unwrap()
    }

    /// Each line is its level padded to five, a space and the message, with nothing after but the
    /// line break, and a message's own line breaks are kept as they are. Debug and trace lines are off.
    fn a_plain_line_is_its_level_and_its_message() {
        assert_eq!(
            piped("plain"),
            "ERROR an error\nWARN  a warning\nINFO  some news\nINFO  a message of\ntwo lines\n"
        );
    }

    /// The verbose log adds debug lines, never trace ones, and the time after the level: masked here,
    /// but for its width and its three decimals.
    fn a_verbose_line_has_the_time_after_its_level() {
        let text = piped("verbose");
        let mut masked = String::new();
        for line in text.lines() {
            match line.get(6..18) {
                Some(time) if time.starts_with('[') && time.ends_with("] ") => {
                    let seconds = time[1..10].trim_start();
                    assert!(seconds.len() >= 5 && seconds.parse::<f64>().is_ok(), "{line}");
                    assert_eq!(seconds.split_once('.').unwrap().1.len(), 3, "{line}");
                    masked.push_str(&format!("{}[    T.TTT] {}\n", &line[..6], &line[18..]));
                }
                _ => masked.push_str(&format!("{line}\n")),
            }
        }
        assert_eq!(
            masked,
            "ERROR [    T.TTT] an error\nWARN  [    T.TTT] a warning\nINFO  [    T.TTT] some news\n\
             INFO  [    T.TTT] a message of\ntwo lines\nDEBUG [    T.TTT] a detail\n"
        );
    }

    /// The first start holds: a second one changes neither the level nor the times.
    fn a_second_start_changes_nothing() {
        assert_eq!(piped("twice"), "INFO  an info line\n");
    }

    fn line(number: usize) -> String {
        format!("INFO  line {number:05}\n")
    }

    /// A pipe is never looked at, so every line goes through, however many.
    fn a_pipe_takes_every_line() {
        let output = run("lines", LINES_TO_A_CHECK, Stdio::piped());
        let expected: String = (0..LINES_TO_A_CHECK).map(line).collect();
        assert!(output.stderr == expected.as_bytes(), "{} bytes", output.stderr.len());
    }

    /// A log file that is already at its limit, made sparse so that it costs nothing, and open at its
    /// end as the script that starts the daemon leaves it.
    fn full_log(name: &str) -> (File, std::path::PathBuf) {
        let path = env::temp_dir().join(format!("guesthouse-daemon-logger-{}-{name}", process::id()));
        let mut file =
            File::options().read(true).write(true).create(true).truncate(true).open(&path).unwrap();
        file.set_len(MOST_BYTES).unwrap();
        file.seek(SeekFrom::End(0)).unwrap();
        (file, path)
    }

    fn written(path: &std::path::Path) -> Vec<u8> {
        let mut bytes = Vec::new();
        File::open(path).unwrap().read_to_end(&mut bytes).unwrap();
        let _ = fs::remove_file(path);
        bytes
    }

    /// The size isn't looked at until 1 MiB of lines has gone by, counted at 200 bytes a line: one
    /// line short of that, a file past its limit is left as it is.
    fn a_file_is_not_looked_at_before_its_check() {
        let (file, path) = full_log("before");
        run("lines", LINES_TO_A_CHECK - 1, Stdio::from(file));
        let bytes = written(&path);
        let lines: String = (0..LINES_TO_A_CHECK - 1).map(line).collect();
        assert_eq!(bytes.len() as u64, MOST_BYTES + lines.len() as u64);
        assert!(bytes[MOST_BYTES as usize..] == *lines.as_bytes());
    }

    /// At the check, a file past its limit keeps its first 8 MiB and its newest part from its first
    /// whole line, with a line between that says how much went. Here the newest part's first line
    /// is the first one written, which began in the middle, so it goes with the middle.
    fn a_file_past_its_limit_is_cut_at_its_check() {
        let (file, path) = full_log("cut");
        run("lines", LINES_TO_A_CHECK, Stdio::from(file));
        let bytes = written(&path);
        assert!(bytes[..START_BYTES].iter().all(|&byte| byte == 0));
        let rest = std::str::from_utf8(&bytes[START_BYTES..]).unwrap();
        let mut expected = String::from(
            "\nINFO  the log passed 67108864 bytes, so its middle was dropped: 58720273 bytes\n",
        );
        expected.extend((1..LINES_TO_A_CHECK).map(line));
        assert_eq!(rest, expected);
    }

    const CASES: &[(&str, fn())] = &[
        ("a_plain_line_is_its_level_and_its_message", a_plain_line_is_its_level_and_its_message),
        ("a_verbose_line_has_the_time_after_its_level", a_verbose_line_has_the_time_after_its_level),
        ("a_second_start_changes_nothing", a_second_start_changes_nothing),
        ("a_pipe_takes_every_line", a_pipe_takes_every_line),
        ("a_file_is_not_looked_at_before_its_check", a_file_is_not_looked_at_before_its_check),
        ("a_file_past_its_limit_is_cut_at_its_check", a_file_past_its_limit_is_cut_at_its_check),
    ];

    pub fn main() {
        if let Ok(case) = env::var(CASE_VARIABLE) {
            child(&case);
            return;
        }
        let mut failed = Vec::new();
        for (name, test) in CASES {
            let passed = std::panic::catch_unwind(test).is_ok();
            println!("{name} ... {}", if passed { "ok" } else { "FAILED" });
            if !passed {
                failed.push(name);
            }
        }
        assert!(failed.is_empty(), "failed: {failed:?}");
    }
}

#[cfg(target_os = "linux")]
use linux::main;

#[cfg(not(target_os = "linux"))]
fn main() {
    println!("the logger is Linux's, so this does nothing here.");
}
