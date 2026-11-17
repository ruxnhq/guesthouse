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

/// The part's folder among the fetched parts, and the top folder of its archive.
pub const PART_NAME: &str = "guesthouse-macos-part-1";
const ARCHIVE_URL: &str = "https://github.com/xnixy/guesthouse/releases/download/macos-part-1/\
                           guesthouse-macos-part-1-x86_64.tar.xz";
const ARCHIVE_SHA256: &str = "578710e662fc755b3105cc3800de35d6eae812cdd716624ac02bad322981a8bc";
/// The archive's size, for how far a download is and for saying it beforehand.
const ARCHIVE_BYTES: u64 = 22_435_448;
/// A program folder that has the part in it is used as it is. A build from source can put one there.
const IN_PROGRAM: &str = "macos";

pub const MACOS_QEMU: &str = "qemu/bin/qemu-system-x86_64";
pub const MACOS_ROM: &str = "ReimsGraphics.rom";
/// The tool that makes the boot image and fetches the installer, and the guest's drivers it adds.
pub const REIMS_UTIL: &str = "reims-util";
pub const KEXTS_FOLDER: &str = "kexts";
pub const KEXT_SET: &str = "set.toml";

const PART: &str = "part";
const NEW: &str = "new";
const DOWNLOADING: &str = "Downloading macOS support";
const POLL: Duration = Duration::from_millis(500);
const MIB: u64 = 1024 * 1024;

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
    format!("{} MiB", ARCHIVE_BYTES.div_ceil(MIB))
}

fn said(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_owned()
}

/// Downloads to `to`, going on from what an earlier try left there, and says how far it is.
fn download(url: &str, to: &Path, step: &dyn Fn(&str)) -> Result<()> {
    let mut child = Command::new("curl")
        .args(["--fail", "--location", "--silent", "--show-error", "--retry", "3", "--continue-at", "-"])
        .arg("--output")
        .arg(to)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| Failure(format!("curl could not be started: {error}")))?;
    let mut shown = None;
    while child.try_wait()?.is_none() {
        let percent = fs::metadata(to).map_or(0, |found| found.len() * 100 / ARCHIVE_BYTES.max(1)).min(100);
        if shown != Some(percent) {
            shown = Some(percent);
            step(&format!("{DOWNLOADING}: {percent}%"));
        }
        sleep(POLL);
    }
    let ended = child.wait_with_output()?;
    if ended.status.success() {
        return Ok(());
    }
    Err(Failure(format!("macOS support could not be downloaded: {}", said(&ended))))
}

fn sha256(file: &Path) -> Result<String> {
    let output = Command::new("sha256sum").arg(file).stdin(Stdio::null()).output()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.split_whitespace().next().map(str::to_owned).ok_or_else(|| Failure(said(&output)))
}

/// Unpacks the archive beside where it goes and moves it into place whole, so a part that's there
/// is all there.
fn unpack(archive: &Path, into: &Path) -> Result<()> {
    let new = into.with_extension(NEW);
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
        return Err(Failure(format!("macOS support could not be unpacked. {}", said(&output))));
    }
    let _ = fs::remove_dir_all(into);
    fs::rename(new, into)?;
    Ok(())
}

fn fetch(url: &str, wanted: &str, into: &Path, step: &dyn Fn(&str)) -> Result<()> {
    let archive = into.with_extension(PART);
    fs::create_dir_all(into.parent().unwrap_or(Path::new("")))?;
    step(DOWNLOADING);
    download(url, &archive, step)?;
    let found = sha256(&archive)?;
    if found != wanted {
        let _ = fs::remove_file(&archive);
        return Err(Failure::new("The macOS support that came is not the file it should be. Try again."));
    }
    step("Unpacking macOS support");
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
mod tests {
    use super::*;
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
