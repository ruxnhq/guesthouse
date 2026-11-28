//! The files a disk is built on. A qcow2 overlay, made by hand or brought in from elsewhere, reads
//! what it hasn't changed from its backing file, and that one may have its own. The app's own
//! snapshots live inside the qcow2, so the disks it makes have none.
//!
//! The machine's settings name the files its disk is built on, and a start goes ahead only when the
//! disk's header names the same ones. QEMU writes the disk, header and all, so the header alone
//! never says which files a start may open: the settings do, and QEMU can't write those.
//!
//! A jailed QEMU only reaches the files it's given, so these have to be found before it starts. Each
//! one is read in the format the one above it names, as QEMU does: a guest can write anything to a
//! raw disk, and a guessed format would let it name a file of the user's as a backing file.

use super::header::{self, Named};
use super::info;
use crate::failure::{Failure, Result};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::path::{Path, PathBuf};

/// The format QEMU is told the machine's disk is in.
pub const DISK_FORMAT: &str = "qcow2";
/// A raw file is all data, so a chain ends there.
const RAW: &str = "raw";
/// Far more than any real chain. Past this, it's more likely a loop than a disk.
const MOST_BACKING_FILES: usize = 16;
const FIX_FORMAT: &str = "Name it with qemu-img rebase -u -F FORMAT -b BACKING DISK.";
/// How the user says a file is meant to be under the disk: see docs/headless.md.
const KEEP_IT: &str = "If the disk is meant to be built on it, guesthouse-vm set NAME backing-files \
                       FILE records that in the machine's settings.";

/// A file a machine's disk is built on, as its settings name it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackingFile {
    pub path: String,
    pub format: String,
}

fn refusal(file: &Path, why: &str) -> Failure {
    Failure(format!("The VM did not start, as its disk is built on {}, which {why}.", file.display()))
}

/// Says why QEMU couldn't open this backing file, if it couldn't.
fn check(file: &Path) -> Result<()> {
    let metadata = file.metadata().map_err(|_| refusal(file, "is missing"))?;
    if !metadata.is_file() {
        return Err(refusal(file, "isn't a plain file"));
    }
    File::open(file).map_err(|error| refusal(file, &format!("can't be read: {error}")))?;
    Ok(())
}

/// A name QEMU reads as something other than a file: `json:{...}`, or a protocol's URL.
fn names_no_file(name: &str) -> bool {
    name.starts_with("json:") || name.contains("://")
}

/// Refuses a qcow2 whose header names a file its data is kept in, which QEMU would open too.
fn refuse_data_file(image: &Path, named: &Named) -> Result<()> {
    if named.data_file {
        return Err(refusal(image, "keeps its data in another file, which a machine's disk can't"));
    }
    Ok(())
}

/// The files the disk's header names, nearest first, each with the format the one above it names,
/// and none for a disk that stands alone. Refuses a chain QEMU couldn't open, so a start says which
/// file it is.
pub fn found_chain(disk: &Path) -> Result<Vec<BackingFile>> {
    let mut chain: Vec<BackingFile> = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    let (mut image, mut format) = (disk.to_owned(), DISK_FORMAT.to_owned());
    loop {
        let found = match info(image.as_os_str(), Some(&format)) {
            Ok(found) => found,
            // A disk that's missing or can't be read is QEMU's to say, as it always has been.
            Err(_) if chain.is_empty() => return Ok(chain),
            Err(reason) => return Err(refusal(&image, &format!("can't be read: {reason}"))),
        };
        if format == DISK_FORMAT {
            refuse_data_file(&image, &header::named(&image)?)?;
        }
        // qemu-img gives the name as QEMU resolves it, against the folder of the file that names it.
        let Some(name) = found["full-backing-filename"].as_str() else { return Ok(chain) };
        if names_no_file(name) {
            return Err(Failure(format!(
                "The VM did not start, as its disk is built on {name}, which isn't a file."
            )));
        }
        let file = std::path::absolute(name)?;
        let Some(named) = found["backing-filename-format"].as_str() else {
            return Err(refusal(&file, &format!("has no format named. {FIX_FORMAT}")));
        };
        check(&file)?;
        if file == disk || seen.contains(&file) {
            return Err(refusal(&file, "is built on itself"));
        }
        if chain.len() == MOST_BACKING_FILES {
            return Err(refusal(&file, &format!("is more than {MOST_BACKING_FILES} files deep")));
        }
        seen.push(file.clone());
        chain.push(BackingFile { path: file.to_string_lossy().into_owned(), format: named.to_owned() });
        if named == RAW {
            return Ok(chain);
        }
        (image, format) = (file, named.to_owned());
    }
}

