//! A link to a new GitHub issue with a sent report's ID in it, and the summary's facts. It's only
//! ever offered: the person decides whether to open it, and can change anything before posting.
//!
//! The issue is public. The Windows host's link, [`issue_link`], holds no log, no description and
//! no contact: only what sorts a report. The Linux host's, [`issue_link_with_words`], also holds the
//! tester's words and a few lines of the app's own log. Each is kept as its host had it. The
//! contact is in neither, and the report itself stays on the report server.

use super::contract::NEW_ISSUE;
use super::summary::{self, Summary};

/// GitHub answered issue links of up to about 6,900 characters, and failed from about 7,000. This
/// leaves room below that, as the Linux host's link does.
pub const MOST_LINK_CHARS: usize = 6000;
/// The facts worth a line in the Windows host's issue, in the order they read best.
const FACTS: [&str; 6] =
    [summary::KIND, summary::VERSION, summary::HOST, summary::OS, summary::CARD, summary::MACHINE];
/// The facts worth a line in the Linux host's issue, in the order they read best.
const FACTS_WITH_WORDS: [&str; 8] = [
    summary::KIND,
    summary::VERSION,
    summary::CARD,
    summary::DRIVER,
    summary::DISTRO,
    summary::KERNEL,
    summary::SESSION,
    summary::MACHINE,
];

/// The link, with the report's `id` as its title and in its body, and `summary`'s facts under it.
pub fn issue_link(id: &str, summary: &Summary) -> String {
    let title = format!("Report {id}");
    let mut body = format!("Report ID: {id}\n\n");
    for key in FACTS {
        if let Some(value) = summary.get(key) {
            body.push_str(&format!("- {key}: {value}\n"));
        }
    }
    // A value is at most 2000 characters, so a link of all of them can be too long. The facts that
    // read best are first, so the last are cut one by one.
    loop {
        let link = format!("{NEW_ISSUE}?title={}&body={}", percent_encode(&title), percent_encode(&body));
        if link.len() <= MOST_LINK_CHARS {
            return link;
        }
        match body.trim_end().rfind('\n') {
            Some(at) if at > 0 => body.truncate(at + 1),
            _ => return format!("{NEW_ISSUE}?title={}", percent_encode(&title)),
        }
    }
}

/// The Linux host's link, with the tester's words under the facts and then `excerpt`, lines of the
/// app's own log. The excerpt is cut first, from its oldest line, and then the description, until it
/// fits.
pub fn issue_link_with_words(
    report_id: &str,
    summary: &Summary,
    description: &str,
    excerpt: &[String],
) -> String {
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
    for key in FACTS_WITH_WORDS {
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
        summary.set(summary::VERSION, "0.10.0");
        summary.set(summary::HOST, summary::HOST_WINDOWS);
        summary.set(summary::OS, "Windows 11 Pro 24H2 build 26100");
        summary.set(summary::CARD, "NVIDIA GeForce RTX 4080");
        summary.set(summary::KIND, "wont-start");
        summary.set(summary::MACHINE, "windows");
        summary.set(summary::DESCRIPTION, "It went black after the logo, on my own machine.");
        summary.set(summary::CONTACT, "someone@example.org");
        summary
    }

    #[test]
    fn the_link_holds_the_id_and_the_facts_and_nothing_else() {
        let link = issue_link("0a1b2c3d4e", &summary());
        assert!(link.starts_with(&format!("{NEW_ISSUE}?title=Report%200a1b2c3d4e&body=")));
        assert!(link.contains("Report%20ID%3A%200a1b2c3d4e"));
        for fact in ["wont-start", "0.10.0", "windows", "RTX%204080", "build%2026100"] {
            assert!(link.contains(fact), "{fact}");
        }
        assert!(!link.contains("example.org"), "the contact is for the developer, not a public issue");
        assert!(!link.contains("went%20black"), "the description stays in the report");
    }

    #[test]
    fn a_fact_is_written_as_a_list_item() {
        let link = issue_link("0a1b2c3d4e", &summary());
        assert!(link.contains("-%20kind%3A%20wont-start%0A"));
    }

    #[test]
    fn a_link_that_would_be_too_long_is_cut_by_whole_facts_and_keeps_the_id() {
        let mut long = summary();
        long.set(summary::CARD, &"G".repeat(2000));
        long.set(summary::OS, &"W".repeat(2000));
        long.set(summary::VERSION, &"V".repeat(2000));
        let link = issue_link("0a1b2c3d4e", &long);
        assert!(link.len() <= MOST_LINK_CHARS, "{}", link.len());
        assert!(link.contains("Report%200a1b2c3d4e"));
        assert!(link.contains("kind%3A%20wont-start"), "the first facts stay");
    }

    #[test]
    fn a_report_with_no_facts_still_has_its_id() {
        let link = issue_link("0a1b2c3d4e", &Summary::default());
        assert!(link.contains("title=Report%200a1b2c3d4e&body=Report%20ID%3A%200a1b2c3d4e"));
    }

    #[test]
    fn only_what_a_url_leaves_alone_is_not_encoded() {
        assert_eq!(percent_encode("a b&c=d/é-_.~"), "a%20b%26c%3Dd%2F%C3%A9-_.~");
    }
}
