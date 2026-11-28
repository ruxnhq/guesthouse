// SPDX-License-Identifier: GPL-2.0-or-later
//! Sends a written report to the server with the system's `curl`, as the other hosts do, and reads
//! the answer. The arguments and the reading are the shared crate's (and tested there against the
//! Linux host's own); running curl is this file's.

use guesthouse_report_core::contract::{MOST_REPORT_BYTES, report_address};
use guesthouse_report_core::send::{curl_arguments, read_reply, summary_file, write_summary_file};
use std::fs;
use std::path::Path;
use std::process::Command;

/// Sends the archive with its summary header and gives back the ID the server gave it, or why not.
/// **Only the app's Send button calls this**, after the person has seen what is in the report.
pub fn send(archive: &Path, summary_header: &str) -> Result<String, String> {
    let size = fs::metadata(archive).map_err(|e| e.to_string())?.len();
    if size > MOST_REPORT_BYTES {
        return Err("The report is too big to send. Save it to a file instead.".into());
    }
    let answer_file = archive.with_extension("answer");
    let _ = fs::remove_file(summary_file(archive));
    write_summary_file(archive, summary_header).map_err(|e| e.to_string())?;
    let output = Command::new("curl")
        .args(curl_arguments(&report_address(), summary_header, archive, &answer_file))
        .output();
    let body = fs::read_to_string(&answer_file).unwrap_or_default();
    let _ = fs::remove_file(&answer_file);
    let _ = fs::remove_file(summary_file(archive));
    let output = output.map_err(|e| format!("curl could not run, so nothing was sent: {e}"))?;
    read_reply(
        &String::from_utf8_lossy(&output.stdout),
        &String::from_utf8_lossy(&output.stderr),
        output.status.success(),
        &body,
    )
}
