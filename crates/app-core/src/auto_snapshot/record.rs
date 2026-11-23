//! What the machine's folder remembers of its automatic snapshot: which one it is, whether the start
//! that followed it has finished, and which changes a finished start has already seen through.

use crate::context::Context;
use crate::machine::Machine;
use crate::settings::write_json;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

pub const RECORD_FILE: &str = "auto-snapshot.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// The tag of the snapshot taken before the last start that needed one. Empty if there is none.
    #[serde(default)]
    pub tag: String,
    #[serde(default)]
    pub name: String,
    /// A start has begun on that snapshot's disk and has not succeeded. While it is, the disk may
    /// already hold part of a change, so the snapshot is the last disk known to have worked and is
    /// not replaced.
    #[serde(default)]
    pub pending: bool,
    /// The changes ([`super::risks::Pending::signature`]) a successful start has already met. A start
    /// that meets only these takes no snapshot.
    #[serde(default)]
    pub covered: Vec<String>,
}

fn file(context: &Context, machine: &Machine) -> PathBuf {
    machine.folder(context).join(RECORD_FILE)
}

/// A missing or unreadable file reads as no record, which only means the next start that has a
/// change to make takes a snapshot.
pub fn read(context: &Context, machine: &Machine) -> Record {
    let text = fs::read_to_string(file(context, machine)).unwrap_or_default();
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn write(context: &Context, machine: &Machine, record: &Record) -> crate::failure::Result<()> {
    write_json(&file(context, machine), record)
}

/// The snapshot with this tag has gone, so there is nothing to go back to.
pub fn forget_tag(context: &Context, machine: &Machine, tag: &str) {
    let mut record = read(context, machine);
    if record.tag == tag {
        record.tag.clear();
        record.name.clear();
        record.pending = false;
        let _ = write(context, machine, &record);
    }
}

/// The disk was made anew, as an install does, and the snapshot and the starts this told of went with
/// the old one.
pub fn forget(context: &Context, machine: &Machine) {
    let _ = fs::remove_file(file(context, machine));
}

/// The disk went back to some snapshot. Whatever a start had begun on it is undone, and the changes
/// it had met are to be met again.
pub fn disk_changed(context: &Context, machine: &Machine) {
    let mut record = read(context, machine);
    if record.pending || !record.covered.is_empty() {
        record.pending = false;
        record.covered.clear();
        let _ = write(context, machine, &record);
    }
}
