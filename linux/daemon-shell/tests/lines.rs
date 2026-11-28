//! The lines the shell writes as they reach standard error, in each style, and the line `fail` stops
//! with. Each case runs in a child process of its own, as a logger can only be started once and `fail`
//! is seen by its exit code.

use std::env;
use std::process::{Command, ExitCode, Output, Stdio};

const CASE_VARIABLE: &str = "DAEMON_SHELL_CASE";
const LINES_VARIABLE: &str = "DAEMON_SHELL_LINES";

/// The child for `case`, run to its end with only the variables given.
fn run(case: &str, lines: usize, stderr: Stdio) -> Output {
    let mut command = Command::new(env::current_exe().unwrap());
    command.env_clear().env(CASE_VARIABLE, case).env(LINES_VARIABLE, lines.to_string()).stderr(stderr);
    // Windows can't start a program with no system root.
    if let Some(root) = env::var_os("SystemRoot") {
        command.env("SystemRoot", root);
    }
    command.output().unwrap()
}

/// What a child that fails gives back: its line and its exit code.
fn failed(case: &str) -> (String, Option<i32>) {
    let output = run(case, 0, Stdio::piped());
    assert!(output.stdout.is_empty(), "{case}");
    (String::from_utf8(output.stderr).unwrap(), output.status.code())
}

/// `fail` writes the prefix and the error as one bare line and gives exit code 1.
fn fail_is_the_prefix_and_the_error_and_exit_1() {
    assert_eq!(failed("fail"), ("x: QEMU has gone\n".to_owned(), Some(1)));
}

/// With no prefix it is the error alone, as the Linux daemon's refusals are.
fn fail_with_no_prefix_is_the_error_alone() {
    assert_eq!(
        failed("fail-bare"),
        ("GUESTHOUSE_SLOT must be a number below 4, not \"x\"\n".to_owned(), Some(1))
    );
}

/// What the child does.
fn child(case: &str) -> ExitCode {
    match case {
        "fail" => guesthouse_daemon_shell::fail("x: ", "QEMU has gone"),
        "fail-bare" => {
            guesthouse_daemon_shell::fail("", "GUESTHOUSE_SLOT must be a number below 4, not \"x\"")
        }
        #[cfg(unix)]
        other => {
            unix::child(other);
            ExitCode::SUCCESS
        }
        #[cfg(not(unix))]
        other => panic!("no case {other}"),
    }
}

#[cfg(unix)]
mod unix {
    use super::{LINES_VARIABLE, run};
    use guesthouse_daemon_shell::logger::{self, Style};
    use std::env;
    use std::fs::{self, File};
    use std::io::{Read, Seek, SeekFrom};
    use std::process::{self, Stdio};

    const PREFIX: &str = "x: ";
    /// The size a levelled log that is a file is kept under, and what stays of its start.
    const MOST_BYTES: u64 = 64 << 20;
    /// A size check comes once 1 MiB has gone by, at 200 bytes a line whatever its length.
    const LINES_TO_A_CHECK: usize = 5_243;

