//! A machine's files as they're moved: what's in its folder, copied, and deleted, but its own.

use crate::store::OWN_FILES;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

/// Every file under `folder` and its size, and every folder with a size of nothing, by path from it
/// in lowercase and with `/`, except what's a machine's own at the top. A link is an error, not
/// followed: a machine's folder holds none, so one is something else's.
pub(super) fn tree(folder: &Path) -> Result<BTreeMap<String, u64>, String> {
    fn walk(folder: &Path, prefix: &str, top: bool, found: &mut BTreeMap<String, u64>) -> Result<(), String> {
        let said = |error: io::Error| format!("{}: {error}", folder.display());
        for entry in fs::read_dir(folder).map_err(said)? {
            let entry = entry.map_err(|error| format!("{}: {error}", folder.display()))?;
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if top && OWN_FILES.iter().any(|own| own.eq_ignore_ascii_case(&name)) {
                continue;
            }
            let kind = entry.file_type().map_err(|error| format!("{}: {error}", entry.path().display()))?;
            if kind.is_symlink() {
                return Err(format!(
                    "{} is a link, and the service doesn't move links.",
                    entry.path().display()
                ));
            }
            if kind.is_dir() {
                found.insert(format!("{prefix}{name}/"), 0);
                walk(&entry.path(), &format!("{prefix}{name}/"), false, found)?;
            } else {
                let size =
                    entry.metadata().map_err(|error| format!("{}: {error}", entry.path().display()))?.len();
                found.insert(format!("{prefix}{name}"), size);
            }
        }
        Ok(())
    }
    let mut found = BTreeMap::new();
    walk(folder, "", true, &mut found)?;
    Ok(found)
}

/// Copies what's in `from` into `to`, which is there, but a machine's own files at the top. Each file
/// that's copied is told to `done` by its place and its size.
pub(super) fn copy_tree(
    from: &Path,
    to: &Path,
    top: bool,
    done: &mut dyn FnMut(&str, u64),
) -> Result<(), String> {
    let entries = fs::read_dir(from).map_err(|error| format!("{}: {error}", from.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", from.display()))?;
        let name = entry.file_name();
        if top && OWN_FILES.iter().any(|own| own.eq_ignore_ascii_case(&name.to_string_lossy())) {
            continue;
        }
        let (source, copy) = (entry.path(), to.join(&name));
        let kind = entry.file_type().map_err(|error| format!("{}: {error}", source.display()))?;
        if kind.is_symlink() {
            return Err(format!("{} is a link, and the service doesn't move links.", source.display()));
        }
        if kind.is_dir() {
            fs::create_dir(&copy).map_err(|error| format!("{}: {error}", copy.display()))?;
            copy_tree(&source, &copy, false, done)?;
        } else {
            let bytes = fs::copy(&source, &copy)
                .map_err(|error| format!("Copying {} to {}: {error}", source.display(), copy.display()))?;
            done(&name.to_string_lossy(), bytes);
        }
    }
    Ok(())
}

/// Deletes what's in a machine's own folder that isn't its own: its disks and what's beside them.
pub(super) fn remove_disks(folder: &Path) -> Result<(), String> {
    let entries = fs::read_dir(folder).map_err(|error| format!("{}: {error}", folder.display()))?;
    for entry in entries.filter_map(Result::ok) {
        if OWN_FILES.iter().any(|own| own.eq_ignore_ascii_case(&entry.file_name().to_string_lossy())) {
            continue;
        }
        let path = entry.path();
        let gone = match entry.file_type() {
            Ok(kind) if kind.is_dir() => fs::remove_dir_all(&path),
            _ => fs::remove_file(&path),
        };
        gone.map_err(|error| format!("{}: {error}", path.display()))?;
    }
    Ok(())
}
