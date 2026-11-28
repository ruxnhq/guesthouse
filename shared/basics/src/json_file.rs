//! A small JSON file written in one step: the text goes to a file of its own beside it, `NAME.new`,
//! which is then renamed over it, so a crash leaves the old file or the new one and never half of
//! either. The text is pretty JSON with a line end after it.
//!
//! The Linux host writes its machine files, task files, `setup.json` and the rest this way, through
//! `settings::write_json` in `app-core`.

use serde::Serialize;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Replaces the file in one step, so a crash can't leave half of it.
///
/// A value JSON can't hold fails before anything is made, with serde_json's words for why. The
/// folder is made if it isn't there. If the rename fails, `NAME.new` is left beside the file.
pub fn write_json(path: &Path, fields: &impl Serialize) -> io::Result<()> {
    let text = serde_json::to_string_pretty(fields).map_err(|error| io::Error::other(error.to_string()))?;
    fs::create_dir_all(path.parent().unwrap_or(Path::new("/")))?;
    let new = with_suffix(path, ".new");
    fs::write(&new, text + "\n")?;
    fs::rename(&new, path)?;
    Ok(())
}

pub fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut text = path.as_os_str().to_owned();
    text.push(suffix);
    text.into()
}

#[cfg(test)]
mod tests;