    fn piped(case: &str) -> String {
        let output = run(case, 0, Stdio::piped());
        assert!(output.status.success(), "{case}: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stderr).unwrap()
    }

    pub fn child(case: &str) {
        match case {
            "one" => {
                logger::start_styled(Style::Prefixed(PREFIX), false);
                log::info!("QEMU has gone");
            }
            "two-lines" => {
                logger::start_styled(Style::Prefixed(PREFIX), false);
                log::warn!("a message of\ntwo lines");
                log::info!("and one after");
            }
            "every-level" => {
                logger::start_styled(Style::Prefixed(PREFIX), false);
                every_level();
            }
            "every-level-verbose" => {
                logger::start_styled(Style::Prefixed(PREFIX), true);
                every_level();
            }
            "levelled" => {
                logger::start_styled(Style::Levelled, false);
                every_level();
            }
            "levelled-verbose" => {
                logger::start_styled(Style::Levelled, true);
                every_level();
            }
            "prefixed-then-levelled" => {
                logger::start_styled(Style::Prefixed(PREFIX), false);
                logger::start(true);
                log::debug!("a detail");
                log::info!("some news");
            }
            "levelled-then-prefixed" => {
                logger::start(false);
                logger::start_styled(Style::Prefixed(PREFIX), true);
                log::debug!("a detail");
                log::info!("some news");
            }
            "lines" => {
                logger::start_styled(Style::Prefixed(PREFIX), false);
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
        log::debug!("a detail");
        log::trace!("a finer detail");
    }

    /// The line another host's process prints when QEMU goes, byte for byte.
    fn a_prefixed_line_is_the_prefix_and_the_message() {
        assert_eq!(piped("one"), "x: QEMU has gone\n");
    }

    /// A message's own line breaks pass through, so its later lines have no prefix.
    fn a_prefixed_message_of_two_lines_has_the_prefix_once() {
        assert_eq!(piped("two-lines"), "x: a message of\ntwo lines\nx: and one after\n");
    }

    /// Every level looks the same, with no level word and no time; debug lines are written only when
    /// verbose, and trace lines never, as in the levelled style.
    fn every_level_has_the_prefix_and_no_level_word() {
        assert_eq!(piped("every-level"), "x: an error\nx: a warning\nx: some news\n");
        assert_eq!(piped("every-level-verbose"), "x: an error\nx: a warning\nx: some news\nx: a detail\n");
    }

    /// The levelled style chosen by name is `start`'s, as the daemon's own tests pin it.
    fn the_levelled_style_is_what_start_gives() {
        assert_eq!(piped("levelled"), "ERROR an error\nWARN  a warning\nINFO  some news\n");
        let verbose = piped("levelled-verbose");
        let masked: Vec<String> = verbose
            .lines()
            .map(|line| {
                let (level, rest) = line.split_at(6);
                let (time, message) = rest.split_at(12);
                assert!(time.starts_with('[') && time.ends_with("] "), "{line}");
                format!("{level}[    T.TTT] {message}")
            })
            .collect();
        assert_eq!(
            masked,
            [
                "ERROR [    T.TTT] an error",
                "WARN  [    T.TTT] a warning",
                "INFO  [    T.TTT] some news",
                "DEBUG [    T.TTT] a detail"
            ]
        );
    }

    /// The first start holds, whichever style it was, and a later one changes nothing.
    fn the_first_start_holds_in_either_style() {
        assert_eq!(piped("prefixed-then-levelled"), "x: some news\n");
        assert_eq!(piped("levelled-then-prefixed"), "INFO  some news\n");
    }

    fn line(number: usize) -> String {
        format!("x: line {number:05}\n")
    }

    /// A log file already at the levelled style's limit, sparse so it costs nothing, and open at its end.
    fn full_log() -> (File, std::path::PathBuf) {
        let path = env::temp_dir().join(format!("guesthouse-daemon-shell-{}-full", process::id()));
        let mut file =
            File::options().read(true).write(true).create(true).truncate(true).open(&path).unwrap();
        file.set_len(MOST_BYTES).unwrap();
        file.seek(SeekFrom::End(0)).unwrap();
        (file, path)
    }

    /// A prefixed log that is a file is never cut: past the levelled style's limit and three of its
    /// checks, every byte before and every line after are still there.
    fn a_prefixed_file_is_never_cut() {
        let lines = 3 * LINES_TO_A_CHECK;
        let (file, path) = full_log();
        let output = run("lines", lines, Stdio::from(file));
        assert!(output.status.success());
        let mut bytes = Vec::new();
        File::open(&path).unwrap().read_to_end(&mut bytes).unwrap();
        let _ = fs::remove_file(&path);
        let expected: String = (0..lines).map(line).collect();
        assert_eq!(bytes.len() as u64, MOST_BYTES + expected.len() as u64);
        assert!(bytes[..MOST_BYTES as usize].iter().all(|&byte| byte == 0));
        assert!(bytes[MOST_BYTES as usize..] == *expected.as_bytes());
    }

    pub const CASES: &[(&str, fn())] = &[
        ("a_prefixed_line_is_the_prefix_and_the_message", a_prefixed_line_is_the_prefix_and_the_message),
        (
            "a_prefixed_message_of_two_lines_has_the_prefix_once",
            a_prefixed_message_of_two_lines_has_the_prefix_once,
        ),
        ("every_level_has_the_prefix_and_no_level_word", every_level_has_the_prefix_and_no_level_word),
        ("the_levelled_style_is_what_start_gives", the_levelled_style_is_what_start_gives),
        ("the_first_start_holds_in_either_style", the_first_start_holds_in_either_style),
        ("a_prefixed_file_is_never_cut", a_prefixed_file_is_never_cut),
    ];
}

const CASES: &[(&str, fn())] = &[
    ("fail_is_the_prefix_and_the_error_and_exit_1", fail_is_the_prefix_and_the_error_and_exit_1),
    ("fail_with_no_prefix_is_the_error_alone", fail_with_no_prefix_is_the_error_alone),
];

fn main() -> ExitCode {
    if let Ok(case) = env::var(CASE_VARIABLE) {
        return child(&case);
    }
    #[cfg(unix)]
    let cases = CASES.iter().chain(unix::CASES);
    #[cfg(not(unix))]
    let cases = CASES.iter();
    let mut failed = Vec::new();
    for (name, test) in cases {
        let passed = std::panic::catch_unwind(test).is_ok();
        println!("{name} ... {}", if passed { "ok" } else { "FAILED" });
        if !passed {
            failed.push(name);
        }
    }
    assert!(failed.is_empty(), "failed: {failed:?}");
    ExitCode::SUCCESS
}
