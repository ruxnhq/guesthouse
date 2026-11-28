//! The Linux host's macOS part, fetched its own way: an archive of known size downloaded beside the
//! part's folder as `<folder>.part`, going on from what an earlier try left, checked against its
//! SHA-256, unpacked into `<folder>.new` and moved into place whole. A folder with the part's files in
//! it is trusted as it is. Its curl words are [`super::CURL_WORDS`] and [`super::GO_ON_WORDS`], in that
//! order.

use std::path::{Path, PathBuf};

/// The archive's extension beside the part's folder while it comes.
pub const PART: &str = "part";
/// The extension of the folder the archive is unpacked into, before it is the part's.
pub const NEW: &str = "new";
const MIB: u64 = 1024 * 1024;

/// What is said as the download starts, and before how far it is.
pub const DOWNLOADING: &str = "Downloading macOS support";
/// What is said as the archive is unpacked.
pub const UNPACKING: &str = "Unpacking macOS support";
/// What is said when the archive's SHA-256 isn't the one wanted.
pub const WRONG_FILE: &str = "The macOS support that came is not the file it should be. Try again.";

/// The archive the part in `into` comes as.
pub fn archive_file(into: &Path) -> PathBuf {
    into.with_extension(PART)
}

/// The folder the archive for `into` is unpacked into first.
pub fn unpacked_folder(into: &Path) -> PathBuf {
    into.with_extension(NEW)
}

/// The download's size, as said before it starts: in MiB, rounded up.
pub fn size_text(bytes: u64) -> String {
    format!("{} MiB", bytes.div_ceil(MIB))
}

/// How far the download is, at `percent`.
pub fn progress(percent: u64) -> String {
    format!("{DOWNLOADING}: {percent}%")
}

/// What is said when the download stopped, with curl's words.
pub fn not_downloaded(said: &str) -> String {
    format!("macOS support could not be downloaded: {said}")
}

/// What is said when the archive couldn't be unpacked whole, with tar's words.
pub fn not_unpacked(said: &str) -> String {
    format!("macOS support could not be unpacked. {said}")
}
