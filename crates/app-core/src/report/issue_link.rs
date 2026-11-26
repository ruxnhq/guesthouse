//! A link to a new GitHub issue, filled in with a sent report's ID, the summary's facts, the tester's
//! words and a few lines of the app's own log. It's only ever offered: the tester decides whether to
//! open it, and can change anything in it before posting.
//!
//! Nothing from QEMU or the guest goes in it, as the issue is public.

use super::endpoint::NEW_ISSUE;
use super::summary::{self, Summary};

/// GitHub answered issue links of up to about 6,900 characters, and failed with a server error from
/// about 7,000, when this was measured without being signed in. This leaves room below that.
const MOST_LINK_CHARS: usize = 6000;
/// About this many lines of the log, around the failure.
const EXCERPT_LINES: usize = 20;
/// Lines of the log after the failure, which say what the app did about it.
const LINES_AFTER: usize = 5;
/// Words of the app's own lines that say something went wrong.
const FAILURE_WORDS: [&str; 7] = ["fail", "error", "panic", "could not", "couldn't", "did not", "stopped"];
/// A log line's time, as `12:34:56`.
const TIME_LENGTH: usize = 8;
/// Who wrote a line, as the word after its time, for the lines that only ever hold the host's side:
/// the window, the viewer, and each task's steps (the actions in `run_action` in actions/mod.rs). Many
/// other lines pass on what the guest or QEMU said, so a line from anyone else stays out.
const OWN_SPEAKERS: [&str; 21] = [
    "app:",
    "frames:",
    "own",
    "card:",
    "plan:",
    "after",
    "start",
    "stop",
    "force",
    "grow",
    "resume",
    "clean_up",
    "after_reboot",
    "apply_display",
    "install_display",
    "remote_access",
    "install",
    "take_snapshot",
    "go_back",
    "delete_snapshot",
    "panic:",
];
/// Words that mean a line quotes QEMU or the guest, whoever wrote it.
const GUEST_WORDS: [&str; 4] = ["last words", "qemu-system", "the guest said", "serial"];
/// The facts worth a line in the issue, in the order they read best.
const FACTS: [&str; 8] = [
    summary::KIND,
    summary::VERSION,
    summary::CARD,
    summary::DRIVER,
    summary::DISTRO,
    summary::KERNEL,
    summary::SESSION,
    summary::MACHINE,
];

/// The lines around the last failure in the app's log, or its last lines if nothing failed, with any
/// line that carries QEMU's or the guest's words left out. `log` must already be scrubbed.
pub fn failure_excerpt(log: &str) -> Vec<String> {
    let own: Vec<&str> = log.lines().filter(|line| is_own_line(line)).collect();
    let failure = own.iter().rposition(|line| {
        let lower = line.to_lowercase();
        FAILURE_WORDS.iter().any(|word| lower.contains(word))
    });
    let end = failure.map_or(own.len(), |at| (at + 1 + LINES_AFTER).min(own.len()));
    let start = end.saturating_sub(EXCERPT_LINES);
    own[start..end].iter().map(|line| (*line).to_owned()).collect()
}

/// A line with a time, written by the app's own side, that quotes nobody else. A line with no time
/// is the rest of something written over several lines, which is often what a guest said.
fn is_own_line(line: &str) -> bool {
    let Some((time, rest)) = line.split_once(' ') else { return false };
    let timed = time.len() == TIME_LENGTH && time.bytes().all(|byte| byte.is_ascii_digit() || byte == b':');
    let speaker = rest.split(' ').next().unwrap_or_default();
    let lower = line.to_lowercase();
    timed && OWN_SPEAKERS.contains(&speaker) && !GUEST_WORDS.iter().any(|word| lower.contains(word))
}

/// The link. The excerpt is cut first, from its oldest line, and then the description, until it fits.
pub fn issue_link(report_id: &str, summary: &Summary, description: &str, excerpt: &[String]) -> String {
    let title = format!("Report {report_id}");
    let mut excerpt = excerpt.to_vec();
    let mut description: String = description.trim().to_owned();
    loop {
        let link = format!(
            "{NEW_ISSUE}?title={}&body={}",
            percent_encode(&title),
            percent_encode(&body(report_id, summary, &description, &excerpt))
        );
        if link.len() <= MOST_LINK_CHARS {
            return link;
        }
        if !excerpt.is_empty() {
            excerpt.remove(0);
        } else if !description.is_empty() {
            let keep = description.chars().count() * 3 / 4;
            description = description.chars().take(keep).collect();
        } else {
            return format!("{NEW_ISSUE}?title={}", percent_encode(&title));
        }
    }
}

