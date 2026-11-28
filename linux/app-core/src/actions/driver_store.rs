//! The guest's copy of a driver the host lends it, in Windows' host driver store: what it holds, and
//! what it lacks of the driver's folder on this PC.

use super::Task;
use super::guest_upload::quoted;
use crate::failure::Result;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Where Windows keeps a driver the host lends it, by the folder name the daemon gives.
pub(super) const HOST_DRIVER_STORE: &str = "C:\\Windows\\System32\\HostDriverStore\\FileRepository";
const LIST_SECONDS: u64 = 20;
pub(super) const LISTED_MARK: &str = "all listed";

/// The names of the files in the guest's copy of the driver, in lower case as Windows ignores case,
/// or `None` if the guest didn't say.
pub(super) fn guest_names(task: &Task, store: &str) -> Option<HashSet<String>> {
    let script = format!(
        "Get-ChildItem -LiteralPath {} -File -ea 0 | ForEach-Object Name; '{LISTED_MARK}'",
        quoted(store)
    );
    let said = task.ask_guest(&script, LIST_SECONDS);
    let lines: Vec<&str> = said.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    let (last, names) = lines.split_last()?;
    (*last == LISTED_MARK).then(|| names.iter().map(|name| name.to_lowercase()).collect())
}

/// The files of the driver's folder on this PC that the guest's copy lacks.
pub(super) fn missing_in_guest(folder: &Path, in_guest: &HashSet<String>) -> Result<Vec<PathBuf>> {
    let mut missing: Vec<PathBuf> = fs::read_dir(folder)?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| {
            let name = path.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
            !in_guest.contains(&name)
        })
        .collect();
    missing.sort();
    Ok(missing)
}
