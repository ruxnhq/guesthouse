//! A qcow2's own header, read here before qemu-img opens a disk from elsewhere. Opening one follows
//! the files it names, a backing file or a file its data is kept in, which could be any file of this
//! PC's, so a disk that names one is refused before anything opens it.
//!
//! The fields are QEMU's qcow2 format (docs/interop/qcow2.txt there), all big-endian.

use crate::failure::{Failure, Result};
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// What every qcow2 begins with.
pub const QCOW2_MAGIC: [u8; 4] = *b"QFI\xfb";
const VERSION_AT: usize = 4;
const BACKING_FILE_OFFSET_AT: usize = 8;
const CRYPT_METHOD_AT: usize = 32;
const INCOMPATIBLE_FEATURES_AT: usize = 72;
/// A version 2 header ends before the feature bits, and version 3 adds them.
const VERSION_2: u32 = 2;
const VERSION_3: u32 = 3;
const HEADER_BYTES: usize = 104;
/// The incompatible feature bit for data kept in a file of its own.
const EXTERNAL_DATA_FILE: u64 = 1 << 2;
const NOT_A_DISK: &str = "The disk in the export isn't a qcow2 disk this app can use.";

fn u32_at(header: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(header[at..at + 4].try_into().unwrap_or_default())
}

fn u64_at(header: &[u8], at: usize) -> u64 {
    u64::from_be_bytes(header[at..at + 8].try_into().unwrap_or_default())
}

/// Whether the file begins as a qcow2 does, whatever it holds after.
pub fn looks_like_qcow2(path: &Path) -> bool {
    let mut magic = [0; QCOW2_MAGIC.len()];
    File::open(path).and_then(|mut file| file.read_exact(&mut magic)).is_ok() && magic == QCOW2_MAGIC
}

/// Refuses a qcow2 that names another file or is encrypted, from its header alone.
pub fn check_stands_alone(path: &Path) -> Result<()> {
    let mut header = vec![0; HEADER_BYTES];
    let mut file = File::open(path)?;
    let read = file.read(&mut header)?;
    header.truncate(read);
    if header.len() < INCOMPATIBLE_FEATURES_AT || header[..QCOW2_MAGIC.len()] != QCOW2_MAGIC {
        return Err(Failure::new(NOT_A_DISK));
    }
    let version = u32_at(&header, VERSION_AT);
    if version != VERSION_2 && version != VERSION_3 {
        return Err(Failure::new(NOT_A_DISK));
    }
    if u64_at(&header, BACKING_FILE_OFFSET_AT) != 0 {
        return Err(Failure::new(
            "A disk in the export is built on another file, which an import can't take.",
        ));
    }
    if u32_at(&header, CRYPT_METHOD_AT) != 0 {
        return Err(Failure::new("A disk in the export is encrypted, which an import can't take."));
    }
    if version == VERSION_3 {
        if header.len() < INCOMPATIBLE_FEATURES_AT + 8 {
            return Err(Failure::new(NOT_A_DISK));
        }
        if u64_at(&header, INCOMPATIBLE_FEATURES_AT) & EXTERNAL_DATA_FILE != 0 {
            return Err(Failure::new(
                "A disk in the export keeps its data in another file, which an import can't take.",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;
    use std::process::Command;

    fn made(scratch: &Scratch, name: &str, options: &[&str]) -> std::path::PathBuf {
        let path = scratch.path(name);
        let done = Command::new("qemu-img")
            .args(["create", "-q", "-f", "qcow2"])
            .args(options)
            .arg(&path)
            .arg("64M")
            .status();
        assert!(done.unwrap().success());
        path
    }

    #[test]
    fn a_disk_that_names_no_other_file_stands_alone() {
        let scratch = Scratch::new();
        for options in [&[][..], &["-o", "compat=0.10"][..]] {
            let disk = made(&scratch, "alone.qcow2", options);
            assert!(looks_like_qcow2(&disk));
            check_stands_alone(&disk).unwrap();
        }
    }

    #[test]
    fn a_backing_file_a_data_file_or_another_format_is_refused_unopened() {
        let scratch = Scratch::new();
        let secret = scratch.write("secret.img", vec![1; 4096]);
        let backed = made(&scratch, "backed.qcow2", &["-F", "raw", "-b", &secret.display().to_string()]);
        assert!(check_stands_alone(&backed).unwrap_err().0.contains("built on another file"));
        let data = scratch.path("data.raw");
        let external = made(&scratch, "external.qcow2", &["-o", &format!("data_file={}", data.display())]);
        assert!(check_stands_alone(&external).unwrap_err().0.contains("another file"));
        let raw = scratch.write("raw.img", vec![0; 4096]);
        assert!(!looks_like_qcow2(&raw));
        assert_eq!(check_stands_alone(&raw).unwrap_err().0, NOT_A_DISK);
        let short = scratch.write("short.qcow2", b"QFI\xfb\0\0\0\x03");
        assert_eq!(check_stands_alone(&short).unwrap_err().0, NOT_A_DISK);
    }
}
