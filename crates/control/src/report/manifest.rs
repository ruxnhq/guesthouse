//! The note at the top of a bundle, which says who reads it and what is in it and what is not. The
//! words are the Linux host's, but for who made it.

use super::contract::KEPT_DAYS;

/// The manifest's own name inside the bundle.
pub const MANIFEST_FILE: &str = "manifest.txt";

/// The manifest's text. `files` are each file in the bundle and its size in bytes, and `missing` each
/// part that isn't in it and why.
pub fn manifest(files: &[(String, usize)], missing: &[(String, String)]) -> String {
    let mut text = format!(
        "A Guesthouse bug report from a Windows host. It goes to the developer, is read by the developer \
         and their automation tools, and is deleted after {KEPT_DAYS} days.\n\
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
        for (name, reason) in missing {
            text.push_str(&format!("  {name}: {reason}\n"));
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
    fn its_words_are_the_linux_hosts() {
        let linux = include_str!("../../../app-core/src/report/bundle.rs");
        assert!(linux.contains("It goes to the developer, is read by the developer and their automation \\"));
        assert!(linux.contains("tools, and is deleted after {KEPT_DAYS} days."));
        assert!(linux.contains("a best effort on free-form logs, not a promise."));
    }
}
