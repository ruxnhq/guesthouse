//! The Windows host's snapshot rules. A stopped machine's snapshots are a chain of disks, and
//! `snapshots/index.json` beside the machine's disk lists them: each snapshot's ID, label, when it was
//! taken and the snapshot its disk is made on, the one the machine's disk is made on, the next ID, and a
//! snapshot being taken until it's done. Here are the index as it's kept, what is read from it, and
//! the label and count rules; the disks and the files stay with the host.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The folder beside a machine's disk that holds its snapshots.
pub const FOLDER: &str = "snapshots";
pub const INDEX: &str = "index.json";
/// The index is written here, then renamed over [`INDEX`].
pub const NEW_INDEX: &str = "index.new.json";
/// The most snapshots a machine shows, and the longest a label is.
pub const MOST: usize = 32;
pub const LONGEST_LABEL: usize = 60;

/// A snapshot as the app is told of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub id: u64,
    pub label: String,
    /// When it was taken, in seconds since 1970 in UTC.
    pub taken: u64,
    /// Whether the machine's disk is made on it, as it is just after it's taken or gone back to.
    pub current: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    #[serde(default)]
    pub snapshots: Vec<Entry>,
    /// The snapshot the machine's disk is made on, if any.
    #[serde(default)]
    pub current: Option<u64>,
    #[serde(default)]
    pub next_id: u64,
    /// A snapshot being taken, until it's done, so a crash between moving the disk and making the new
    /// one can be put right.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub taking: Option<Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub id: u64,
    pub label: String,
    pub taken: u64,
    /// The snapshot its disk is made on, if any.
    pub parent: Option<u64>,
    /// Deleted, and kept only while a disk is made on it.
    #[serde(default)]
    pub hidden: bool,
}

/// The folder of the snapshots of the machine whose disk is `disk`.
pub fn folder(disk: &Path) -> PathBuf {
    disk.parent().unwrap_or(Path::new(".")).join(FOLDER)
}

/// A snapshot's disk. Its firmware state and record sit beside it, named as the machine's are
/// beside its disk.
pub fn snapshot_disk(disk: &Path, id: u64) -> PathBuf {
    folder(disk).join(format!("{id}.vhdx"))
}

/// The index from its file's text. A field it doesn't know is passed over, and one it lacks is empty.
pub fn read_index(text: &str) -> Result<Index, serde_json::Error> {
    serde_json::from_str(text)
}

/// The index's file's text.
pub fn index_text(index: &Index) -> String {
    let text = serde_json::to_string_pretty(index).expect("an index always serialises");
    text + "\n"
}

/// The machine's snapshots in `index`, oldest first, without those deleted.
pub fn shown(index: &Index) -> Vec<Snapshot> {
    index
        .snapshots
        .iter()
        .filter(|entry| !entry.hidden)
        .map(|entry| Snapshot {
            id: entry.id,
            label: entry.label.clone(),
            taken: entry.taken,
            current: index.current == Some(entry.id),
        })
        .collect()
}

/// The disks the machine's disk `disk` is made on by `index`, nearest first.
pub fn chain_in(index: &Index, disk: &Path) -> Result<Vec<PathBuf>, String> {
    let mut disks = Vec::new();
    let mut next = index.current;
    while let Some(id) = next {
        let entry = index
            .snapshots
            .iter()
            .find(|entry| entry.id == id)
            .ok_or_else(|| format!("The snapshot index names a snapshot {id} it doesn't have."))?;
        disks.push(snapshot_disk(disk, id));
        next = entry.parent;
        if disks.len() > index.snapshots.len() {
            return Err("The snapshot index goes round in a circle.".into());
        }
    }
    Ok(disks)
}

/// Every disk by `index` that's made on another, each with the one it's made on: each snapshot's that
/// has a parent, then the machine's own disk `disk`.
pub fn links_in(index: &Index, disk: &Path) -> Vec<(PathBuf, PathBuf)> {
    let mut links: Vec<(PathBuf, PathBuf)> = index
        .snapshots
        .iter()
        .filter_map(|entry| Some((snapshot_disk(disk, entry.id), snapshot_disk(disk, entry.parent?))))
        .collect();
    if let Some(current) = index.current {
        links.push((disk.to_owned(), snapshot_disk(disk, current)));
    }
    links
}

/// The label a snapshot is given, trimmed, or the sentence that refuses it: 1 to [`LONGEST_LABEL`]
/// characters, none of them a control character. Labels may repeat.
pub fn checked_label(label: &str) -> Result<&str, String> {
    let label = label.trim();
    if label.is_empty() || label.chars().count() > LONGEST_LABEL || label.chars().any(char::is_control) {
        return Err(format!("A snapshot's label is 1 to {LONGEST_LABEL} characters on one line."));
    }
    Ok(label)
}

/// Why the machine called `name`, which shows `count` snapshots, can't have another: it has [`MOST`].
pub fn count_refusal(name: &str, count: usize) -> Option<String> {
    (count >= MOST).then(|| format!("{name} has {MOST} snapshots, the most a machine has. Delete one first."))
}

#[cfg(test)]
mod tests;
