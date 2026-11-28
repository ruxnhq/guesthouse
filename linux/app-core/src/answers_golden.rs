//! What the Linux host's answer-file code makes today, kept byte for byte in `tests/answers-golden/`,
//! so a change to a Windows guest's `autounattend.xml`, an Ubuntu guest's `user-data`, the XML escape,
//! the edition table, the zone table or lean Windows' step shows as a difference here.
//! `GUESTHOUSE_WRITE_GOLDEN=1 cargo test -p guesthouse-app-core answers_golden` writes the copies
//! afresh, which is only for a change meant to alter what a guest is given.
//!
//! Every account and password here is made up. The answer files are filled from the real templates
//! in `guest/` and `linux/setup/seed/`, as the program folder has them.

use crate::account::{LINUX_USER, WINDOWS_USER, xml_text};
use crate::actions::answer_file;
use crate::iso::{WINDOWS_EDITION, WINDOWS_EDITIONS, edition_key};
use crate::lean::with_specialize_step;
use crate::linux::seed::fill_template;
use crate::testing::{Scratch, scratch_context};
use crate::zones::windows_zone;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

const WRITE: &str = "GUESTHOUSE_WRITE_GOLDEN";

fn folder() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/answers-golden")
}

fn repository_file(path: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(path)).unwrap()
}

