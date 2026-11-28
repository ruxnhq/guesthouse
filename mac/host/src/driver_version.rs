// SPDX-License-Identifier: GPL-2.0-or-later
//! Whether the graphics driver in a Windows machine is the one this Guesthouse comes with: the
//! version the released package says (kept from the last fetch) against the version the machine
//! last reported. The guest does not report its driver's version yet, so this stores one when it
//! does (`record_reported`) and compares what it has. With either unknown it says nothing, and the
//! app shows nothing.

use crate::install::driver::{MANIFEST_NAME, Manifest, kept_package};
use crate::install::plan::{Guest, Step};
use crate::install::state::{self, StepStatus};
use guesthouse_update_check::{is_version, newer};
use serde_json::json;
use std::fs;
use std::path::Path;

/// Where a machine's reported driver version is kept, in its folder.
const REPORTED_FILE: &str = "driver-version.txt";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriverState {
    /// Nothing to say: no released package is known, or the machine is not a Windows install.
    Unknown,
    /// The machine has the released version or a newer one.
    Current,
    /// The machine's driver is older than the released one.
    Old { reported: String, released: String },
    /// The install did not put the driver in (it failed, or was left out) and a released one is known.
    Missing { released: String },
}

/// The version of the newest released package this Mac has kept.
pub fn released_version(support: &Path) -> Option<String> {
    let folder = kept_package(&support.join("Downloads"))?;
    let text = fs::read_to_string(folder.join(MANIFEST_NAME)).ok()?;
    Some(Manifest::parse(&text).ok()?.version)
}

/// What the machine last reported as its driver's version.
pub fn reported_version(machine_folder: &Path) -> Option<String> {
    let text = fs::read_to_string(machine_folder.join(REPORTED_FILE)).ok()?;
    let version = text.trim().to_owned();
    is_version(&version).then_some(version)
}

/// Keeps a version the machine reported. Refuses what is not three numbers.
pub fn record_reported(machine_folder: &Path, version: &str) -> Result<(), String> {
    if !is_version(version) {
        return Err(format!("{version} is not a version (three numbers)."));
    }
    fs::write(machine_folder.join(REPORTED_FILE), format!("{}\n", version.trim()))
        .map_err(|e| format!("Cannot keep the driver's version: {e}"))
}

pub fn state(support: &Path, machine_folder: &Path) -> DriverState {
    let Some(released) = released_version(support) else {
        return DriverState::Unknown;
    };
    if let Some(reported) = reported_version(machine_folder) {
        return if newer(&released, &reported) {
            DriverState::Old { reported, released }
        } else {
            DriverState::Current
        };
    }
    // Nothing reported: a Windows install that did not get its driver in is missing it.
    match state::load(machine_folder) {
        Some(info) if info.guest == Guest::Windows => {
            let driver = info.steps.iter().find(|s| s.step == Step::InstallDriver).map(|s| s.status);
            match driver {
                Some(StepStatus::Failed | StepStatus::Skipped) => DriverState::Missing { released },
                _ => DriverState::Unknown,
            }
        }
        _ => DriverState::Unknown,
    }
}

/// The state as JSON for the app: `state` is `unknown`, `current`, `old` or `missing`, with the
/// versions that are known.
pub fn answer(state: &DriverState) -> String {
    match state {
        DriverState::Unknown => json!({"state": "unknown"}),
        DriverState::Current => json!({"state": "current"}),
        DriverState::Old { reported, released } => {
            json!({"state": "old", "reported": reported, "released": released})
        }
        DriverState::Missing { released } => json!({"state": "missing", "released": released}),
    }
    .to_string()
}

#[cfg(test)]
mod tests;
