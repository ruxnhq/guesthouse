//! What `guesthouse-check` says of this PC, and the part of `guesthouse-card-check` that only reads
//! the NVIDIA driver's files. Card-check's survey and channel test open the card, so they stay out.

use super::Collection;
use crate::context::Context;
use crate::setup::checks::{CheckState, check_host};
use guesthouse_backend_nvidia::profile::{host_branch, host_branch_of_version};
use guesthouse_card_check::host::{no_host_text, read_host};
use guesthouse_card_check::report::{Branch, host_lines};

pub fn collect(context: &Context, found: &mut Collection) {
    let report = check_host(context);
    let mut text = String::new();
    for check in &report.checks {
        let state = match check.state {
            CheckState::Ok => "ok",
            CheckState::Warning => "warning",
            CheckState::Failed => "failed",
        };
        text.push_str(&format!("[{state}] {}: {}\n", check.title, check.text));
        if !check.fix.is_empty() && check.state != CheckState::Ok {
            text.push_str(&format!("    fix: {}\n", check.fix));
        }
    }
    text.push_str(&format!("GPU machines possible: {}\n", if report.gpu { "yes" } else { "no" }));
    found.add("host/checks.txt", text);
    found.add("host/card-check.txt", card_check_text());
}

fn card_check_text() -> String {
    let host = match read_host() {
        Ok(host) => host,
        Err(why) => return format!("{}\n", no_host_text(&why)),
    };
    let branch = match host_branch(host_branch_of_version(&host.version)) {
        Some(profile) => {
            Branch::Known { version_tested: profile.tested_versions.contains(&host.version.as_str()) }
        }
        None => Branch::Unknown,
    };
    host_lines(&host, branch)
}
