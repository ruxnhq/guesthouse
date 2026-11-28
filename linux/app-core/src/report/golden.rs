//! What the Linux host's report code makes today, kept byte for byte in `tests/report-golden/`, so a
//! change to how a report is scrubbed, cut, summed up, packed or linked shows as a difference here.
//! `GUESTHOUSE_WRITE_GOLDEN=1 cargo test -p guesthouse-app-core report::golden` writes the copies
//! afresh, which is only for a change meant to alter a report.

use super::before_sending;
use super::bundle::{Words, write_bundle};
use super::collect::Collection;
use super::issue_link::{failure_excerpt, issue_link};
use super::kind::ProblemKind;
use super::scrub::{Scrubber, scrub_emails, scrub_names};
use super::summary::{self, Summary};
use super::trim::trim;
use crate::testing::Scratch;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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

/// A user, a PC, a home and machines that belong to nobody.
fn scrubber() -> Scrubber {
    let all = ["w11", "quillon-games", "tv", "windows", "work-box"].map(str::to_owned);
    Scrubber::new("quillon", "marrowbox", Path::new("/home/quillon"))
        .with_machines(Some("quillon-games"), &all)
}

/// A daemon's log of `lines` debug lines with a line worth keeping every so often.
fn long_log(lines: usize) -> String {
    (0..lines)
        .map(|number| match number % 97 {
            13 => format!("{:<6}[{number:>9}] queue 0x400e93c0 stops at /home/quillon/x\n", "WARN"),
            50 => format!("{:<6}[{number:>9}] context node=1\n{:2}and its next line\n", "INFO", ""),
            _ => format!("DEBUG [{number:>9}] message Some(QueryAdapter) process=0x400012c0\n"),
        })
        .collect()
}

#[test]
fn the_scrubber_takes_out_what_it_takes_out_today() {
    let scrubber = scrubber();
    let sample = sample();
    golden("linux-scrub.txt", &scrubber.scrub(&sample));
    golden("linux-scrub-machines.txt", &scrubber.scrub_machines(&sample));
    golden("linux-scrub-emails.txt", &scrub_emails(&sample));
    let names =
        [("<guest-computer>", "DESKTOP-GUEST"), ("<guest-user>", "GuestAccount"), ("<guest-user-2>", "Pat")];
    golden("linux-scrub-names.txt", &scrub_names(&sample, &names));
    let bare = Scrubber::new("", "", Path::new(""));
    golden("linux-scrub-nothing-known.txt", &bare.scrub(&sample));
}

#[test]
fn a_log_is_cut_as_it_is_cut_today() {
    let log = long_log(2000);
    assert_eq!(trim(&log, log.len()), log, "a log that fits stays whole");
    for most in [16_000, 4_000, 1_000] {
        golden(&format!("linux-trim-{most}.txt"), &trim(&log, most));
    }
    golden("linux-trim-no-line-breaks.txt", &trim(&"é".repeat(5_000), 1_001));
}

fn summary() -> Summary {
    let mut summary = Summary::default();
    summary.set(summary::VERSION, "0.11.0");
    summary.set(summary::CARD, "NVIDIA GeForce RTX 4080");
    summary.set(summary::DRIVER, "580.95.05");
    summary.set(summary::DISTRO, "Fedora Linux 42");
    summary.set(summary::KERNEL, "6.15.4");
    summary.set(summary::SESSION, "wayland");
    summary.set(summary::MACHINE, "windows");
    summary.set(summary::KIND, "crashed");
    summary.set(summary::DESCRIPTION, "It went black. 画面が真っ黒になりました。");
    summary.set(summary::CONTACT, "someone@example.org");
    summary
}

#[test]
fn the_summary_is_written_as_it_is_today() {
    let summary = summary();
    golden("linux-summary.json", &summary.to_json());
    golden("linux-summary-header.txt", &summary.header());
    let mut long = summary.clone();
    long.set(summary::DESCRIPTION, &"画面が真っ黒になりました。".repeat(200));
    golden("linux-summary-header-long.txt", &long.header());
}

#[test]
fn the_issue_link_is_made_as_it_is_today() {
    let log = sample()
        + "10:00:13 start Starting the VM\n\
           10:00:14 app: The VM did not start: something is still running.\n\
           10:00:15 w11 stopped by itself. QEMU's last words: qemu-system-x86_64: secret\n\
           10:00:16 stop it stopped\n";
    let excerpt = failure_excerpt(&scrubber().scrub(&log));
    golden("linux-issue-excerpt.txt", &(excerpt.join("\n") + "\n"));
    golden("linux-issue-link.txt", &issue_link("0a1b2c3d4e", &summary(), "It went black.", &excerpt));
    let long_excerpt: Vec<String> =
        (0..200).map(|number| format!("10:00:00 app: line {number} {}", "x".repeat(60))).collect();
    golden(
        "linux-issue-link-long.txt",
        &issue_link("0a1b2c3d4e", &summary(), &"写".repeat(5000), &long_excerpt),
    );
}

#[test]
fn a_bundle_holds_what_it_holds_today() {
    let scratch = Scratch::new();
    let mut collection = Collection { machine: Some("quillon-games".into()), ..Collection::default() };
    collection.add("app/guesthouse.log", sample());
    collection.add("machine/run-1/daemon.log", long_log(300));
    collection.miss("host/kernel-log.txt", "this user may not read the kernel's log", Some("sudo dmesg"));
    collection.miss("guest/events.txt", "the machine isn't running", None);
    collection.notes.push("A note about quillon-games at /home/quillon.".into());
    for (key, value) in summary().facts() {
        if key != summary::DESCRIPTION && key != summary::CONTACT {
            collection.summary.set(key, &format!("{value} on marrowbox"));
        }
    }
    let words = Words {
        description: "  quillon-games went black for quillon@example.org on marrowbox  ".into(),
        contact: "quillon on the Discord".into(),
    };
    let bundle = write_bundle(&collection, &words, &scrubber(), scratch.root()).unwrap();
    let unpacked = scratch.path("unpacked");
    fs::create_dir_all(&unpacked).unwrap();
    let tar = Command::new("tar").arg("-xzf").arg(&bundle.path).arg("-C").arg(&unpacked).status();
    assert!(tar.unwrap().success());
    let top = fs::read_dir(&unpacked).unwrap().next().unwrap().unwrap().path();
    let mut all = String::new();
    for (name, bytes) in &bundle.files {
        all.push_str(&format!("== {name} ({bytes} bytes)\n"));
        all.push_str(&fs::read_to_string(top.join(name)).unwrap());
    }
    all.push_str(&format!("== header\n{}\n", bundle.summary.header()));
    golden("linux-bundle.txt", &all);
}

#[test]
fn what_is_said_before_sending_is_what_is_said_today() {
    let mut made = String::new();
    for machine in [None, Some("quillon-games".to_owned())] {
        for has_run_logs in [false, true] {
            for machine_running in [false, true] {
                for kind in ProblemKind::ALL {
                    let collection = Collection {
                        machine: machine.clone(),
                        has_run_logs,
                        machine_running,
                        ..Collection::default()
                    };
                    let said = before_sending(&collection, kind);
                    made.push_str(&format!(
                        "{} logs={has_run_logs} running={machine_running} {}: {said:?}\n",
                        machine.is_some(),
                        kind.word()
                    ));
                }
            }
        }
    }
    golden("linux-before-sending.txt", &made);
}
