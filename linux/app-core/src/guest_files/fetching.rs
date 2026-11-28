//! Fetches what was dragged out of the guest's screen.

use super::names::is_one_name;
use super::{Helper, unpacked};
use std::fs::{self, File};
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// One file or folder of a drag in the guest, by its place in the drag, with `/` between folders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub is_folder: bool,
    pub size: u64,
}

impl Entry {
    /// From the helper's line: the kind, the size and the name in Base64.
    pub(super) fn from_line(line: &str) -> Option<Self> {
        let mut words = line.split(' ');
        let is_folder = match words.next()? {
            "folder" => true,
            "file" => false,
            _ => return None,
        };
        let size = words.next()?.parse().ok()?;
        let name = unpacked(words.next()?)?;
        Some(Self { name, is_folder, size })
    }

    /// Whether this is one of the things that were dragged, and not something inside one.
    pub fn is_dragged_itself(&self) -> bool {
        !self.name.contains('/')
    }

    /// Where it goes under `folder`. The guest chose the name, so it gets no say in anything else:
    /// `None` for a name that would land outside.
    fn place(&self, folder: &Path) -> Option<PathBuf> {
        let mut place = folder.to_owned();
        for part in self.name.split('/') {
            if !is_one_name(part) {
                return None;
            }
            place.push(part);
        }
        Some(place)
    }
}

/// Copies the drag's files into `folder`, which must be new, and returns where the dragged things
/// themselves are. Stops with `Interrupted` once `stop` is set.
pub fn fetch(
    helper: &Helper,
    entries: &[Entry],
    folder: &Path,
    stop: &AtomicBool,
) -> io::Result<Vec<PathBuf>> {
    fs::create_dir_all(folder)?;
    let mut dragged = Vec::new();
    for entry in entries {
        if stop.load(Ordering::Relaxed) {
            return Err(ErrorKind::Interrupted.into());
        }
        let place = entry.place(folder).ok_or(ErrorKind::InvalidData)?;
        if entry.is_folder {
            fs::create_dir_all(&place)?;
        } else {
            helper.get(&entry.name, entry.size, &mut File::create_new(&place)?, stop)?;
        }
        if entry.is_dragged_itself() {
            dragged.push(place);
        }
    }
    Ok(dragged)
}
