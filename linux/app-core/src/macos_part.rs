//! The macOS part: reims-vgpu's QEMU, the device's option ROM, the guest's drivers and reims-util. A
//! release doesn't carry it. It's fetched the first time a macOS machine needs it, from a release of
//! its own, and kept outside the program folder so that an update of the app leaves it be. It has a
//! number of its own, which only changes when what it's built from does: packaging/build-macos-part.sh
//! builds it, and prints the hash that goes here.

use crate::failure::{Failure, Result};
use crate::paths::Paths;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::Duration;

/// The part's folder among the fetched parts, its archive's address, SHA-256 and size, as every
/// host's catalog has them in `guesthouse_catalog_core`.
pub use guesthouse_catalog_core::macos_part::PART_NAME;
use guesthouse_catalog_core::macos_part::{ARCHIVE_BYTES, ARCHIVE_SHA256, ARCHIVE_URL};
/// A program folder that has the part in it is used as it is. A build from source can put one there.
const IN_PROGRAM: &str = "macos";

pub const MACOS_QEMU: &str = "qemu/bin/qemu-system-x86_64";
pub const MACOS_ROM: &str = "ReimsGraphics.rom";
/// The tool that makes the boot image and fetches the installer, and the guest's drivers it adds.
pub const REIMS_UTIL: &str = "reims-util";
pub const KEXTS_FOLDER: &str = "kexts";
pub const KEXT_SET: &str = "set.toml";

/// The archive's and the unpacked folder's names, the words curl is given, how far it is and the
/// sentences, as the Linux host has them in every host's `guesthouse_download_core`.
use guesthouse_download_core::linux_host::macos_part::{
    DOWNLOADING, UNPACKING, WRONG_FILE, archive_file, not_downloaded, not_unpacked, progress, unpacked_folder,
};
#[cfg(test)]
use guesthouse_download_core::linux_host::macos_part::{NEW, PART};
use guesthouse_download_core::linux_host::{self as rules, CURL_WORDS, GO_ON_WORDS};
const POLL: Duration = Duration::from_millis(500);

pub fn folder(paths: &Paths) -> PathBuf {
    let own = paths.helper(IN_PROGRAM);
    if own.is_dir() { own } else { paths.parts.join(PART_NAME) }
}

/// A file of the part, whether or not it's there.
pub fn file(paths: &Paths, name: &str) -> PathBuf {
    folder(paths).join(name)
}

fn whole(folder: &Path) -> bool {
    [REIMS_UTIL, MACOS_QEMU, MACOS_ROM].iter().all(|name| folder.join(name).exists())
        && folder.join(KEXTS_FOLDER).join(KEXT_SET).exists()
}

/// Whether a macOS machine can be made and started without a download.
pub fn is_here(paths: &Paths) -> bool {
    whole(&folder(paths))
}

/// The download's size, as said before it starts.
pub fn download_size() -> String {
    guesthouse_download_core::linux_host::macos_part::size_text(ARCHIVE_BYTES)
}

fn said(output: &std::process::Output) -> String {
    rules::said(&output.stderr)
}

/// Downloads to `to`, going on from what an earlier try left there, and says how far it is.
fn download(url: &str, to: &Path, step: &dyn Fn(&str)) -> Result<()> {
    let mut child = Command::new("curl")
        .args(CURL_WORDS)
        .args(GO_ON_WORDS)
        .arg(to)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| Failure(rules::not_started(error)))?;
    let mut shown = None;
    while child.try_wait()?.is_none() {
        let percent = rules::percent(fs::metadata(to).map_or(0, |found| found.len()), ARCHIVE_BYTES);
        if shown != Some(percent) {
            shown = Some(percent);
            step(&progress(percent));
        }
        sleep(POLL);
    }
    let ended = child.wait_with_output()?;
    if ended.status.success() {
        return Ok(());
    }
    Err(Failure(not_downloaded(&said(&ended))))
}

fn sha256(file: &Path) -> Result<String> {
    let output = Command::new("sha256sum").arg(file).stdin(Stdio::null()).output()?;
    rules::sha256sum_sum(&output.stdout).ok_or_else(|| Failure(said(&output)))
}

/// Unpacks the archive beside where it goes and moves it into place whole, so a part that's there
/// is all there.
fn unpack(archive: &Path, into: &Path) -> Result<()> {
    let new = unpacked_folder(into);
    let _ = fs::remove_dir_all(&new);
    fs::create_dir_all(&new)?;
    let output = Command::new("tar")
        .args(["--extract", "--xz", "--strip-components=1", "--no-same-owner", "--file"])
        .arg(archive)
        .arg("--directory")
        .arg(&new)
        .stdin(Stdio::null())
        .output()?;
    if !output.status.success() || !whole(&new) {
        let _ = fs::remove_dir_all(&new);
        return Err(Failure(not_unpacked(&said(&output))));
    }
    let _ = fs::remove_dir_all(into);
    fs::rename(new, into)?;
    Ok(())
}

