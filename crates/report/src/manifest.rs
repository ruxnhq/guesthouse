//! The note at the top of a bundle, which says who reads it and what is in it and what is not. Every
//! host's says the same, but for which host made it, and the Linux host's also says how to get a
//! missing part by hand and adds notes.

use super::contract::KEPT_DAYS;
use super::host::Host;

/// The manifest's own name inside the bundle.
pub const MANIFEST_FILE: &str = "manifest.txt";
/// The tester's own words, as their host scrubs them.
pub const DESCRIPTION_FILE: &str = "description.txt";
/// The facts that sort the report, as `summary.json`.
pub const SUMMARY_FILE: &str = "summary.json";

/// A part that isn't in a report, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Missing {
    pub name: String,
    pub reason: String,
    /// A command the tester could run to get it by hand, if there is one.
    pub how: Option<String>,
}

/// The Windows host's manifest. `files` are each file in the bundle and its size in bytes, and
/// `missing` each part that isn't in it and why.
pub fn manifest(files: &[(String, usize)], missing: &[(String, String)]) -> String {
    let missing: Vec<Missing> = missing
        .iter()
        .map(|(name, reason)| Missing { name: name.clone(), reason: reason.clone(), how: None })
        .collect();
    manifest_for(Host::WINDOWS, files, &missing, &[])
}

/// The manifest's text, for a report from `host`, which says who it is from as that host does.
/// `files`, `missing` and `notes` are as in [`manifest_from`].
pub fn manifest_for(host: Host, files: &[(String, usize)], missing: &[Missing], notes: &[String]) -> String {
    manifest_from(host.phrase(), files, missing, notes)
}

/// The manifest's text, for a report from `from`, or from no host named, as the Linux host's says.
/// `files` are each file in the bundle and its size in bytes, `missing` each part that isn't in it,
/// and `notes` lines that explain what's there and what isn't.
pub fn manifest_from(
    from: Option<&str>,
    files: &[(String, usize)],
    missing: &[Missing],
    notes: &[String],
) -> String {
    let from = from.map(|host| format!(" from {host}")).unwrap_or_default();
    let mut text = format!(
        "A Guesthouse bug report{from}. It goes to the developer, is read by the developer and their \
         automation tools, and is deleted after {KEPT_DAYS} days.\n\
         The user name, host name and home folder were taken out of its files where they were found. That's \
         a best effort on free-form logs, not a promise.\n\n\
         Files:\n"
    );
    for (name, bytes) in files {
        text.push_str(&format!("  {name} ({bytes} bytes)\n"));
    }
    text.push_str(&format!("  {MANIFEST_FILE}\n"));
    if !missing.is_empty() {
        text.push_str("\nNot in it:\n");
        for missing in missing {
            text.push_str(&format!("  {}: {}\n", missing.name, missing.reason));
            if let Some(how) = &missing.how {
                text.push_str(&format!("    to get it by hand: {how}\n"));
            }
        }
    }
    if !notes.is_empty() {
        text.push_str("\nNotes:\n");
        for note in notes {
            text.push_str(&format!("  {note}\n"));
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_says_who_reads_it_and_lists_the_files() {
        let text = manifest(&[("service.log".to_owned(), 1200), ("summary.json".to_owned(), 80)], &[]);
        assert!(text.starts_with("A Guesthouse bug report from a Windows host. It goes to the developer"));
        assert!(text.contains("is deleted after 30 days."));
        assert!(
            text.contains("Files:\n  service.log (1200 bytes)\n  summary.json (80 bytes)\n  manifest.txt\n")
        );
        assert!(!text.contains("Not in it"));
    }

    #[test]
    fn what_is_missing_is_said_with_why() {
        let text = manifest(
            &[("app.log".to_owned(), 5)],
            &[("crash dumps".to_owned(), "there were none".to_owned())],
        );
        assert!(text.ends_with("\nNot in it:\n  crash dumps: there were none\n"));
    }

    #[test]
    fn every_host_names_a_reports_own_files_the_same() {
        assert_eq!(
            [MANIFEST_FILE, DESCRIPTION_FILE, SUMMARY_FILE],
            ["manifest.txt", "description.txt", "summary.json"]
        );
    }

    #[test]
    fn its_words_are_the_linux_hosts() {
        let linux = manifest_from(None, &[], &[], &[]);
        assert!(linux.starts_with(
            "A Guesthouse bug report. It goes to the developer, is read by the developer and their \
             automation tools, and is deleted after 30 days.\n"
        ));
        assert!(linux.contains("a best effort on free-form logs, not a promise."));
        let windows = manifest(&[], &[]);
        assert_eq!(windows.replace(" from a Windows host", ""), linux);
    }

    #[test]
    fn the_linux_host_says_how_to_get_a_part_by_hand_and_adds_notes() {
        let missing = [
            Missing { name: "kernel".into(), reason: "not allowed".into(), how: Some("sudo dmesg".into()) },
            Missing { name: "events".into(), reason: "not running".into(), how: None },
        ];
        let text = manifest_from(None, &[], &missing, &["A note.".to_owned()]);
        assert!(text.ends_with(
            "\nNot in it:\n  kernel: not allowed\n    to get it by hand: sudo dmesg\n  events: not running\n\
             \nNotes:\n  A note.\n"
        ));
    }
}
