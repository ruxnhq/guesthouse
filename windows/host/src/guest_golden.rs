//! What the Windows host's answer-file code makes today, kept byte for byte in
//! `tests/answers-golden/`, so a change to a Windows guest's `unattend.xml`, the XML escape, or what
//! an Ubuntu guest's files disk is given shows as a difference here.
//! `GUESTHOUSE_WRITE_GOLDEN=1 cargo test -p guesthouse-host-windows guest::answers_golden` writes the copies
//! afresh, which is only for a change meant to alter what a guest is given.
//!
//! Every account and password here is made up.

use super::linux::{FILES, account, unix_text};
use super::{answer_file, escape};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

const WRITE: &str = "GUESTHOUSE_WRITE_GOLDEN";

fn folder() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/answers-golden")
}

/// Compares `made` with its kept copy byte for byte, or writes the copy when asked to. A difference
/// is named by its file and line, and the text isn't printed.
fn golden(name: &str, made: &str) {
    let path = folder().join(name);
    if std::env::var_os(WRITE).is_some() {
        fs::create_dir_all(folder()).unwrap();
        fs::write(&path, made).unwrap();
        return;
    }
    let kept = fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    if kept != made.as_bytes() {
        let kept = String::from_utf8_lossy(&kept);
        let line = kept.split('\n').zip(made.split('\n')).position(|(a, b)| a != b);
        let line = line.map_or_else(|| "its length".to_owned(), |at| format!("line {}", at + 1));
        panic!("{name} is no longer what the answer-file code made: it differs at {line}");
    }
}

fn longest(count: usize, seed: &str) -> String {
    seed.chars().cycle().take(count).collect()
}

/// The host makes each password as 32 hex digits, so the longest is that; the rest are made up to
/// show what the escape does.
fn cases() -> Vec<(&'static str, String, String)> {
    let case = |name, user: &str, password: &str| (name, user.to_owned(), password.to_owned());
    vec![
        case("plain", "guesthouse", "0123456789abcdef0123456789abcdef"),
        case("xml-specials", "a&b<c>\"d'e", "p&ss<w>\"o'rd"),
        case("spaces", "Ana B", "fake pass word"),
        case("non-ascii", "Zoë Ångström", "pässwörd-ñ-日本"),
        case("longest", &longest(20, "LongestName"), &longest(127, "fake-longest-password-")),
        case("marks-in-values", "{user}", "{password} @PASSWORD@"),
        case("empty-account", "", ""),
        // Capitals, dots, dashes, underscores and spaces between words, which the shared account rules
        // let the Windows host take.
        case("common-rule-name", "Ana.Ber-x_y Z", "fedcba9876543210fedcba9876543210"),
    ]
}

#[test]
fn a_windows_guests_answer_file_is_as_it_was() {
    for (number, (name, user, password)) in cases().iter().enumerate() {
        golden(&format!("windows-windows-{:02}-{name}.xml", number + 1), &answer_file(user, password));
    }
}

#[test]
fn the_xml_escape_is_as_it_was() {
    let inputs = [
        "",
        "plain",
        "&",
        "<",
        ">",
        "\"",
        "'",
        "a&b<c>\"d'e",
        "&amp;",
        "&lt;&gt;",
        "Zoë Ångström 日本",
        "tab\there",
        "line\nbreak",
        "@USER@ ]]> <!-- -->",
    ];
    let mut made = String::new();
    for input in inputs {
        writeln!(made, "{input:?} -> {:?}", escape(input)).unwrap();
    }
    golden("windows-xml-escape.txt", &made);
}

/// What an Ubuntu guest's files disk is given for its install: the installer's answers as they are,
/// and the account line `install.sh` reads.
#[test]
fn an_ubuntu_guests_answers_are_as_they_were() {
    for name in ["user-data", "meta-data"] {
        let text = FILES.iter().find(|(path, _)| *path == name).unwrap().1;
        golden(&format!("windows-ubuntu-{name}"), &unix_text(text));
    }
    let mut made = String::new();
    for (_, user, password) in cases() {
        writeln!(made, "{:?}", account(&user, &password)).unwrap();
    }
    golden("windows-ubuntu-account.txt", &made);
}