/// Compares `made` with its kept copy byte for byte, or writes the copy when asked to. A difference
/// is named by its file and line, and the text isn't printed.
pub(crate) fn golden(name: &str, made: &str) {
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

/// The longest name and password the Linux host takes, made of letters anyone can read.
fn longest(count: usize, seed: &str) -> String {
    seed.chars().cycle().take(count).collect()
}

/// A Windows guest's answer file on the Linux host: user, password, edition, the host's IANA zone,
/// and whether it's lean.
struct WindowsCase {
    name: &'static str,
    user: String,
    password: String,
    edition: Option<&'static str>,
    zone: &'static str,
    lean: bool,
}

fn windows_cases() -> Vec<WindowsCase> {
    let case = |name, user: &str, password: &str, edition, zone, lean| WindowsCase {
        name,
        user: user.to_owned(),
        password: password.to_owned(),
        edition,
        zone,
        lean,
    };
    vec![
        case("default-account", WINDOWS_USER, "fake-password-1", None, "Etc/UTC", false),
        case("default-account-lean", WINDOWS_USER, "fake-password-1", None, "Etc/UTC", true),
        case("pro", "ana", "fake-password-2", Some(WINDOWS_EDITION), "Europe/Bucharest", false),
        case(
            "iot-ltsc",
            "ana",
            "fake-password-3",
            Some("Windows 11 IoT Enterprise LTSC"),
            "Asia/Tokyo",
            false,
        ),
        case(
            "iot-ltsc-2024-lean",
            "ana",
            "fake-password-4",
            Some("Windows 11 IoT Enterprise LTSC 2024"),
            "Pacific/Tongatapu",
            true,
        ),
        case(
            "edition-without-key",
            "ana",
            "fake-password-5",
            Some("Windows 11 Home"),
            "America/Los_Angeles",
            false,
        ),
        case("edition-with-xml", "ana", "fake-password-6", Some("Pro & <\"N\"> 'x'"), "Europe/Berlin", false),
        case("xml-specials", "a&b<c>\"d'e", "p&ss<w>\"o'rd", None, "Etc/UTC", false),
        case("xml-specials-lean", "a&b<c>\"d'e", "p&ss<w>\"o'rd", None, "Etc/UTC", true),
        case("spaces", "Ana B", "fake pass word", None, "Etc/UTC", false),
        case("non-ascii", "Zoë Ångström", "pässwörd-ñ-日本", None, "Etc/UTC", false),
        case(
            "longest",
            &longest(20, "LongestName"),
            &longest(127, "fake-longest-password-"),
            None,
            "Etc/UTC",
            false,
        ),
        case("marks-in-values", "@PASSWORD@", "@USER@ @EDITION@ @PRODUCT_KEY@", None, "Etc/UTC", false),
        case("empty-account", "", "", None, "Etc/UTC", false),
        case("unknown-zone", "ana", "fake-password-7", None, "Mars/Olympus_Mons", false),
        case("empty-zone", "ana", "fake-password-8", None, "", false),
        case("zone-with-xml", "ana", "fake-password-9", None, "<TimeZone>", false),
    ]
}

/// What `fill_answer_disc` writes as `autounattend.xml` for this case.
fn windows_answers(case: &WindowsCase) -> String {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    scratch.write("program/guest/autounattend.xml", repository_file("guest/windows/autounattend.xml"));
    let answers = answer_file(&context, case.zone, &case.user, &case.password, case.edition).unwrap();
    if case.lean { with_specialize_step(&answers).unwrap() } else { answers }
}

#[test]
fn a_windows_guests_answer_file_is_as_it_was() {
    let cases = windows_cases();
    assert_eq!(cases.len(), 17);
    for (number, case) in cases.iter().enumerate() {
        golden(&format!("linux-windows-{:02}-{}.xml", number + 1, case.name), &windows_answers(case));
    }
}

#[test]
fn lean_windows_refuses_an_answer_file_with_no_place_for_its_step_as_it_did() {
    for answers in ["<unattend/>", "      </RunSynchronous>\n      </RunSynchronous>\n"] {
        let refused = with_specialize_step(answers).unwrap_err();
        assert_eq!(
            refused.0,
            "The answer file has no place for lean Windows' step, so nothing was installed."
        );
    }
}

/// An Ubuntu guest's `user-data` on the Linux host: user, password hash, SSH key and the kernel the
/// ISO holds. These go in as they are, with no escape.
#[test]
fn an_ubuntu_guests_user_data_is_as_it_was() {
    let template = repository_file("guest/linux/setup/seed/user-data.template");
    let hash = "$6$fakesalt$fakehashfakehashfakehash";
    let key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKEFAKE nvfwd";
    let kernel = "linux-image-7.0.0-38-generic";
    let cases: [(&str, String, &str, &str, &str); 9] = [
        ("default-account", LINUX_USER.to_owned(), hash, key, kernel),
        ("chosen-account", "ana".to_owned(), hash, key, kernel),
        ("xml-and-yaml-specials", "a&b<c>\"d'e: #x".to_owned(), hash, key, kernel),
        ("spaces", "ana b".to_owned(), hash, key, kernel),
        ("non-ascii", "zoë-ångström".to_owned(), hash, key, kernel),
        ("longest", longest(32, "longest_name"), hash, key, kernel),
        ("marks-in-values", "@SSH_KEY@".to_owned(), "@USER@", "@KERNEL_PACKAGE@", "@PASSWORD_HASH@"),
        ("empty-account", String::new(), "", "", ""),
        ("other-kernel", "ana".to_owned(), hash, key, "linux-image-6.17.0-5-generic"),
    ];
    for (number, (name, user, hash, key, kernel)) in cases.iter().enumerate() {
        // As `make_seed` writes it.
        let user_data = fill_template(&template, user, hash, key, kernel) + "\n";
        golden(&format!("linux-ubuntu-{:02}-{name}.yaml", number + 1), &user_data);
    }
    golden("linux-ubuntu-meta-data", &repository_file("guest/linux/setup/seed/meta-data"));
}

/// Characters XML reads, and some it doesn't, through the Linux host's escape.
pub(crate) const ESCAPE_INPUTS: [&str; 14] = [
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

#[test]
fn the_xml_escape_is_as_it_was() {
    let mut made = String::new();
    for input in ESCAPE_INPUTS {
        writeln!(made, "{input:?} -> {:?}", xml_text(input)).unwrap();
    }
    golden("linux-xml-text.txt", &made);
}

#[test]
fn the_edition_table_is_as_it_was() {
    let mut made = format!("default: {WINDOWS_EDITION}\n");
    for (name, key) in WINDOWS_EDITIONS {
        writeln!(made, "{name}\t{key}").unwrap();
    }
    for edition in [
        "Windows 11 Pro",
        "windows 11 pro",
        "Windows 11 Pro ",
        "Windows 11 Pro N",
        "Windows 11 Pro 2024",
        "Windows 11 Home",
        "Windows 11 IoT Enterprise LTSC",
        "Windows 11 IoT Enterprise LTSC 2024",
        "Windows 11 IoT Enterprise LTSC 24",
        "Windows 11 IoT Enterprise LTSC 20245",
        "Windows 11 IoT Enterprise LTSC x024",
        "Windows 11 IoT Enterprise LTSC\t2024",
        "Windows 11 Enterprise LTSC",
        "",
    ] {
        writeln!(made, "{edition:?} -> {:?}", edition_key(edition)).unwrap();
    }
    golden("linux-editions.txt", &made);
}

#[test]
fn the_zone_names_given_to_windows_are_as_they_were() {
    // Only names in the table, and names that are no zone: a zone matched by its offsets depends on
    // this PC's time zone files and on the year.
    let mut made = String::new();
    // Every zone in the table, as `zones::answers_golden` keeps it, whichever file it is read from.
    let table = fs::read_to_string(folder().join("linux-zone-table.txt")).unwrap();
    let known = table.lines().filter_map(|line| line.split_once('\t'));
    for (zone, _) in known {
        writeln!(made, "{zone} -> {}", windows_zone(zone)).unwrap();
    }
    for zone in ["", "UTC", "Mars/Olympus_Mons", "<TimeZone>", "America", "../../etc/passwd", "etc/utc"] {
        writeln!(made, "{zone:?} -> {}", windows_zone(zone)).unwrap();
    }
    golden("linux-zone-names.txt", &made);
}
