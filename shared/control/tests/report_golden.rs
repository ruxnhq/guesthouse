//! What the Windows host's report code makes today, kept byte for byte in `tests/report-golden/`, so a
//! change to how a report is scrubbed, cut, summed up, staged, sent or linked shows as a difference
//! here. Everything goes through the paths the Windows app and service use.
//! `GUESTHOUSE_WRITE_GOLDEN=1 cargo test -p guesthouse-control --test report_golden` writes the
//! copies afresh, which is only for a change meant to alter a report.

use guesthouse_control::System;
use guesthouse_control::report::stage::{self, FIRST_LIMIT, Facts};
use guesthouse_control::report::{
    Parts, ProblemKind, Scrubber, Summary, clock, issue_link, keep_end, manifest, read_answer, send, summary,
};
use std::fs;
use std::path::{Path, PathBuf};

const WRITE: &str = "GUESTHOUSE_WRITE_GOLDEN";

fn folder() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/report-golden")
}

fn sample() -> String {
    fs::read_to_string(folder().join("sample.txt")).unwrap()
}

/// Compares `made` with its kept copy, or writes the copy when asked to.
fn golden(name: &str, made: &str) {
    let path = folder().join(name);
    if std::env::var_os(WRITE).is_some() {
        fs::write(&path, made).unwrap();
        return;
    }
    let kept = fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    assert!(made == kept, "{name} is no longer what the report code made:\n{made}");
}

/// A user, a PC, a profile and machines that belong to nobody, the one the report is about first.
fn scrubber() -> Scrubber {
    let machines = ["quillon-games", "w11", "tv", "windows", "work-box"].map(str::to_owned);
    Scrubber::new("quillon", r"C:\Users\quillon", "marrowbox", &machines)
}

/// A log of `lines` lines with a longer one every so often.
fn long_log(lines: usize) -> String {
    (0..lines)
        .map(|number| match number % 97 {
            13 => format!("WARN  [{number:>9}] queue 0x400e93c0 stops at C:\\Users\\quillon\\x\n"),
            _ => format!("DEBUG [{number:>9}] message Some(QueryAdapter) process=0x400012c0\n"),
        })
        .collect()
}

#[test]
fn the_scrubber_takes_out_what_it_takes_out_today() {
    golden("windows-scrub.txt", &scrubber().scrub(&sample()));
    let home = Scrubber::new("quillon", "/home/quillon", "marrowbox", &[]);
    golden("windows-scrub-unix-home.txt", &home.scrub(&sample()));
    let none = Scrubber::new("", "", "", &[String::new()]);
    golden("windows-scrub-nothing-known.txt", &none.scrub(&sample()));
}

#[test]
fn a_log_keeps_its_end_as_it_does_today() {
    let log = long_log(2000);
    assert_eq!(keep_end(&log, log.len()), log, "a log that fits stays whole");
    for most in [16_000, 4_000, 1_000] {
        golden(&format!("windows-keep-end-{most}.txt"), &keep_end(&log, most));
    }
    golden("windows-keep-end-no-line-breaks.txt", &keep_end(&"é".repeat(5_000), 1_001));
}

fn summary() -> Summary {
    let mut summary = Summary::default();
    summary.set(summary::VERSION, "0.13.0");
    summary.set(summary::HOST, summary::HOST_WINDOWS);
    summary.set(summary::OS, "Windows 11 Pro 24H2 build 26100");
    summary.set(summary::CARD, "NVIDIA GeForce RTX 4080");
    summary.set(summary::DRIVER, "32.0.15.8097");
    summary.set(summary::MACHINE, "windows");
    summary.set(summary::KIND, "crashed");
    summary.set(summary::DESCRIPTION, "It went black. 画面が真っ黒になりました。");
    summary.set(summary::CONTACT, "someone@example.org");
    summary
}

#[test]
fn the_summary_is_written_as_it_is_today() {
    let summary = summary();
    golden("windows-summary.json", &summary.to_json());
    golden("windows-summary-header.txt", &summary.header());
    let mut long = summary.clone();
    long.set(summary::DESCRIPTION, &"画面が真っ黒になりました。".repeat(200));
    golden("windows-summary-header-long.txt", &long.header());
}

