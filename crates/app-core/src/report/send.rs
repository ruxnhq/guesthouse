//! Sends a written report to the server with the system's `curl`, which `install.sh` already needs, so
//! the workspace needs no HTTP of its own.

use super::bundle::{Bundle, FORMAT};
use super::endpoint::{MOST_REPORT_BYTES, report_address};
use crate::failure::{Failure, Result};
use serde::Deserialize;
use std::fs;
use std::process::Command;

/// A report is small, but a slow line can take a while with one near the limit.
const MOST_SECONDS: &str = "600";
const CONNECT_SECONDS: &str = "20";
const CREATED: u16 = 201;
const ID_LENGTH: usize = 10;

#[derive(Deserialize)]
struct Answer {
    id: Option<String>,
    error: Option<String>,
}

/// Sends the report, and gives back the ID the server gave it. On failure, the server's own sentence
/// if it gave one.
pub fn send(bundle: &Bundle) -> Result<String> {
    let size = fs::metadata(&bundle.path)?.len();
    if size > MOST_REPORT_BYTES {
        return Err(Failure::new("The report is too big to send. Save it to a file instead."));
    }
    let address = report_address();
    let answer_file = bundle.path.with_extension("answer");
    let mut curl = Command::new("curl");
    // Only https for the real server. A stand-in on this PC for a test can be plain http.
    let protocol = if address.starts_with("https://") { "=https" } else { "=http,https" };
    curl.args(["--proto", protocol, "--silent", "--show-error"])
        .args(["--max-time", MOST_SECONDS, "--connect-timeout", CONNECT_SECONDS])
        .args(["--header", "Content-Type: application/octet-stream"])
        .args(["--header", &format!("X-Report-Format: {FORMAT}")])
        // curl waits a second for a go-ahead before a large body, which this server never sends.
        .args(["--header", "Expect:"]);
    let summary = bundle.summary.header();
    if !summary.is_empty() {
        curl.args(["--header", &format!("X-Report-Summary: {summary}")]);
    }
    let mut body = std::ffi::OsString::from("@");
    body.push(&bundle.path);
    curl.arg("--data-binary").arg(body);
    curl.arg("--output").arg(&answer_file).args(["--write-out", "%{http_code}"]).arg(&address);
    let result = curl.output();
    let answer = fs::read_to_string(&answer_file).unwrap_or_default();
    let _ = fs::remove_file(&answer_file);
    let result =
        result.map_err(|error| Failure(format!("curl could not run, so nothing was sent: {error}")))?;
    let status: u16 = String::from_utf8_lossy(&result.stdout).trim().parse().unwrap_or(0);
    if status == 0 || !result.status.success() {
        let said = String::from_utf8_lossy(&result.stderr);
        return Err(Failure(format!(
            "The report server couldn't be reached: {}",
            said.trim().trim_start_matches("curl: ")
        )));
    }
    read_answer(status, &answer)
}

fn read_answer(status: u16, answer: &str) -> Result<String> {
    let parsed: Option<Answer> = serde_json::from_str(answer).ok();
    if status == CREATED {
        let id = parsed.and_then(|answer| answer.id).unwrap_or_default();
        if id.len() == ID_LENGTH && id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Ok(id);
        }
        return Err(Failure::new("The report server took the report but gave no ID it could be found by."));
    }
    match parsed.and_then(|answer| answer.error).filter(|error| !error.trim().is_empty()) {
        Some(error) => Err(Failure(error)),
        None => {
            Err(Failure(format!("The report server answered with an error ({status}), and gave no reason.")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_comes_from_a_created_answer() {
        assert_eq!(read_answer(201, r#"{"id":"0a1b2c3d4e"}"#).unwrap(), "0a1b2c3d4e");
    }

    #[test]
    fn an_id_of_another_shape_is_not_taken() {
        assert!(read_answer(201, r#"{"id":"../../etc"}"#).is_err());
        assert!(read_answer(201, "not json").is_err());
    }

    #[test]
    fn the_servers_own_sentence_is_passed_on() {
        let refused = read_answer(429, r#"{"error":"Today's reports are full. Try again tomorrow."}"#);
        assert_eq!(refused.unwrap_err().0, "Today's reports are full. Try again tomorrow.");
    }

    #[test]
    fn an_answer_with_no_reason_still_says_something() {
        assert_eq!(
            read_answer(502, "<html>bad gateway</html>").unwrap_err().0,
            "The report server answered with an error (502), and gave no reason."
        );
    }
}
