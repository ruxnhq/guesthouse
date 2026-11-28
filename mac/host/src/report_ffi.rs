// SPDX-License-Identifier: GPL-2.0-or-later
//! The C functions for bug reports. Like the rest of the interface they take and give JSON text,
//! and every string they return is freed with `guesthouse_string_free`.
//!
//! The app calls `guesthouse_report_prepare` to make a report and show what is in it, and
//! `guesthouse_report_send` only when the person presses Send.

use crate::report::bundle::{self, Prepared};
use crate::report::collect::{self, Places};
use crate::report::{self, Request, send};
use crate::{error_json, into_c_string, store};
use guesthouse_report_core::contract::NEW_ISSUE;
use guesthouse_report_core::issue::issue_link;
use guesthouse_report_core::summary::Summary;
use serde_json::{Value, json};
use std::ffi::{CStr, c_char};
use std::path::PathBuf;

/// # Safety
/// `text` is null or points to a nul-terminated string.
unsafe fn text<'a>(text: *const c_char) -> Option<&'a str> {
    if text.is_null() {
        return None;
    }
    // SAFETY: the caller promises a nul-terminated string.
    unsafe { CStr::from_ptr(text) }.to_str().ok()
}

fn places() -> Option<Places> {
    Some(Places { support: store::support_folder()?, home: PathBuf::from(std::env::var_os("HOME")?) })
}

fn prepared_json(prepared: &Prepared, warning: Option<String>) -> Value {
    json!({
        "archive": prepared.archive,
        "folder": prepared.folder,
        "files": prepared.files.iter().map(|(name, bytes)| json!({"name": name, "bytes": bytes})).collect::<Vec<_>>(),
        "missing": prepared.missing.iter().map(|(name, why)| json!({"name": name, "reason": why})).collect::<Vec<_>>(),
        "notes": prepared.notes,
        "summary": prepared.summary.facts().collect::<std::collections::BTreeMap<_, _>>(),
        "summary_header": prepared.summary_header,
        "bytes": prepared.bytes,
        "warning": warning,
    })
}

/// Makes a report and writes it in the Reports folder, without sending it. `request` is JSON:
/// `{"machine", "kind", "description", "contact", "include_serial", "machine_running",
/// "app_version", "driver_version"}`, each optional. Returns `{"archive", "folder", "files",
/// "missing", "notes", "summary", "summary_header", "bytes", "warning"}` or `{"error": "..."}`.
///
/// # Safety
/// `request` is null or points to a nul-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guesthouse_report_prepare(request: *const c_char) -> *mut c_char {
    // SAFETY: passed on from the caller.
    let request = unsafe { text(request) };
    into_c_string(match (request.map(serde_json::from_str::<Request>), places()) {
        (Some(Ok(request)), Some(places)) => {
            let found = collect::collect(&request, &places);
            let warning = report::before_sending(
                request.machine.is_some(),
                request.machine_running,
                found.has_run_logs,
                request.problem_kind(),
            );
            match bundle::prepare(&request, &found, &places) {
                Ok(prepared) => prepared_json(&prepared, warning).to_string(),
                Err(error) => error_json(&error),
            }
        }
        (Some(Err(error)), _) => error_json(&format!("the report request is not valid: {error}")),
        _ => error_json("no support folder, or the request is not text"),
    })
}

/// Sends a report made by `guesthouse_report_prepare`: `archive` is its archive and `summary_header`
/// the header it gave. **Call it only when the person has pressed Send.** Returns `{"id": "...",
/// "issue_link": "..."}` or `{"error": "..."}`.
///
/// # Safety
/// Both are null or point to nul-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guesthouse_report_send(
    archive: *const c_char,
    summary_header: *const c_char,
    summary_json: *const c_char,
) -> *mut c_char {
    // SAFETY: passed on from the caller.
    let (archive, header, summary) = unsafe { (text(archive), text(summary_header), text(summary_json)) };
    into_c_string(match archive {
        Some(archive) => match send::send(std::path::Path::new(archive), header.unwrap_or("")) {
            Ok(id) => {
                let link = issue_link(&id, &summary_from_json(summary.unwrap_or("{}")));
                json!({"id": id, "issue_link": link}).to_string()
            }
            Err(error) => error_json(&error),
        },
        None => error_json("the report is not text"),
    })
}

/// The summary again from its JSON, for the link.
fn summary_from_json(text: &str) -> Summary {
    let mut summary = Summary::default();
    if let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(text) {
        for (key, value) in fields {
            if let Some(value) = value.as_str() {
                summary.set(&key, value);
            }
        }
    }
    summary
}

/// Where new issues are opened, for the app to say.
///
/// # Safety
/// No arguments.
#[unsafe(no_mangle)]
pub extern "C" fn guesthouse_report_issue_page() -> *mut c_char {
    into_c_string(json!({"page": NEW_ISSUE}).to_string())
}
