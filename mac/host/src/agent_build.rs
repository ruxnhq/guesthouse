// SPDX-License-Identifier: GPL-2.0-or-later
//! Which build an agent is, and whether it is the app's own. The app and its agent are built
//! together, but a person replaces yesterday's app with today's while yesterday's agent may still
//! be running (launchd keeps it, and it stays while it has machines). The app must not take
//! whatever agent answers, so the agent says which build it is and the app compares.
//!
//! The identity is the bundle's version and build number, the agent program's real path, and the
//! program's size and time of change, all read when the agent started. The version and build say
//! what to call it ("0.1.20261128 (20261128)"); the path tells a copy in another folder; the size
//! and time tell a build that replaced the file in place, and a developer's build, which has the
//! same version every time. Both the agent (of itself) and the app (of the agent program in its
//! own bundle) make it with `identity_of`, so the two cannot disagree on how.
//!
//! What to do about a different agent is `judge`: facts in, a verdict out, like `start_checks`.

use crate::protocol::VERSION;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

/// A build of the agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub version: String,
    pub build: String,
    /// The agent program's real path (symbolic links followed).
    pub path: String,
    /// Its size and time of change, as `SIZE.SECONDS`.
    pub stamp: String,
}

impl Identity {
    /// What the person is told it is: `0.1.20261128 (20261128)`.
    pub fn label(&self) -> String {
        format!("{} ({})", self.version, self.build)
    }
}

/// The identity of the agent program at `program`, for a bundle of `version` and `build`; none
/// when the file is not there.
pub fn identity_of(program: &Path, version: &str, build: &str) -> Option<Identity> {
    let real = fs::canonicalize(program).ok()?;
    let meta = fs::metadata(&real).ok()?;
    let seconds = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(Identity {
        version: version.to_string(),
        build: build.to_string(),
        path: real.to_str()?.to_string(),
        stamp: format!("{}.{seconds}", meta.len()),
    })
}

/// The states of a machine that is not running: it holds nothing that stopping the agent loses.
const NOT_RUNNING: [&str; 2] = ["off", "failed"];

fn name_of(entry: &Value) -> Option<String> {
    entry.get("machine").and_then(Value::as_str).map(String::from)
}

/// The entries of the list in `answer[key]`.
fn entries<'a>(answer: &'a Value, key: &str) -> impl Iterator<Item = &'a Value> {
    answer.get(key).and_then(Value::as_array).into_iter().flatten()
}

/// What to do about the agent that answered a listing. `expected` is the identity of the app's
/// own agent, `answer` the agent's whole answer to `list`, which may come from a build with no
/// identity in it at all.
///
/// - `own`: it is this app's agent; carry on.
/// - `replace`: another build, with nothing running in it; it can be replaced without asking.
/// - `busy`: another build with machines (or installs) running, named in `running`, those being
///   installed also in `installing`. `can_continue` says whether this app can work against it for
///   now (it speaks the same protocol).
pub fn judge(expected: &Identity, answer: &Value) -> Value {
    let reported: Option<Identity> =
        answer.get("agent").and_then(|agent| serde_json::from_value(agent.clone()).ok());
    if reported.as_ref() == Some(expected) {
        return json!({"verdict": "own"});
    }
    let mut running: Vec<String> = entries(answer, "machines")
        .filter(|machine| {
            let state = machine.get("state").and_then(Value::as_str).unwrap_or("off");
            !NOT_RUNNING.contains(&state)
        })
        .filter_map(name_of)
        .collect();
    // An install in progress is a machine being made; stopping the agent would end it.
    let installing: Vec<String> = entries(answer, "installs")
        .filter(|install| install.get("state").and_then(Value::as_str) == Some("running"))
        .filter_map(name_of)
        .collect();
    for machine in &installing {
        if !running.contains(machine) {
            running.push(machine.clone());
        }
    }
    let older = reported.map_or_else(|| "an older build".to_string(), |identity| identity.label());
    if running.is_empty() {
        return json!({"verdict": "replace", "older": older});
    }
    let same_protocol = answer.get("v").and_then(Value::as_u64) == Some(u64::from(VERSION));
    json!({"verdict": "busy", "older": older, "running": running, "installing": installing,
        "can_continue": same_protocol})
}

/// From the app's JSON request to the answer's. `{"what": "identity", "program", "version",
/// "build"}` gives `{"identity": {...}}` (or `{"identity": null}`); `{"what": "judge",
/// "expected", "answer"}` gives the verdict.
pub fn answer(request: &str) -> String {
    let Ok(request) = serde_json::from_str::<Value>(request) else {
        return json!({"error": "the request is not JSON"}).to_string();
    };
    let text = |key: &str| request.get(key).and_then(Value::as_str);
    match text("what") {
        Some("identity") => {
            let found = identity_of(
                Path::new(text("program").unwrap_or_default()),
                text("version").unwrap_or_default(),
                text("build").unwrap_or_default(),
            );
            json!({"identity": found}).to_string()
        }
        Some("judge") => {
            let expected = request.get("expected").and_then(|e| serde_json::from_value(e.clone()).ok());
            match (expected, request.get("answer")) {
                (Some(expected), Some(answer)) => judge(&expected, answer).to_string(),
                _ => json!({"error": "the judging needs `expected` and `answer`"}).to_string(),
            }
        }
        _ => json!({"error": "the request is not one the agent's build answers"}).to_string(),
    }
}

#[cfg(test)]
mod tests;