fn fetch(url: &str, wanted: &str, into: &Path, step: &dyn Fn(&str)) -> Result<()> {
    let archive = archive_file(into);
    fs::create_dir_all(into.parent().unwrap_or(Path::new("")))?;
    step(DOWNLOADING);
    download(url, &archive, step)?;
    let found = sha256(&archive)?;
    if !rules::sum_is_wanted(&found, wanted) {
        let _ = fs::remove_file(&archive);
        return Err(Failure::new(WRONG_FILE));
    }
    step(UNPACKING);
    unpack(&archive, into)?;
    let _ = fs::remove_file(archive);
    Ok(())
}

/// Fetches the part if it isn't here. `step` is told what is happening.
pub fn ensure(paths: &Paths, step: &dyn Fn(&str)) -> Result<()> {
    if is_here(paths) {
        return Ok(());
    }
    fetch(ARCHIVE_URL, ARCHIVE_SHA256, &paths.parts.join(PART_NAME), step)
}

#[cfg(test)]
#[path = "macos_part_pins.rs"]
mod pins;

#[cfg(test)]
mod tests {
    use super::*;

    /// Where the part comes from, what it must hash to and how big it is, as they were pinned.
    #[test]
    fn the_part_is_what_it_was() {
        assert_eq!(PART_NAME, "guesthouse-macos-part-2");
        assert_eq!(
            ARCHIVE_URL,
            "https://github.com/ruxnlabs/guesthouse/releases/download/macos-part-2/guesthouse-macos-part-2-x86_64.tar.xz"
        );
        assert_eq!(ARCHIVE_SHA256, "454f0800e5d231cde7dc29432053739a1393ff95b444b9a9dcad2f0bb08cec96");
        assert_eq!(ARCHIVE_BYTES, 22_772_560);
    }
    use crate::testing::{Scratch, scratch_paths};

    /// An archive as the build packs it, and a `file://` address for it.
    fn archive(scratch: &Scratch, files: &[&str]) -> (String, String) {
        let top = scratch.path("made").join(PART_NAME);
        for name in files {
            let file = top.join(name);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, name).unwrap();
        }
        let packed = scratch.path("part.tar.xz");
        let made = Command::new("tar")
            .args(["--create", "--xz", "--file"])
            .arg(&packed)
            .arg("--directory")
            .arg(scratch.path("made"))
            .arg(PART_NAME)
            .status()
            .unwrap();
        assert!(made.success());
        (format!("file://{}", packed.display()), sha256(&packed).unwrap())
    }

    const FILES: [&str; 4] = [REIMS_UTIL, MACOS_QEMU, MACOS_ROM, "kexts/set.toml"];

    #[test]
    fn a_program_folders_own_part_comes_first() {
        let scratch = Scratch::new();
        let paths = scratch_paths(&scratch);
        assert_eq!(folder(&paths), paths.parts.join(PART_NAME));
        assert!(!is_here(&paths));
        fs::create_dir_all(paths.helper(IN_PROGRAM)).unwrap();
        assert_eq!(file(&paths, MACOS_ROM), paths.helper("macos/ReimsGraphics.rom"));
    }

    #[test]
    fn a_fetched_part_is_checked_and_then_whole() {
        let scratch = Scratch::new();
        let paths = scratch_paths(&scratch);
        let (url, hash) = archive(&scratch, &FILES);
        let into = paths.parts.join(PART_NAME);
        let wrong = fetch(&url, &"0".repeat(64), &into, &|_| {}).unwrap_err().to_string();
        assert!(wrong.contains("not the file it should be"), "{wrong}");
        assert!(!into.exists() && !into.with_extension(PART).exists());
        fetch(&url, &hash, &into, &|_| {}).unwrap();
        assert!(is_here(&paths));
        assert!(!into.with_extension(PART).exists() && !into.with_extension(NEW).exists());
    }

    #[test]
    fn an_archive_with_a_file_missing_is_not_put_in_place() {
        let scratch = Scratch::new();
        let paths = scratch_paths(&scratch);
        let (url, hash) = archive(&scratch, &FILES[..3]);
        let into = paths.parts.join(PART_NAME);
        assert!(fetch(&url, &hash, &into, &|_| {}).is_err());
        assert!(!into.exists() && !is_here(&paths));
    }
}
