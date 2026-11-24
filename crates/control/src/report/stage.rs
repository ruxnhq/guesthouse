//! Makes the files of a report from what was gathered: scrubs each, keeps the end of a long one,
//! adds the description, the summary and the manifest, and says what is missing. The app writes the
//! result into a folder and packs it. Nothing here touches a file, so all of it is tested anywhere.

use super::kind::ProblemKind;
use super::manifest::{MANIFEST_FILE, manifest};
use super::parts::Parts;
use super::scrub::Scrubber;
use super::summary::{self, Summary};
use super::trim::keep_end;
use crate::System;

/// The tester's own words, scrubbed.
pub const DESCRIPTION_FILE: &str = "description.txt";
/// The facts that sort the report, as `summary.json`.
pub const SUMMARY_FILE: &str = "summary.json";
/// The most of one file a report takes at first. A report that is too big to send is made again
/// with half as much of each file.
pub const FIRST_LIMIT: usize = 4 * 1024 * 1024;
/// How many times a report that is too big is made again, each with half as much.
pub const MOST_REPACKS: u32 = 6;
/// The longest a file's name may be, folder and all.
const LONGEST_NAME: usize = 120;

/// What the tester said and what the app knows about the PC, for the summary and the description.
#[derive(Debug, Clone)]
pub struct Facts<'a> {
    /// The Windows host's version.
    pub version: &'a str,
    /// The service's `windows.txt`, if it came: the Windows edition, version and build.
    pub windows: Option<&'a str>,
    /// The name of the card the default is, from the service's last status.
    pub card: Option<&'a str>,
    /// The chosen machine's system, never its name.
    pub machine: Option<System>,
    pub kind: ProblemKind,
    /// What went wrong, as the tester wrote it. It's scrubbed here.
    pub description: &'a str,
    /// How to reach them, if they gave one. It's for the developer, so it's not scrubbed.
    pub contact: &'a str,
}

/// A report's files, ready to be written, and what is not in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Staged {
    /// Each file's name and text, the manifest last.
    pub files: Vec<(String, String)>,
    /// Each part that is not in it, and why.
    pub missing: Vec<(String, String)>,
    pub summary: Summary,
}

impl Staged {
    /// Each file's name and size in bytes.
    pub fn sizes(&self) -> Vec<(String, usize)> {
        self.files.iter().map(|(name, text)| (name.clone(), text.len())).collect()
    }
}

/// The Windows version in one line from the service's `windows.txt`, such as
/// `Windows 11 Pro 24H2 build 26100`. `None` if it says none of it.
pub fn os_line(windows: &str) -> Option<String> {
    let value = |key: &str| {
        windows
            .lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix(':'))
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    let mut words: Vec<String> = Vec::new();
    words.extend(value("ProductName").map(str::to_owned));
    words.extend(value("DisplayVersion").map(str::to_owned));
    words.extend(value("CurrentBuild").map(|build| format!("build {build}")));
    (!words.is_empty()).then(|| words.join(" "))
}

/// The summary: what sorts the report before anyone opens it. `description` is already scrubbed.
pub fn build_summary(facts: &Facts, description: &str) -> Summary {
    let mut summary = Summary::default();
    summary.set(summary::VERSION, facts.version);
    summary.set(summary::HOST, summary::HOST_WINDOWS);
    if let Some(os) = facts.windows.and_then(os_line) {
        summary.set(summary::OS, &os);
    }
    if let Some(card) = facts.card {
        summary.set(summary::CARD, card);
    }
    match facts.machine {
        Some(System::Windows) => summary.set(summary::MACHINE, "windows"),
        Some(System::Linux) => summary.set(summary::MACHINE, "linux"),
        None => {}
    }
    summary.set(summary::KIND, facts.kind.word());
    summary.set(summary::DESCRIPTION, description);
    summary.set(summary::CONTACT, facts.contact);
    summary
}