/// Whether two names are the same file, as the file system finds each now.
fn same_file(one: &str, other: &str) -> bool {
    let real = |name: &str| std::fs::canonicalize(name).unwrap_or_else(|_| PathBuf::from(name));
    one == other || real(one) == real(other)
}

/// Whether the chain the header names is the one the settings name, file for file and format for
/// format.
pub fn same_chain(found: &[BackingFile], expected: &[BackingFile]) -> bool {
    found.len() == expected.len()
        && found.iter().zip(expected).all(|(found, expected)| {
            found.format == expected.format && same_file(&found.path, &expected.path)
        })
}

/// How a chain reads in a message: each file with its format, or none.
fn chain_text(chain: &[BackingFile]) -> String {
    if chain.is_empty() {
        return "none".to_owned();
    }
    let each: Vec<String> = chain.iter().map(|file| format!("{} ({})", file.path, file.format)).collect();
    each.join(", ")
}

/// What a machine's settings are to name for a disk the user says is built on `files`, nearest
/// first: the files its header names, with their formats, if they are those files. The header is
/// only taken when the user has named each file in it.
pub fn chain_to_record(disk: &Path, files: &[PathBuf]) -> Result<Vec<BackingFile>> {
    if header::named(disk)?.backing.is_none() && files.is_empty() {
        return Ok(Vec::new());
    }
    let found = found_chain(disk)?;
    let same = found.len() == files.len()
        && found.iter().zip(files).all(|(found, file)| same_file(&found.path, &file.to_string_lossy()));
    if !same {
        return Err(Failure(format!(
            "The disk is built on {}. Name each of those files, nearest first, with a colon between \
             them, or none if the disk is to stand alone and its header names nothing.",
            chain_text(&found)
        )));
    }
    // Kept as the file system finds each file, without the `..` a relative name leaves in it.
    let real = |file: BackingFile| -> Result<BackingFile> {
        Ok(BackingFile { path: std::fs::canonicalize(&file.path)?.to_string_lossy().into_owned(), ..file })
    };
    found.into_iter().map(real).collect()
}

