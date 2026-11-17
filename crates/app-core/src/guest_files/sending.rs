//! Sends what was dropped on the viewer to the guest.

use super::Helper;
use super::names::{for_windows, not_among};
use std::fs::{self, File};
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// One file or folder to send, and its place in the drop, with `/` between folders.
#[derive(Debug, PartialEq, Eq)]
pub struct DroppedFile {
    pub path: PathBuf,
    pub name: String,
    pub is_folder: bool,
}

/// Everything under what was dropped, each folder before what is in it. Links are left out, as
/// they could lead anywhere, and so is whatever can't be read.
pub fn dropped_files(dropped: &[PathBuf]) -> Vec<DroppedFile> {
    let mut files = Vec::new();
    let mut taken = Vec::new();
    for path in dropped {
        let Some(name) = path.file_name() else { continue };
        let name = not_among(for_windows(&name.to_string_lossy()), &taken);
        let before = files.len();
        add(path, name.clone(), &mut files);
        if files.len() > before {
            taken.push(name);
        }
    }
    files
}

fn add(path: &Path, name: String, files: &mut Vec<DroppedFile>) {
    let Ok(kind) = fs::symlink_metadata(path) else { return };
    if kind.is_file() {
        files.push(DroppedFile { path: path.to_owned(), name, is_folder: false });
    } else if kind.is_dir() {
        files.push(DroppedFile { path: path.to_owned(), name: name.clone(), is_folder: true });
        let Ok(entries) = fs::read_dir(path) else { return };
        let mut entries: Vec<PathBuf> = entries.filter_map(|entry| Some(entry.ok()?.path())).collect();
        entries.sort();
        let mut taken = Vec::new();
        for entry in entries {
            let Some(inner) = entry.file_name() else { continue };
            let inner = not_among(for_windows(&inner.to_string_lossy()), &taken);
            taken.push(inner.clone());
            add(&entry, format!("{name}/{inner}"), files);
        }
    }
}

/// Sends the files to the guest as the drop called `drop`. Stops with `Interrupted` once `stop` is
/// set, which is how a drag that left the viewer ends it.
pub fn send(helper: &Helper, drop: &str, files: &[DroppedFile], stop: &AtomicBool) -> io::Result<()> {
    for file in files {
        if stop.load(Ordering::Relaxed) {
            return Err(ErrorKind::Interrupted.into());
        }
        if file.is_folder {
            helper.folder(drop, &file.name)?;
        } else {
            let mut opened = File::open(&file.path)?;
            let length = opened.metadata()?.len();
            helper.put(drop, &file.name, &mut opened, length)?;
        }
    }
    Ok(())
}
