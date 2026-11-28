//! The Linux host's snapshot rules. A snapshot is kept inside the machine's qcow2 disk under a tag of
//! ours, with files beside the disk named after the tag; the one ending `.json` says what the snapshot
//! is called and whether Guesthouse took it by itself.

use serde_json::{Value, json};

/// A name inside the qcow2 of ours, as qemu-img would read a name like 1 as a snapshot's number.
pub const SNAPSHOT_TAG: &str = "guesthouse-";
pub const MAX_SNAPSHOT_NAME: usize = 60;
/// The random bytes a new tag has after [`SNAPSHOT_TAG`], written as hex.
pub const TAG_RANDOM_BYTES: usize = 4;
/// The hex digits after [`SNAPSHOT_TAG`] in a tag an import takes.
pub const SNAPSHOT_HEX_DIGITS: usize = 8;
/// Said when a snapshot the person names would have a name another of the machine's snapshots has.
pub const NAME_TAKEN: &str = "There is already a snapshot with that name.";

/// Whether `name` is a tag this host gives a snapshot: [`SNAPSHOT_TAG`], then
/// [`SNAPSHOT_HEX_DIGITS`] lower-case hex digits.
pub fn is_snapshot_tag(name: &str) -> bool {
    name.strip_prefix(SNAPSHOT_TAG).is_some_and(|rest| {
        rest.len() == SNAPSHOT_HEX_DIGITS
            && rest.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

/// The name a snapshot is given, trimmed, or the sentence that refuses it: it needs one of up to
/// [`MAX_SNAPSHOT_NAME`] characters, whatever they are.
pub fn checked_name(name: &str) -> Result<&str, String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_SNAPSHOT_NAME {
        return Err(format!("A snapshot needs a name of up to {MAX_SNAPSHOT_NAME} characters."));
    }
    Ok(name)
}

/// Whether `name`, already checked, is one of `names`, the names of the machine's snapshots. A
/// snapshot the person names may not repeat one, which [`NAME_TAKEN`] says; one Guesthouse takes by
/// itself may, as the name is for reading and the tag is what tells them apart.
pub fn name_taken<'a>(name: &str, names: impl IntoIterator<Item = &'a str>) -> bool {
    names.into_iter().any(|taken| taken == name)
}

/// What is written in the file beside a snapshot: its name, whether Guesthouse took it by itself,
/// and whether the machine's disk was on NVMe then.
pub fn note_text(name: &str, automatic: bool, nvme: bool) -> String {
    json!({"name": name, "auto": automatic, "nvme": nvme}).to_string()
}

/// What is read from the file beside a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    /// Empty when the file says none.
    pub name: String,
    pub automatic: bool,
}

/// The file beside a snapshot as it is read when the snapshots are listed: none if it isn't JSON, and
/// a name or a mark that is missing or of another kind reads as empty or as not taken by itself.
pub fn read_note(text: &str) -> Option<Note> {
    let saved: Value = serde_json::from_str(text).ok()?;
    Some(Note {
        name: saved["name"].as_str().unwrap_or_default().to_owned(),
        automatic: saved["auto"].as_bool().unwrap_or(false),
    })
}

#[cfg(test)]
mod tests;
