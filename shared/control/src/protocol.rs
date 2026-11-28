//! The pipe's version, which an app and a service must share. Each request carries it beside its
//! words, as `"protocol": 2`, and the service refuses a request without it, or with another, in
//! words an app of any version shows. The service's `Status` answer carries it too, and the app reads
//! it before the rest, so it can tell a service of another version from one it can't read.
//!
//! The pipe had no version before 2, which is when the machines' states became the ones every host
//! shares: an app and a service from before would read each other's states as garbled. A line with
//! no version reads as 1. Nothing older than this is read in any other way.

use crate::Request;
use serde_json::Value;

/// The pipe's version. It goes up whenever an app and a service of different versions would
/// misread each other.
pub const PROTOCOL: u32 = 2;
/// What a line with no version is.
pub(crate) const BEFORE: u32 = 1;
const KEY: &str = "protocol";

/// What the app shows when it and the service aren't the same version, whichever is older.
pub const OTHER_VERSION: &str = "This app and the Guesthouse service are different versions. Update Guesthouse, \
                                 or install it again, so both are the same.";

/// What the service says to a line that isn't a request it knows.
pub const NOT_A_REQUEST: &str =
    "That isn't a request the service knows. The app and the service may be different versions.";

/// The version a line says, or 1 if it says none.
fn version(line: &Value) -> u32 {
    line.get(KEY).and_then(Value::as_u64).and_then(|number| u32::try_from(number).ok()).unwrap_or(BEFORE)
}

/// `request` as the app writes it on the pipe: its words, with the pipe's version beside them. A
/// service from before the version reads the words as they are and passes over the version.
pub fn request_line(request: &Request) -> Value {
    let mut line = serde_json::to_value(request).expect("a request is always JSON");
    if let Value::Object(fields) = &mut line {
        fields.insert(KEY.to_owned(), PROTOCOL.into());
    }
    line
}

/// The request in a line the service read, or what to refuse it with: an app of another version,
/// or a line that isn't a request.
pub fn request_from(line: Value) -> Result<Request, String> {
    if version(&line) != PROTOCOL {
        return Err(OTHER_VERSION.to_owned());
    }
    serde_json::from_value(line).map_err(|_| NOT_A_REQUEST.to_owned())
}

/// Whether a line the app read comes from a service of another version. Only `Status` says the
/// version, and the app asks for it first and every two seconds after.
pub fn from_other_version(line: &Value) -> bool {
    line.get("answer").and_then(Value::as_str) == Some("status") && version(line) != PROTOCOL
}

#[cfg(test)]
mod tests;