/// The files the machine's disk is built on, nearest first. Refuses a disk whose header names any
/// other file than `expected` (its settings) does, or the same files in other formats, so QEMU is
/// never started on a file only the header names. Each is given as the settings name it, which is
/// the name QEMU is told to open it by (see `vm_args::disk_node`), so the jail grants that name.
pub fn checked_chain(disk: &Path, expected: &[BackingFile]) -> Result<Vec<PathBuf>> {
    if expected.is_empty() {
        // Read from the header itself, so a disk that names a file is refused even where qemu-img
        // can't read it.
        let named = header::named(disk)?;
        refuse_data_file(disk, &named)?;
        if let Some(name) = named.backing {
            let file = if names_no_file(&name) {
                PathBuf::from(name)
            } else {
                std::path::absolute(disk.parent().unwrap_or(Path::new("/")).join(name))?
            };
            return Err(Failure(format!(
                "The VM did not start, as its disk is built on {}, which the machine's settings don't name. \
                 {KEEP_IT}",
                file.display()
            )));
        }
        return Ok(Vec::new());
    }
    let found = found_chain(disk)?;
    if !same_chain(&found, expected) {
        return Err(Failure(format!(
            "The VM did not start, as its disk is built on {}, and the machine's settings name {}. {KEEP_IT}",
            chain_text(&found),
            chain_text(expected)
        )));
    }
    Ok(expected.iter().map(|file| PathBuf::from(&file.path)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;
    use std::process::Command;

    /// The paths of the files the header names, nearest first.
    fn backing_files(disk: &Path) -> Result<Vec<PathBuf>> {
        Ok(found_chain(disk)?.into_iter().map(|file| PathBuf::from(file.path)).collect())
    }

    fn qemu_img(words: &[&str]) {
        let done = Command::new("qemu-img").arg(words[0]).arg("-q").args(&words[1..]).status().unwrap();
        assert!(done.success(), "qemu-img {words:?}");
    }

    /// A qcow2 at `name` in the scratch folder, on `backing` as qemu-img would be told it.
    fn overlay(scratch: &Scratch, name: &str, backing: Option<(&str, &str)>) -> PathBuf {
        let path = scratch.path(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let path_text = path.to_str().unwrap();
        match backing {
            Some((file, format)) => qemu_img(&["create", "-f", "qcow2", "-b", file, "-F", format, path_text]),
            None => qemu_img(&["create", "-f", "qcow2", path_text, "1G"]),
        }
        path
    }

    #[test]
    fn a_disk_on_its_own_has_none() {
        let scratch = Scratch::new();
        let disk = overlay(&scratch, "disk.qcow2", None);
        assert_eq!(backing_files(&disk).unwrap(), Vec::<PathBuf>::new());
    }

    #[test]
    fn a_disk_qemu_cant_open_is_left_for_it_to_say() {
        let scratch = Scratch::new();
        let not_a_disk = scratch.write("disk.qcow2", "a disk");
        for disk in [Path::new("/nowhere/disk.qcow2"), &not_a_disk] {
            assert_eq!(backing_files(disk).unwrap(), Vec::<PathBuf>::new());
        }
    }

    #[test]
    fn relative_names_are_read_from_the_folder_of_the_file_that_names_them() {
        let scratch = Scratch::new();
        let base = overlay(&scratch, "bases/base.qcow2", None);
        let middle = overlay(&scratch, "bases/middle.qcow2", Some(("base.qcow2", "qcow2")));
        let disk = overlay(&scratch, "machine/disk.qcow2", Some(("../bases/middle.qcow2", "qcow2")));
        let chain = backing_files(&disk).unwrap();
        let resolved: Vec<PathBuf> = chain.iter().map(|file| file.canonicalize().unwrap()).collect();
        assert_eq!(resolved, [middle.canonicalize().unwrap(), base.canonicalize().unwrap()]);
        assert!(chain.iter().all(|file| file.is_absolute()), "{chain:?}");
    }

    #[test]
    fn an_absolute_name_is_taken_as_it_is() {
        let scratch = Scratch::new();
        let base = overlay(&scratch, "elsewhere/base.qcow2", None);
        let disk = overlay(&scratch, "machine/disk.qcow2", Some((base.to_str().unwrap(), "qcow2")));
        assert_eq!(backing_files(&disk).unwrap(), [base]);
    }

    #[test]
    fn a_raw_base_ends_the_chain() {
        let scratch = Scratch::new();
        let base = scratch.write("base.img", vec![0; 4096]);
        let disk = overlay(&scratch, "disk.qcow2", Some(("base.img", "raw")));
        assert_eq!(backing_files(&disk).unwrap(), [base]);
    }

    #[test]
    fn a_missing_file_anywhere_in_the_chain_is_named() {
        let scratch = Scratch::new();
        let base = overlay(&scratch, "base.qcow2", None);
        overlay(&scratch, "middle.qcow2", Some(("base.qcow2", "qcow2")));
        let disk = overlay(&scratch, "disk.qcow2", Some(("middle.qcow2", "qcow2")));
        std::fs::remove_file(&base).unwrap();
        let refused = backing_files(&disk).unwrap_err();
        assert_eq!(
            refused.0,
            format!("The VM did not start, as its disk is built on {}, which is missing.", base.display())
        );
    }

    #[test]
    fn a_folder_is_not_a_backing_file() {
        let scratch = Scratch::new();
        let base = overlay(&scratch, "base.qcow2", None);
        let disk = overlay(&scratch, "disk.qcow2", Some(("base.qcow2", "qcow2")));
        std::fs::remove_file(&base).unwrap();
        std::fs::create_dir(&base).unwrap();
        assert!(backing_files(&disk).unwrap_err().0.ends_with("isn't a plain file."));
    }

    #[test]
    fn a_loop_is_refused() {
        let scratch = Scratch::new();
        overlay(&scratch, "first.qcow2", None);
        let second = overlay(&scratch, "second.qcow2", Some(("first.qcow2", "qcow2")));
        let first = scratch.path("first.qcow2");
        // Unsafe, so qemu-img only writes the name and doesn't open the loop it makes.
        qemu_img(&[
            "rebase",
            "-u",
            "-f",
            "qcow2",
            "-b",
            "second.qcow2",
            "-F",
            "qcow2",
            first.to_str().unwrap(),
        ]);
        assert!(backing_files(&second).unwrap_err().0.ends_with("is built on itself."));
    }

    fn named(file: &Path, format: &str) -> BackingFile {
        BackingFile { path: file.display().to_string(), format: format.to_owned() }
    }

    #[test]
    fn a_chain_the_settings_name_is_what_qemu_is_given() {
        let scratch = Scratch::new();
        let base = overlay(&scratch, "bases/base.qcow2", None);
        let disk = overlay(&scratch, "machine/disk.qcow2", Some(("../bases/base.qcow2", "qcow2")));
        // The settings may name it by another path to the same file, and that name is what QEMU gets.
        assert_eq!(checked_chain(&disk, &[named(&base, "qcow2")]).unwrap(), [base]);
        let alone = overlay(&scratch, "alone.qcow2", None);
        assert_eq!(checked_chain(&alone, &[]).unwrap(), Vec::<PathBuf>::new());
    }

    #[test]
    fn a_header_that_names_another_file_or_format_than_the_settings_is_refused() {
        let scratch = Scratch::new();
        let base = overlay(&scratch, "base.qcow2", None);
        let other = scratch.write("other.img", vec![0; 4096]);
        let disk = overlay(&scratch, "disk.qcow2", Some(("other.img", "raw")));
        for expected in [vec![named(&base, "qcow2")], vec![named(&other, "qcow2")], vec![]] {
            let refused = checked_chain(&disk, &expected).unwrap_err();
            assert!(refused.0.contains(&other.display().to_string()), "{}", refused.0);
            assert!(refused.0.contains("backing-files"), "{}", refused.0);
        }
        // A file under the one the settings name is part of the chain too.
        let middle = overlay(&scratch, "middle.qcow2", Some(("other.img", "raw")));
        let top = overlay(&scratch, "top.qcow2", Some(("middle.qcow2", "qcow2")));
        assert!(checked_chain(&top, &[named(&middle, "qcow2")]).is_err());
        let both = [named(&middle, "qcow2"), named(&other, "raw")];
        assert_eq!(checked_chain(&top, &both).unwrap().len(), 2);
    }

    #[test]
    fn a_disk_that_keeps_its_data_in_another_file_is_refused() {
        let scratch = Scratch::new();
        let data = scratch.path("data.raw");
        let disk = scratch.path("disk.qcow2");
        let option = format!("data_file={}", data.display());
        qemu_img(&["create", "-f", "qcow2", "-o", &option, disk.to_str().unwrap(), "64M"]);
        let refused = checked_chain(&disk, &[]).unwrap_err();
        assert!(refused.0.contains("keeps its data in another file"), "{}", refused.0);
    }

    #[test]
    fn a_name_that_is_no_file_is_refused_before_qemu_img_reads_it() {
        let scratch = Scratch::new();
        let disk = overlay(&scratch, "disk.qcow2", None);
        let name = "json:{\"driver\":\"raw\",\"file\":{\"driver\":\"file\",\"filename\":\"/etc/hostname\"}}";
        qemu_img(&["rebase", "-u", "-f", "qcow2", "-F", "raw", "-b", name, disk.to_str().unwrap()]);
        assert!(checked_chain(&disk, &[]).unwrap_err().0.contains("json:"));
    }
}
