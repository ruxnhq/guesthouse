//! What the report server answered. It says `{"id": ...}` when it took the report, and
//! `{"error": ...}` with a sentence when it didn't.

use super::contract::{CREATED, ID_LENGTH};
use serde::Deserialize;

#[derive(Deserialize)]
struct Answer {
    id: Option<String>,
    error: Option<String>,
}

/// The report's ID if the server took it, and otherwise a sentence for the tester: the server's own
/// if it gave one.
pub fn read_answer(status: u16, body: &str) -> Result<String, String> {
    let parsed: Option<Answer> = serde_json::from_str(body).ok();
    if status == CREATED {
        let id = parsed.and_then(|answer| answer.id).unwrap_or_default();
        if id.len() == ID_LENGTH && id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Ok(id);
        }
        return Err("The report server took the report but gave no ID it could be found by.".to_owned());
    }
    match parsed.and_then(|answer| answer.error).filter(|error| !error.trim().is_empty()) {
        Some(error) => Err(error),
        None => Err(format!("The report server answered with an error ({status}), and gave no reason.")),
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
        let said = "The report server took the report but gave no ID it could be found by.";
        assert_eq!(read_answer(201, r#"{"id":"../../etc"}"#).unwrap_err(), said);
        assert_eq!(read_answer(201, r#"{"id":"0a1b2c3d4"}"#).unwrap_err(), said);
        assert_eq!(read_answer(201, r#"{"id":"0a1b2c3d4g"}"#).unwrap_err(), said);
        assert_eq!(read_answer(201, "not json").unwrap_err(), said);
        assert_eq!(read_answer(201, "{}").unwrap_err(), said);
    }

    #[test]
    fn the_servers_own_sentence_is_passed_on() {
        let refused = read_answer(429, r#"{"error":"Today's reports are full. Try again tomorrow."}"#);
        assert_eq!(refused.unwrap_err(), "Today's reports are full. Try again tomorrow.");
    }

    #[test]
    fn an_answer_with_no_reason_still_says_something() {
        let said = "The report server answered with an error (502), and gave no reason.";
        assert_eq!(read_answer(502, "<html>bad gateway</html>").unwrap_err(), said);
        assert_eq!(read_answer(502, r#"{"error":"  "}"#).unwrap_err(), said);
        assert_eq!(
            read_answer(500, "{}").unwrap_err(),
            "The report server answered with an error (500), and gave no reason."
        );
    }

    #[test]
    fn an_id_with_another_status_is_no_success() {
        assert!(read_answer(200, r#"{"id":"0a1b2c3d4e"}"#).is_err());
    }
}