/// Makes the files. Each of `parts` is scrubbed with `scrubber` and cut to its last `per_file`
/// bytes. A name that isn't safe to make a file of is made safe, and one the report keeps for itself
/// is put in what's missing.
pub fn stage(parts: &Parts, scrubber: &Scrubber, facts: &Facts, per_file: usize) -> Staged {
    let mut files: Vec<(String, String)> = Vec::new();
    let mut missing: Vec<(String, String)> = Vec::new();
    for (name, text) in &parts.files {
        match file_name(name, scrubber, &files) {
            Some(name) => files.push((name, keep_end(&scrubber.scrub(text), per_file))),
            None => missing.push((
                scrubber.scrub(name),
                "That name is the report's own, so the file is left out.".to_owned(),
            )),
        }
    }
    for (name, reason) in &parts.missing {
        missing.push((scrubber.scrub(name), scrubber.scrub(reason)));
    }
    let windows = parts.text("windows.txt");
    let description = scrubber.scrub(facts.description.trim());
    let summary = build_summary(&Facts { windows, ..facts.clone() }, &description);
    files.push((DESCRIPTION_FILE.to_owned(), format!("{description}\n")));
    files.push((SUMMARY_FILE.to_owned(), summary.to_json()));
    let sizes: Vec<(String, usize)> = files.iter().map(|(name, text)| (name.clone(), text.len())).collect();
    files.push((MANIFEST_FILE.to_owned(), manifest(&sizes, &missing)));
    Staged { files, missing, summary }
}

/// The name a part's file is given: its own, scrubbed (a machine's name is in its record's), and made
/// of letters, digits and `.`, `-`, `_`, with `/` between folders. `None` if it's one the report
/// keeps for itself. A name that another file has is told apart with a number.
fn file_name(name: &str, scrubber: &Scrubber, taken: &[(String, String)]) -> Option<String> {
    if [DESCRIPTION_FILE, SUMMARY_FILE, MANIFEST_FILE].contains(&name) {
        return None;
    }
    // Only a record has a machine's name in its own. Scrubbing every name could change a fixed one.
    let scrubbed = if name.ends_with(".guesthouse.json") { scrubber.scrub(name) } else { name.to_owned() };
    let mut safe: String = scrubbed
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '/') { c } else { '_' })
        .take(LONGEST_NAME)
        .collect();
    // No way out of the report's folder, and no empty or hidden folder.
    safe = safe.trim_start_matches(['/', '.']).replace("..", "._").replace("//", "/_");
    if safe.is_empty() || safe.ends_with('/') {
        safe.push_str("file");
    }
    let mut chosen = safe.clone();
    let mut number = 2;
    while taken.iter().any(|(known, _)| known.eq_ignore_ascii_case(&chosen)) {
        chosen = format!("{safe}-{number}");
        number += 1;
    }
    Some(chosen)
}

