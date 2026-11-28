// SPDX-License-Identifier: GPL-2.0-or-later
//! The driver's disk: a small raw disk image with one FAT16 volume labelled `RUXN`, which holds the
//! driver's files and which the guest attaches as a USB stick. The guest finds it by that label,
//! so the label, the file system and the files at the top of the volume are what the guest reads.
//!
//! It is made with the system's own tools, as the answer disc is: `hdiutil` attaches the image
//! without mounting it, `newfs_msdos` makes the volume, and `hdiutil` mounts it in a folder of its
//! own to copy the files in. No script is run.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

/// The volume's label, which the guest looks the stick up by.
pub const LABEL: &str = crate::install::windows::STICK_LABEL;
/// The driver's disk, at least, and the room it keeps beside its files.
const MEGABYTES: u64 = 64;
const SPARE_MEGABYTES: u64 = 16;
/// Detaching can be refused for a moment after the last write, so it is tried again.
const DETACH_TRIES: u32 = 5;
const DETACH_WAIT: Duration = Duration::from_millis(400);
const HDIUTIL: &str = "/usr/bin/hdiutil";
const NEWFS_MSDOS: &str = "/sbin/newfs_msdos";

/// A stick that holds `files`: 64 MB, or more when they need it (a Linux package's `.deb`s).
pub fn megabytes(files: &[PathBuf]) -> u64 {
    let bytes: u64 = files.iter().filter_map(|f| fs::metadata(f).ok()).map(|m| m.len()).sum();
    // FAT's own tables and the clusters' rounding take about a tenth more.
    let needed = bytes / 1_000_000 * 11 / 10 + SPARE_MEGABYTES;
    needed.max(MEGABYTES)
}

/// Makes `image` a stick with `files` at the top of its volume.
pub fn make(image: &Path, files: &[PathBuf]) -> Result<(), String> {
    let _ = fs::remove_file(image);
    let result = format(image, megabytes(files)).and_then(|()| copy_in(image, files));
    if result.is_err() {
        let _ = fs::remove_file(image);
    }
    result
}

/// An image of zeros with one FAT16 volume.
fn format(image: &Path, megabytes: u64) -> Result<(), String> {
    File::create(image)
        .and_then(|file| file.set_len(megabytes * 1_048_576))
        .map_err(|e| format!("The driver's disk could not be made: {e}"))?;
    let attached = run(
        Command::new(HDIUTIL)
            .args(["attach", "-nomount", "-imagekey", "diskimage-class=CRawDiskImage"])
            .arg(image),
        "attach the driver's disk",
    )?;
    // The first word of the first line is the disk's device, such as /dev/disk5.
    let device = attached
        .split_whitespace()
        .next()
        .filter(|word| word.starts_with("/dev/"))
        .ok_or("hdiutil did not name the driver's disk.")?
        .to_owned();
    let made = run(
        Command::new(NEWFS_MSDOS).args(["-F", "16", "-v", LABEL]).arg(&device),
        "make the driver's disk's volume",
    );
    let detached = detach(&device);
    made.and(detached)
}

/// Mounts the volume in a folder of its own, copies the files into it and lets it go.
fn copy_in(image: &Path, files: &[PathBuf]) -> Result<(), String> {
    // One folder for each stick, as the agent can make two at once (two installs).
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mount = std::env::temp_dir().join(format!(
        "guesthouse-stick-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&mount);
    fs::create_dir_all(&mount).map_err(|e| format!("Cannot make {}: {e}", mount.display()))?;
    let attached = run(
        Command::new(HDIUTIL)
            .args(["attach", "-quiet", "-nobrowse", "-imagekey", "diskimage-class=CRawDiskImage"])
            .arg("-mountpoint")
            .arg(&mount)
            .arg(image),
        "mount the driver's disk",
    );
    let copied = attached.and_then(|_| {
        files.iter().try_for_each(|file| {
            let name = file.file_name().ok_or_else(|| format!("{} has no name.", file.display()))?;
            fs::copy(file, mount.join(name))
                .map(|_| ())
                .map_err(|e| format!("{} could not be put on the driver's disk: {e}", file.display()))
        })
    });
    let detached = detach(&mount.to_string_lossy());
    let _ = fs::remove_dir(&mount);
    copied.and(detached)
}

fn detach(target: &str) -> Result<(), String> {
    let mut last = Ok(());
    for _ in 0..DETACH_TRIES {
        last = run(Command::new(HDIUTIL).args(["detach", "-quiet", target]), "let go of the driver's disk")
            .map(|_| ());
        if last.is_ok() {
            return Ok(());
        }
        thread::sleep(DETACH_WAIT);
    }
    last
}

/// Runs a command and gives what it said, or a sentence with what it said when it failed.
fn run(command: &mut Command, what: &str) -> Result<String, String> {
    let output = command.output().map_err(|e| format!("Could not {what}: {e}"))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    Err(format!("Could not {what}: {}", String::from_utf8_lossy(&output.stderr).trim()))
}

#[cfg(test)]
mod tests;
