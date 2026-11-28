//! How a report is sent, as every host sends it: with the system's `curl`, which Linux hosts have
//! and Windows has had since 10, so the workspace needs no HTTP of its own. The arguments are built
//! here, and what curl said is read here, so both are tested anywhere. Running curl is each host's.

use super::answer::read_answer;
use super::contract::{FORMAT, FORMAT_HEADER, SUMMARY_HEADER};
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

/// A report is small, but a slow line can take a while with one near the limit.
pub const MOST_SECONDS: &str = "600";
pub const CONNECT_SECONDS: &str = "20";

/// The file beside the archive that holds the summary header for curl to read, as the summary is
/// the person's own words and a command line is read by every user of the PC.
pub fn summary_file(archive: &Path) -> PathBuf {
    let mut name = archive.as_os_str().to_owned();
    name.push(".summary");
    PathBuf::from(name)
}

/// Writes the summary header for [`curl_arguments`], for this user only. The file is new: one already
/// there is an error. Takes nothing to write when `summary` is empty. Remove it once curl has run.
pub fn write_summary_file(archive: &Path, summary: &str) -> std::io::Result<()> {
    if summary.is_empty() {
        return Ok(());
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(summary_file(archive))?;
    writeln!(file, "{SUMMARY_HEADER}: {summary}")
}

/// What curl is run with to post the archive at `archive` to `address`, with the server's answer
/// written to `answer`. `summary` is the `X-Report-Summary` header's value, which may be empty; when it
/// isn't, curl reads the header from [`summary_file`], which [`write_summary_file`] has made.
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
        add(&format!("@{}", summary_file(archive).display()));
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
            "--header @report.tar.gz.summary",
            "--data-binary @report.tar.gz",
            "--output report.answer --write-out %{http_code} https://files.ruxn.com/r/guesthouse",
        ] {
            assert!(line.contains(expected), "{expected} is not in {line}");
        }
        // The Linux host's own values, pinned now that both hosts post with these arguments.
        assert_eq!(MOST_SECONDS, "600");
        assert_eq!(CONNECT_SECONDS, "20");
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

    #[test]
    fn the_summary_is_in_a_file_for_this_user_and_not_on_the_command_line() {
        let folder = std::env::temp_dir().join(format!("guesthouse-summary-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let archive = folder.join("report.tar.gz");
        let secret = "eyJjb250YWN0IjoiYUBiLmNvbSJ9";
        write_summary_file(&archive, secret).unwrap();
        let line = curl_arguments("https://x.example/r", secret, &archive, &folder.join("a"))
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(!line.contains(secret), "{line}");
        assert!(line.contains(&format!("--header @{}", summary_file(&archive).display())));
        let file = std::fs::read_to_string(summary_file(&archive)).unwrap();
        assert_eq!(file, format!("X-Report-Summary: {secret}\n"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(summary_file(&archive)).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0);
        }
        assert!(write_summary_file(&archive, secret).is_err(), "a file already there is not written into");
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
