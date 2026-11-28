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
    /// The snapshot the disk was last put back to, for as long as nothing has written to the disk
    /// since. That snapshot already is this disk, so a start from it takes no automatic snapshot of
    /// its own, which would replace the one there is, and offers it instead if the start fails.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub back_to: Option<BackTo>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackTo {
    pub tag: String,
    pub name: String,
    /// When the disk file was last written, in nanoseconds, as going back left it. Any start, and
    /// anything else that writes to the disk, changes it.
    pub written: u128,
}

/// When the disk file was last written, if it can be read.
pub fn disk_written(machine: &Machine) -> Option<u128> {
    let modified = fs::metadata(&machine.disk).and_then(|found| found.modified()).ok()?;
    modified.duration_since(std::time::UNIX_EPOCH).ok().map(|since| since.as_nanos())
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
    if record.back_to.as_ref().is_some_and(|back| back.tag == tag) {
        record.back_to = None;
        record.pending = false;
        let _ = write(context, machine, &record);
    }
}

/// The disk was made anew, as an install does, and the snapshot and the starts this told of went with
/// the old one.
pub fn forget(context: &Context, machine: &Machine) {
    let _ = fs::remove_file(file(context, machine));
}

/// Runs `change`, which writes the disk file but leaves the disk the guest sees as it was, as taking
/// or deleting a snapshot does. A disk that was the snapshot it went back to before is it still after.
pub fn keeping_back_to<T>(context: &Context, machine: &Machine, change: impl FnOnce() -> T) -> T {
    let before = read(context, machine);
    let still = before.back_to.as_ref().is_some_and(|back| disk_written(machine) == Some(back.written));
    let changed = change();
    let mut after = read(context, machine);
    if still && let (Some(back), Some(written)) = (after.back_to.as_mut(), disk_written(machine)) {
        back.written = written;
        let _ = write(context, machine, &after);
    }
    changed
}

/// The disk went back to the snapshot `tag`. Whatever a start had begun on it is undone, and the
/// changes it had met are to be met again, but that snapshot is the way back to this disk, so the
/// automatic snapshot there is stays.
pub fn went_back(context: &Context, machine: &Machine, tag: &str, name: &str) {
    let mut record = read(context, machine);
    record.pending = false;
    record.covered.clear();
    record.back_to =
        disk_written(machine).map(|written| BackTo { tag: tag.to_owned(), name: name.to_owned(), written });
    let _ = write(context, machine, &record);
}
