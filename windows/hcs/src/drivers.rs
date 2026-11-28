//! The copy of a card's driver a Windows guest needs. With GPU-PV the host tells the guest's
//! Windows the name of its driver's folder in the host's driver store, and the guest loads the
//! driver's user-mode parts from a folder of that name in its own host driver store. So the folder is
//! copied there whole, under the same name, while the guest's disk is attached to the host.

use crate::error::{Error, Result};
use std::fs;
use std::path::Path;
use std::time::Duration;

/// Where a guest's Windows looks for the drivers its host lends it, from the root of its volume.
pub const HOST_DRIVER_STORE: &str = r"Windows\System32\HostDriverStore\FileRepository";

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Copied {
    pub copied: usize,
    /// Files already there from an earlier copy, with the same size and time.
    pub kept: usize,
}

/// Copies `driver_folder` into the host driver store of the Windows volume at `guest_root`, under
/// the same name. A file already there with the same size and time is left.
pub fn copy_driver(driver_folder: &Path, guest_root: &Path) -> Result<Copied> {
    let name = driver_folder.file_name().ok_or_else(|| {
        Error::new(format!("Copying {}", driver_folder.display()), -1, Some("it has no folder name".into()))
    })?;
    copy_folder(driver_folder, &guest_root.join(HOST_DRIVER_STORE).join(name))
}

/// Copies the folder `from` to `to`, with everything in it. A file already there with the same size
/// and time is left.
pub fn copy_folder(from: &Path, to: &Path) -> Result<Copied> {
    let mut copied = Copied::default();
    copy_tree(from, to, &mut copied)
        .map_err(|error| Error::io(format!("Copying {} to {}", from.display(), to.display()), error))?;
    Ok(copied)
}

fn copy_tree(from: &Path, to: &Path, copied: &mut Copied) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_tree(&entry.path(), &target, copied)?;
        } else if kind.is_file() {
            if same_file(&entry.metadata()?, &target) {
                copied.kept += 1;
            } else {
                // Windows' copy keeps the file's time, which is what the next run compares.
                fs::copy(entry.path(), &target)?;
                copied.copied += 1;
            }
        }
    }
    Ok(())
}

/// FAT keeps a file's time to two seconds, so a copy on a FAT disk is as new as its source if the
/// two times are this close.
const TIME_SLACK: Duration = Duration::from_secs(2);

fn same_file(source: &fs::Metadata, target: &Path) -> bool {
    let Ok(there) = fs::metadata(target) else { return false };
    let (Ok(new), Ok(old)) = (source.modified(), there.modified()) else { return false };
    let apart = new.duration_since(old).or_else(|_| old.duration_since(new)).unwrap_or(Duration::MAX);
    there.len() == source.len() && apart <= TIME_SLACK
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("guesthouse-hcs-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn the_driver_lands_under_its_own_name_and_a_second_copy_keeps_it() {
        let host = scratch("host");
        let folder = host.join("card.inf_amd64_0123456789abcdef");
        fs::create_dir_all(folder.join("sub")).unwrap();
        fs::write(folder.join("a.dll"), b"one").unwrap();
        fs::write(folder.join("sub").join("b.bin"), b"two").unwrap();
        let guest = scratch("guest");

        assert_eq!(copy_driver(&folder, &guest).unwrap(), Copied { copied: 2, kept: 0 });
        let landed = guest.join(HOST_DRIVER_STORE).join("card.inf_amd64_0123456789abcdef");
        assert_eq!(fs::read(landed.join("a.dll")).unwrap(), b"one");
        assert_eq!(fs::read(landed.join("sub").join("b.bin")).unwrap(), b"two");

        assert_eq!(copy_driver(&folder, &guest).unwrap(), Copied { copied: 0, kept: 2 });
        fs::write(folder.join("a.dll"), b"newer").unwrap();
        assert_eq!(copy_driver(&folder, &guest).unwrap(), Copied { copied: 1, kept: 1 });

        let _ = fs::remove_dir_all(host);
        let _ = fs::remove_dir_all(guest);
    }
}