#[test]
fn the_issue_link_is_made_as_it_is_today() {
    golden("windows-issue-link.txt", &issue_link("0a1b2c3d4e", &summary()));
    let mut long = summary();
    long.set(summary::CARD, &"G".repeat(2000));
    long.set(summary::OS, &"W".repeat(2000));
    long.set(summary::VERSION, &"V".repeat(2000));
    golden("windows-issue-link-long.txt", &issue_link("0a1b2c3d4e", &long));
}

#[test]
fn a_report_is_sent_and_answered_as_it_is_today() {
    let mut made = String::new();
    for address in ["https://files.ruxn.com/r/guesthouse", "http://127.0.0.1:8080/r/guesthouse"] {
        for header in [summary().header(), String::new()] {
            let args = send::curl_arguments(
                address,
                &header,
                Path::new("guesthouse-report-20260101-000000.tar.gz"),
                Path::new("guesthouse-report-20260101-000000.answer"),
            );
            for arg in args {
                made.push_str(&format!("{}\n", arg.to_string_lossy()));
            }
            made.push_str("--\n");
        }
    }
    golden("windows-curl-arguments.txt", &made);
    let mut answers = String::new();
    for (status, body) in [
        (201, r#"{"id":"0a1b2c3d4e"}"#),
        (201, r#"{"id":"../../etc"}"#),
        (201, "not json"),
        (200, r#"{"id":"0a1b2c3d4e"}"#),
        (429, r#"{"error":"Today's reports are full. Try again tomorrow."}"#),
        (502, r#"{"error":"  "}"#),
        (502, "<html>bad gateway</html>"),
    ] {
        answers.push_str(&format!("{status} {body} => {:?}\n", read_answer(status, body)));
    }
    for (stdout, stderr, succeeded, body) in [
        ("201", "", true, r#"{"id":"0a1b2c3d4e"}"#),
        ("000", "curl: (6) Could not resolve host: files.ruxn.com\n", false, ""),
        ("", "", true, ""),
        ("429", "", true, r#"{"error":"Today's reports are full."}"#),
    ] {
        let reply = send::read_reply(stdout, stderr, succeeded, body);
        answers.push_str(&format!("{stdout:?} {stderr:?} {succeeded} {body} => {reply:?}\n"));
    }
    golden("windows-answers.txt", &answers);
}

#[test]
fn a_report_is_staged_as_it_is_today() {
    let mut parts = Parts::default();
    parts.piece("service.log", &sample());
    parts.piece("windows.txt", "ProductName: Windows 11 Pro\nDisplayVersion: 24H2\nCurrentBuild: 26100\n");
    parts.piece("quillon-games.guesthouse.json", r#"{"name": "quillon-games"}"#);
    parts.piece("snapshots/index.json", "{}");
    parts.piece("summary.json", "fake");
    parts.piece("../../etc/passwd", "x");
    parts.piece("a b.txt", "1");
    parts.piece("a_b.txt", "2");
    parts.piece("long.log", &long_log(300));
    parts.miss("crashes.txt", "wevtutil failed for quillon on marrowbox");
    let facts = Facts {
        version: "0.13.0",
        windows: None,
        card: Some("NVIDIA GeForce RTX 4080"),
        machine: Some(System::Windows),
        kind: ProblemKind::Crashed,
        description: "  quillon-games went black for quillon@example.org on marrowbox  ",
        contact: "quillon on the Discord",
    };
    for limit in [FIRST_LIMIT, 4_000] {
        let staged = stage::stage(&parts, &scrubber(), &facts, limit);
        let mut all = String::new();
        for (name, text) in &staged.files {
            all.push_str(&format!("== {name} ({} bytes)\n{text}", text.len()));
        }
        for (name, reason) in &staged.missing {
            all.push_str(&format!("== missing {name}: {reason}\n"));
        }
        all.push_str(&format!("== header\n{}\n", staged.summary.header()));
        golden(&format!("windows-staged-{limit}.txt"), &all);
    }
    let files = [("service.log".to_owned(), 1200), ("summary.json".to_owned(), 80)];
    let missing = [("crash dumps".to_owned(), "there were none".to_owned())];
    golden("windows-manifest.txt", &manifest(&files, &missing));
}

#[test]
fn times_are_written_as_they_are_today() {
    let mut made = String::new();
    for seconds in [0, 951_782_400 + 86_399, 1_767_225_600 + 3661, 4_102_444_799] {
        made.push_str(&format!("{seconds} {} {}\n", clock::utc(seconds), clock::stamp(seconds)));
    }
    golden("windows-clock.txt", &made);
}
