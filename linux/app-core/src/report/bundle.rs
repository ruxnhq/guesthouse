//! Writes a report's files into one compressed archive, scrubbed and trimmed, with a manifest that
//! says what's in it and what isn't.
//!
//! It's a `tar.gz` made by the system's `tar`, as the app makes its other archives, so the workspace
//! needs no compression of its own.

use super::collect::Collection;
use super::endpoint::MOST_REPORT_BYTES;
use super::scrub::Scrubber;
use super::summary::{self, Summary};
use super::trim::trim;
use crate::failure::{Failure, Result};
use guesthouse_report_core::host::Host;
use guesthouse_report_core::manifest::{DESCRIPTION_FILE, MANIFEST_FILE, SUMMARY_FILE, manifest_for};
use jiff::ZonedDateTime;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

/// What the server is told the archive is.
pub use guesthouse_report_core::contract::FORMAT;
const NAME_START: &str = "guesthouse-report-";
/// The most a single log keeps at first. Logs are mostly text, which compresses about tenfold, so the
/// whole stays well under the server's limit on most PCs.
const FIRST_MOST_FILE_BYTES: usize = 16 << 20;
/// A bundle that's still too big tries again with every log at half the room, this many times.
const SHRINK_ROUNDS: u32 = 6;
/// How many names a staging folder is tried under before giving up.
const STAGING_TRIES: u32 = 8;

/// A written report.
#[derive(Debug, Clone)]
pub struct Bundle {
    pub path: PathBuf,
    pub summary: Summary,
    /// Each file in it, and its size in bytes.
    pub files: Vec<(String, usize)>,
    /// The ID of the machine it's about.
    pub machine: Option<String>,
}

/// What the tester wrote.
#[derive(Debug, Clone, Default)]
pub struct Words {
    pub description: String,
    pub contact: String,
}

/// Writes the report into `folder`, and says where.
pub fn write_bundle(
    collection: &Collection,
    words: &Words,
    scrubber: &Scrubber,
    folder: &Path,
) -> Result<Bundle> {
    let name = format!("{NAME_START}{}", ZonedDateTime::now().strftime("%Y%m%d-%H%M%S"));
    let archive = folder.join(format!("{name}.{FORMAT}"));
    let mut most = FIRST_MOST_FILE_BYTES;
    for _ in 0..SHRINK_ROUNDS {
        let staging = new_staging_folder(folder, &name)?;
        let written = write_files(collection, words, scrubber, &staging.join(&name), most);
        let packed = written.and_then(|bundle| {
            pack(&staging, &name, &archive)?;
            Ok(bundle)
        });
        let _ = fs::remove_dir_all(&staging);
        let (summary, files) = packed?;
        if fs::metadata(&archive)?.len() <= MOST_REPORT_BYTES {
            let machine = collection.machine.clone();
            return Ok(Bundle { path: archive, summary, files, machine });
        }
        most /= 2;
    }
    let _ = fs::remove_file(&archive);
    Err(Failure::new("The report stayed too big to send, however much of each log was left out."))
}

/// A new folder for the report's files to be written in before they are packed, that no one else
/// has made or can open: a fresh name each time, made only if it isn't there, for this user alone.
fn new_staging_folder(folder: &Path, name: &str) -> Result<PathBuf> {
    for _ in 0..STAGING_TRIES {
        let staging = folder.join(format!(".{name}-{}", crate::random::random_hex::<8>()));
        match DirBuilder::new().mode(0o700).create(&staging) {
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            made => return made.map(|()| staging).map_err(Failure::from),
        }
    }
    Err(Failure::new("A folder to write the report in could not be made."))
}

type Written = (Summary, Vec<(String, usize)>);

fn write_files(
    collection: &Collection,
    words: &Words,
    scrubber: &Scrubber,
    top: &Path,
    most: usize,
) -> Result<Written> {
    let mut files = Vec::new();
    for item in &collection.items {
        put(top, &mut files, &item.name, &scrubber.scrub(&trim(&item.text, most)))?;
    }
    // The tester's own words go as they wrote them, but for the machines' IDs.
    let description = scrubber.scrub_machines(words.description.trim());
    put(top, &mut files, DESCRIPTION_FILE, &format!("{description}\n"))?;
    let summary = summary_of(collection, words, scrubber);
    put(top, &mut files, SUMMARY_FILE, &summary.to_json())?;
    let manifest = manifest(collection, &files);
    put(top, &mut files, MANIFEST_FILE, &manifest)?;
    Ok((summary, files))
}

/// Writes one file of the bundle, and notes its size.
fn put(top: &Path, files: &mut Vec<(String, usize)>, name: &str, text: &str) -> Result<()> {
    let path = top.join(name);
    fs::create_dir_all(path.parent().unwrap_or(top))?;
    fs::write(&path, text)?;
    files.push((name.to_owned(), text.len()));
    Ok(())
}

fn summary_of(collection: &Collection, words: &Words, scrubber: &Scrubber) -> Summary {
    let mut summary = Summary::default();
    for (key, value) in collection.summary.facts() {
        summary.set(key, &scrubber.scrub(value));
    }
    summary.set(summary::DESCRIPTION, &scrubber.scrub_machines(&words.description));
    summary.set(summary::CONTACT, &words.contact);
    summary
}