fn body(report_id: &str, summary: &Summary, description: &str, excerpt: &[String]) -> String {
    let mut text = format!("Report ID: {report_id}\n\n");
    for key in FACTS {
        if let Some(value) = summary.get(key) {
            text.push_str(&format!("- {key}: {value}\n"));
        }
    }
    if !description.is_empty() {
        text.push_str(&format!("\n## What went wrong\n\n{description}\n"));
    }
    if !excerpt.is_empty() {
        text.push_str(&format!("\n## The app's log\n\n```\n{}\n```\n", excerpt.join("\n")));
    }
    text
}

/// Everything but the characters a URL leaves alone is written as `%` and its bytes in hex.
fn percent_encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(byte.into());
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> Summary {
        let mut summary = Summary::default();
        summary.set(summary::VERSION, "0.6.1");
        summary.set(summary::CARD, "NVIDIA GeForce RTX 4080");
        summary.set(summary::KIND, "wont-start");
        summary.set(summary::CONTACT, "someone@example.org");
        summary
    }

    #[test]
    fn the_link_holds_the_id_the_facts_and_the_words() {
        let link = issue_link("0123456789", &summary(), "It went black.", &["10:00 app: it failed".into()]);
        assert!(link.starts_with(
            "https://github.com/ruxnlabs/guesthouse/issues/new?title=Report%200123456789&body="
        ));
        assert!(link.contains("RTX%204080"));
        assert!(link.contains("It%20went%20black."));
        assert!(link.contains("it%20failed"));
        assert!(!link.contains("example.org"), "the contact is for the developer, not a public issue");
    }

    #[test]
    fn a_long_link_loses_its_excerpt_before_the_description() {
        let excerpt: Vec<String> =
            (0..200).map(|number| format!("10:00 app: line {number} {}", "x".repeat(60))).collect();
        let link = issue_link("0123456789", &summary(), "It went black.", &excerpt);
        assert!(link.len() <= MOST_LINK_CHARS);
        assert!(link.contains("It%20went%20black."));
        assert!(link.contains("line%20199"), "the newest lines stay");
        assert!(!link.contains("line%200%20"));
    }

    #[test]
    fn a_long_description_is_cut_to_fit() {
        let link = issue_link("0123456789", &summary(), &"写".repeat(5000), &[]);
        assert!(link.len() <= MOST_LINK_CHARS);
        assert!(link.contains("Report%200123456789"));
    }

    #[test]
    fn the_excerpt_is_around_the_last_failure() {
        let mut log: Vec<String> = (0..100).map(|number| format!("10:00:00 start fine {number}")).collect();
        log[50] = "10:00:00 app: The VM did not start: something is still running.".into();
        let excerpt = failure_excerpt(&log.join("\n"));
        assert_eq!(excerpt.len(), EXCERPT_LINES);
        assert_eq!(excerpt.last().unwrap(), "10:00:00 start fine 55");
        assert!(excerpt.iter().any(|line| line.contains("did not start")));
    }

    #[test]
    fn with_no_failure_the_excerpt_is_the_end() {
        let log: Vec<String> = (0..100).map(|number| format!("10:00:00 start fine {number}")).collect();
        let excerpt = failure_excerpt(&log.join("\n"));
        assert_eq!(excerpt.first().unwrap(), "10:00:00 start fine 80");
        assert_eq!(excerpt.last().unwrap(), "10:00:00 start fine 99");
    }

    #[test]
    fn qemus_and_the_guests_words_stay_out() {
        let log = "10:00:00 start Starting the VM\n\
                   10:00:01 w11 stopped by itself. QEMU's last words: qemu-system-x86_64: secret\n\
                   10:00:02 display: session user GuestAccount\n\
                   CM_PROB_NONE from inside\n\
                   10:00:03 app: It stopped, and this app didn't stop it.";
        let excerpt = failure_excerpt(log).join("\n");
        for guest in ["secret", "GuestAccount", "inside"] {
            assert!(!excerpt.contains(guest), "{guest} is in:\n{excerpt}");
        }
        assert_eq!(
            excerpt,
            "10:00:00 start Starting the VM\n10:00:03 app: It stopped, and this app didn't stop it."
        );
    }
}
