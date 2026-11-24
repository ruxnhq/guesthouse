//! How a report is sent, as the Linux host sends it: with the system's `curl`, which Windows has
//! had since 10, so the workspace needs no HTTP of its own. The arguments are built here, and what
//! curl said is read here, so both are tested anywhere. Running curl is the app's.

use super::answer::read_answer;
use super::contract::{FORMAT, FORMAT_HEADER, SUMMARY_HEADER};
use std::ffi::OsString;
use std::path::Path;

/// A report is small, but a slow line can take a while with one near the limit.
const MOST_SECONDS: &str = "600";
const CONNECT_SECONDS: &str = "20";

/// What curl is run with to post the archive at `archive` to `address`, with the server's answer
/// written to `answer`. `summary` is the `X-Report-Summary` header's value, which may be empty.
/// Only https is taken for the real server. A stand-in on this PC for a test may be plain http.
pub fn curl_arguments(address: &str, summary: &str, archive: &Path, answer: &Path) -> Vec<OsString> {
    let protocol = if address.starts_with("https://") { "=https" } else { "=http,https" };
    let mut args: Vec<OsString> = Vec::new();
    let mut add = |text: &str| args.push(text.into());
    add("--proto");
    add(protocol);
    add("--silent");
    add("--show-error");
    add("--max-time");
    add(MOST_SECONDS);
    add("--connect-timeout");
    add(CONNECT_SECONDS);
    add("--header");
    add("Content-Type: application/octet-stream");
    add("--header");
    add(&format!("{FORMAT_HEADER}: {FORMAT}"));
    // curl waits a second for a go-ahead before a large body, which this server never sends.
    add("--header");
    add("Expect:");
    if !summary.is_empty() {
        add("--header");
        add(&format!("{SUMMARY_HEADER}: {summary}"));
    }
    add("--data-binary");
    let mut body = OsString::from("@");
    body.push(archive);
    args.push(body);
    args.push("--output".into());
    args.push(answer.into());
    args.push("--write-out".into());
    args.push("%{http_code}".into());
    args.push(address.into());
    args
}

/// The report's ID if curl got the server's answer and it took the report, and otherwise a sentence
/// for the person. `stdout` is what `--write-out` printed, which is the status; `stderr` is what
/// curl said if it failed; `body` is the answer written to its file.
pub fn read_reply(stdout: &str, stderr: &str, curl_succeeded: bool, body: &str) -> Result<String, String> {
    let status: u16 = stdout.trim().parse().unwrap_or(0);
    if status == 0 || !curl_succeeded {
        return Err(format!(
            "The report server couldn't be reached: {}",
            stderr.trim().trim_start_matches("curl: ")
        ));
    }
    read_answer(status, body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const SEND: &str = include_str!("../../../app-core/src/report/send.rs");

    fn arguments(address: &str, summary: &str) -> Vec<String> {
        curl_arguments(address, summary, &PathBuf::from("report.tar.gz"), &PathBuf::from("report.answer"))
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn the_report_is_posted_as_the_linux_host_posts_it() {
        let args = arguments("https://files.ruxn.com/r/guesthouse", "eyJhIjoxfQ");
        let line = args.join(" ");
        for expected in [
            "--proto =https --silent --show-error",
            "--max-time 600 --connect-timeout 20",
            "--header Content-Type: application/octet-stream",
            "--header X-Report-Format: tar.gz",
            "--header Expect:",
            "--header X-Report-Summary: eyJhIjoxfQ",
            "--data-binary @report.tar.gz",
            "--output report.answer --write-out %{http_code} https://files.ruxn.com/r/guesthouse",
        ] {
            assert!(line.contains(expected), "{expected} is not in {line}");
        }
        // The Linux host's own words, so the two stay the same.
        for word in ["\"--proto\"", "\"Content-Type: application/octet-stream\"", "\"Expect:\""] {
            assert!(SEND.contains(word), "{word}");
        }
        assert!(SEND.contains("\"--max-time\", MOST_SECONDS"));
        assert!(SEND.contains(&format!("const MOST_SECONDS: &str = \"{MOST_SECONDS}\";")));
        assert!(SEND.contains(&format!("const CONNECT_SECONDS: &str = \"{CONNECT_SECONDS}\";")));
    }

    #[test]
    fn only_a_stand_in_may_be_plain_http() {
        assert!(
            arguments("http://127.0.0.1:8080/r/guesthouse", "").join(" ").contains("--proto =http,https")
        );
        assert!(arguments("https://files.ruxn.com/r/guesthouse", "").join(" ").contains("--proto =https "));
    }

    #[test]
    fn with_no_summary_there_is_no_summary_header() {
        assert!(!arguments("https://files.ruxn.com/r/guesthouse", "").join(" ").contains("X-Report-Summary"));
    }

    #[test]
    fn the_id_comes_from_a_created_reply() {
        assert_eq!(read_reply("201", "", true, r#"{"id":"0a1b2c3d4e"}"#).unwrap(), "0a1b2c3d4e");
    }

    #[test]
    fn a_failed_curl_says_why() {
        let said = read_reply("000", "curl: (6) Could not resolve host: files.ruxn.com\n", false, "");
        assert_eq!(
            said.unwrap_err(),
            "The report server couldn't be reached: (6) Could not resolve host: files.ruxn.com"
        );
        assert!(read_reply("", "", true, "").is_err(), "no status is no answer");
    }

    #[test]
    fn the_servers_own_sentence_is_passed_on() {
        let said = read_reply("429", "", true, r#"{"error":"Today's reports are full."}"#);
        assert_eq!(said.unwrap_err(), "Today's reports are full.");
    }
}
