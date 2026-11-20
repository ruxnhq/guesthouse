//! Copies the app makes its own. On NixOS the program folder and the firmware are in the store, where
//! every file is read-only, and a plain copy stays read-only. A copy QEMU writes to, or that the app
//! copies over again later, has to be writable by this user.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const OWNER_WRITES: u32 = 0o200;

/// Copies `from` to `to`, in place of any copy made before, and lets this user write the new one.
pub fn copy_writable(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    let to = to.as_ref();
    // A read-only copy can't be opened to be written over, but it can be removed.
    match fs::remove_file(to) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    fs::copy(from, to)?;
    let mut permissions = fs::metadata(to)?.permissions();
    permissions.set_mode(permissions.mode() | OWNER_WRITES);
    fs::set_permissions(to, permissions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;

    const READ_ONLY: u32 = 0o444;

    fn read_only(scratch: &Scratch, name: &str, text: &str) -> std::path::PathBuf {
        let file = scratch.write(name, text);
        fs::set_permissions(&file, fs::Permissions::from_mode(READ_ONLY)).unwrap();
        file
    }

    #[test]
    fn a_copy_from_the_store_can_be_written() {
        let scratch = Scratch::new();
        let template = read_only(&scratch, "store/OVMF_VARS.fd", "blank firmware settings");
        let vars = scratch.path("vars.fd");
        copy_writable(&template, &vars).unwrap();
        assert_eq!(fs::read_to_string(&vars).unwrap(), "blank firmware settings");
        fs::write(&vars, "settings QEMU wrote").unwrap();
    }

    #[test]
    fn a_read_only_copy_from_before_is_replaced() {
        let scratch = Scratch::new();
        let template = read_only(&scratch, "store/meta-data", "new");
        let copy = read_only(&scratch, "seed/meta-data", "old");
        copy_writable(&template, &copy).unwrap();
        assert_eq!(fs::read_to_string(&copy).unwrap(), "new");
    }
}
