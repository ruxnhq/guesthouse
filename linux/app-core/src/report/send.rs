//! Sends a written report to the server with the system's `curl`, which `install.sh` already needs, so
//! the workspace needs no HTTP of its own.

use super::bundle::Bundle;
use super::endpoint::{MOST_REPORT_BYTES, report_address};
use crate::failure::{Failure, Result};
use guesthouse_report_core::send::{curl_arguments, read_reply};
use std::fs;
use std::process::Command;

/// Sends the report, and gives back the ID the server gave it. On failure, the server's own sentence
/// if it gave one. The arguments and the reading of curl's answer are every host's.
pub fn send(bundle: &Bundle) -> Result<String> {
    let size = fs::metadata(&bundle.path)?.len();
    if size > MOST_REPORT_BYTES {
        return Err(Failure::new("The report is too big to send. Save it to a file instead."));
    }
    let address = report_address();
    let answer_file = bundle.path.with_extension("answer");
    let mut curl = Command::new("curl");
    curl.args(curl_arguments(&address, &bundle.summary.header(), &bundle.path, &answer_file));
    let result = curl.output();
    let answer = fs::read_to_string(&answer_file).unwrap_or_default();
    let _ = fs::remove_file(&answer_file);
    let result =
        result.map_err(|error| Failure(format!("curl could not run, so nothing was sent: {error}")))?;
    read_reply(
        &String::from_utf8_lossy(&result.stdout),
        &String::from_utf8_lossy(&result.stderr),
        result.status.success(),
        &answer,
    )
    .map_err(Failure)
}

/// The ID if the server took the report, read as every host reads it.
#[cfg(test)]
fn read_answer(status: u16, answer: &str) -> Result<String> {
    guesthouse_report_core::answer::read_answer(status, answer).map_err(Failure)
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