/// What a report is made again with when it was too big: half as much of each file.
pub fn halved(per_file: usize) -> usize {
    (per_file / 2).max(1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scrubber() -> Scrubber {
        Scrubber::new(
            "alice",
            r"C:\Users\alice",
            "DESKTOP-7F3K",
            &["studio".to_owned(), "build-box".to_owned()],
        )
    }

    fn facts<'a>() -> Facts<'a> {
        Facts {
            version: "0.10.0",
            windows: None,
            card: Some("NVIDIA GeForce RTX 4080"),
            machine: Some(System::Linux),
            kind: ProblemKind::WontStart,
            description: "  It won't start. alice@example.org   ",
            contact: "alice@example.org",
        }
    }

    fn parts() -> Parts {
        let mut parts = Parts::default();
        parts.piece("service.log", "started for alice on DESKTOP-7F3K\n");
        parts
            .piece("windows.txt", "ProductName: Windows 11 Pro\nDisplayVersion: 24H2\nCurrentBuild: 26100\n");
        parts.piece("studio.guesthouse.json", "{}");
        parts.piece("snapshots/index.json", "{}");
        parts.miss("crashes.txt", "wevtutil failed");
        parts
    }

    #[test]
    fn each_part_is_scrubbed_and_the_files_end_with_the_manifest() {
        let staged = stage(&parts(), &scrubber(), &facts(), FIRST_LIMIT);
        let names: Vec<&str> = staged.files.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            [
                "service.log",
                "windows.txt",
                "machine.guesthouse.json",
                "snapshots/index.json",
                "description.txt",
                "summary.json",
                "manifest.txt",
            ],
            "a machine's name is not in its record's name"
        );
        let log = &staged.files[0].1;
        assert_eq!(log, "started for <user> on <host>\n");
        assert_eq!(staged.missing, [("crashes.txt".to_owned(), "wevtutil failed".to_owned())]);
    }

    #[test]
    fn the_description_is_scrubbed_and_the_contact_is_not() {
        let staged = stage(&parts(), &scrubber(), &facts(), FIRST_LIMIT);
        let description = staged.files.iter().find(|(name, _)| name == DESCRIPTION_FILE).unwrap();
        assert_eq!(description.1, "It won't start. <email>\n");
        assert_eq!(staged.summary.get(summary::DESCRIPTION), Some("It won't start. <email>"));
        assert_eq!(staged.summary.get(summary::CONTACT), Some("alice@example.org"));
    }

    #[test]
    fn the_summary_has_the_facts_and_never_a_machines_name() {
        let staged = stage(&parts(), &scrubber(), &facts(), FIRST_LIMIT);
        let summary = &staged.summary;
        assert_eq!(summary.get(summary::VERSION), Some("0.10.0"));
        assert_eq!(summary.get(summary::HOST), Some("windows"));
        assert_eq!(summary.get(summary::OS), Some("Windows 11 Pro 24H2 build 26100"));
        assert_eq!(summary.get(summary::CARD), Some("NVIDIA GeForce RTX 4080"));
        assert_eq!(summary.get(summary::MACHINE), Some("linux"));
        assert_eq!(summary.get(summary::KIND), Some("wont-start"));
        let (_, json) = staged.files.iter().find(|(name, _)| name == SUMMARY_FILE).unwrap();
        assert!(!json.contains("studio"));
    }

    #[test]
    fn a_summary_with_nothing_known_has_only_what_is_always_known() {
        let bare = Facts { card: None, machine: None, description: "", contact: "", ..facts() };
        let summary = build_summary(&bare, "");
        let keys: Vec<&str> = summary.facts().map(|(key, _)| key).collect();
        assert_eq!(keys, ["host", "kind", "version"]);
    }

    #[test]
    fn a_long_file_keeps_its_end() {
        let mut parts = Parts::default();
        parts.piece("service.log", &"0123456789\n".repeat(1000));
        let staged = stage(&parts, &scrubber(), &facts(), 100);
        assert!(staged.files[0].1.starts_with("[10"), "{}", staged.files[0].1);
        assert!(staged.files[0].1.len() < 200);
    }

    #[test]
    fn the_manifest_lists_every_file_with_its_size_and_what_is_missing() {
        let staged = stage(&parts(), &scrubber(), &facts(), FIRST_LIMIT);
        let (_, text) = staged.files.last().unwrap();
        let log = staged.files[0].1.len();
        assert!(text.contains(&format!("  service.log ({log} bytes)\n")), "{text}");
        assert!(text.contains("  description.txt ("));
        assert!(text.contains("  summary.json ("));
        assert!(text.contains("Not in it:\n  crashes.txt: wevtutil failed\n"));
        assert_eq!(staged.sizes().len(), staged.files.len());
    }

    #[test]
    fn a_name_the_report_keeps_is_not_taken_and_unsafe_names_are_made_safe() {
        let mut parts = Parts::default();
        parts.piece("summary.json", "fake");
        parts.piece("../../etc/passwd", "x");
        parts.piece("C:\\x\\y.txt", "y");
        parts.piece("a/../b", "z");
        let staged = stage(&parts, &scrubber(), &facts(), FIRST_LIMIT);
        let names: Vec<&str> = staged.files.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names[..3], ["etc/passwd", "C__x_y.txt", "a/._/b"]);
        assert!(staged.missing.iter().any(|(name, _)| name == "summary.json"));
        let (_, real) = staged.files.iter().find(|(name, _)| name == SUMMARY_FILE).unwrap();
        assert!(real.contains("\"host\""), "the real summary is the report's");
    }

    #[test]
    fn two_names_that_come_to_one_are_told_apart() {
        let mut parts = Parts::default();
        parts.piece("a b.txt", "1");
        parts.piece("a_b.txt", "2");
        let staged = stage(&parts, &scrubber(), &facts(), FIRST_LIMIT);
        assert_eq!(staged.files[0].0, "a_b.txt");
        assert_eq!(staged.files[1].0, "a_b.txt-2");
    }

    #[test]
    fn the_os_comes_from_the_lines_that_are_there() {
        assert_eq!(os_line("ProductName: Windows 11 Pro\nUBR: 1\n").as_deref(), Some("Windows 11 Pro"));
        assert_eq!(os_line("EditionID: Pro\n"), None);
        assert_eq!(os_line(""), None);
    }

    #[test]
    fn a_report_that_was_too_big_is_made_again_with_half() {
        assert_eq!(halved(FIRST_LIMIT), FIRST_LIMIT / 2);
        let mut limit = FIRST_LIMIT;
        for _ in 0..MOST_REPACKS {
            limit = halved(limit);
        }
        assert_eq!(limit, 64 * 1024);
        assert_eq!(halved(1), 1024);
    }
}