fn manifest(collection: &Collection, files: &[(String, usize)]) -> String {
    manifest_for(Host::LINUX, files, &collection.missing, &collection.notes)
}

fn pack(staging: &Path, name: &str, archive: &Path) -> Result<()> {
    // The archive is opened here, for this user only and never through a link, and tar writes to it.
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(archive)
        .map_err(|error| Failure(format!("The report's file could not be made: {error}")))?;
    let result = Command::new("tar")
        .args(["-czf", "-"])
        .stdout(file)
        .arg("-C")
        .arg(staging)
        // The owners would say who made it, and say nothing about the bug.
        .args(["--owner=0", "--group=0", "--numeric-owner", name])
        .output()
        .map_err(|error| Failure(format!("tar could not run: {error}")))?;
    if !result.status.success() {
        let said = String::from_utf8_lossy(&result.stderr);
        return Err(Failure(format!("tar could not pack the report: {}", said.trim())));
    }
    Ok(())
}

/// The file names in an archive, for a test to look at. Uses `tar` as the app does.
#[cfg(test)]
pub fn listing(archive: &Path) -> Vec<String> {
    let said = Command::new("tar").arg("-tzf").arg(archive).output().unwrap();
    String::from_utf8_lossy(&said.stdout).lines().map(str::to_owned).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::collect::Item;
    use crate::testing::Scratch;
    use std::os::unix::fs::PermissionsExt;

    fn collection() -> Collection {
        let mut collection = Collection::default();
        collection.items.push(Item {
            name: "app/guesthouse.log".into(),
            text: "10:00 opened /home/quillon/.config/guesthouse by quillon on marrowbox\n".into(),
        });
        collection.miss("host/kernel-log.txt", "this user may not read the kernel's log", Some("sudo dmesg"));
        collection.notes.push("A note.".into());
        collection.summary.set(summary::VERSION, "0.6.1");
        collection
    }

    fn scrubber() -> Scrubber {
        Scrubber::new("quillon", "marrowbox", Path::new("/home/quillon"))
    }

    #[test]
    fn a_folder_or_link_put_where_the_staging_folder_used_to_be_named_is_not_written_into() {
        let scratch = Scratch::new();
        let target = scratch.path("elsewhere");
        fs::create_dir_all(&target).unwrap();
        let now = ZonedDateTime::now();
        let pid = std::process::id();
        for second in -3..=3 {
            let when = now.checked_add(jiff::SignedDuration::from_secs(second)).unwrap();
            let name = format!("{NAME_START}{}", when.strftime("%Y%m%d-%H%M%S"));
            std::os::unix::fs::symlink(&target, scratch.path(&format!(".{name}-{pid}"))).unwrap();
            std::os::unix::fs::symlink(&target, scratch.path(&format!("{name}.{FORMAT}"))).unwrap();
        }
        let words = Words { description: "It went black.".into(), contact: String::new() };
        let made = write_bundle(&collection(), &words, &scrubber(), scratch.root());
        assert_eq!(fs::read_dir(&target).unwrap().count(), 0, "nothing went through a link");
        assert!(made.is_err(), "an archive name that is a link is not opened");
    }

    #[test]
    fn the_staging_folder_is_new_and_for_this_user_only() {
        let scratch = Scratch::new();
        let first = new_staging_folder(scratch.root(), "r").unwrap();
        let second = new_staging_folder(scratch.root(), "r").unwrap();
        assert_ne!(first, second);
        for folder in [&first, &second] {
            assert_eq!(fs::metadata(folder).unwrap().permissions().mode() & 0o777, 0o700);
        }
    }

    #[test]
    fn a_bundle_has_every_file_scrubbed_and_a_manifest() {
        let scratch = Scratch::new();
        let words = Words { description: "It went black.".into(), contact: "on the Discord".into() };
        let bundle = write_bundle(&collection(), &words, &scrubber(), scratch.root()).unwrap();
        let names = listing(&bundle.path);
        for wanted in ["app/guesthouse.log", "description.txt", "summary.json", "manifest.txt"] {
            assert!(names.iter().any(|name| name.ends_with(wanted)), "{wanted} is missing from {names:?}");
        }
        let unpacked = scratch.path("unpacked");
        fs::create_dir_all(&unpacked).unwrap();
        assert!(
            Command::new("tar")
                .arg("-xzf")
                .arg(&bundle.path)
                .arg("-C")
                .arg(&unpacked)
                .status()
                .unwrap()
                .success()
        );
        let top = fs::read_dir(&unpacked).unwrap().next().unwrap().unwrap().path();
        let log = fs::read_to_string(top.join("app/guesthouse.log")).unwrap();
        assert_eq!(log, "10:00 opened ~/.config/guesthouse by <user> on <host>\n");
        let manifest = fs::read_to_string(top.join("manifest.txt")).unwrap();
        assert!(manifest.contains("host/kernel-log.txt: this user may not read"));
        assert!(manifest.contains("to get it by hand: sudo dmesg"));
        assert!(manifest.contains("deleted after 30 days"));
        assert_eq!(bundle.summary.get(summary::CONTACT), Some("on the Discord"));
        assert!(
            fs::read_dir(scratch.root())
                .unwrap()
                .flatten()
                .all(|entry| !entry.file_name().to_string_lossy().starts_with('.')),
            "the staging folder goes"
        );
    }
}
