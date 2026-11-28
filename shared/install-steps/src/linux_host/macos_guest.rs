//! A macOS machine's install on the Linux host, as far as the app takes it: the boot image and its
//! firmware made by reims-util in [`BOOT_PARTS`], Apple's installer fetched by reims-util into
//! [`INSTALLERS`] and written out as a disk under the [`PART`] extension, each moved into place last,
//! and a first start into it. A part already in place is kept, so an install run again goes on where
//! it stopped.

/// Where the boot image and its firmware are made, before they're the machine's.
pub const BOOT_PARTS: &str = "boot.part";
/// Apple's installers, kept for the next machine. A download that stopped goes on from here.
pub const INSTALLERS: &str = "macos-installers";
/// The extension the installer is written out under, before it is the machine's.
pub const PART: &str = "part";

/// The steps the install says, in order, each with how far it is as reims-util says it.
pub const MAKING_BOOT_IMAGE: &str = "Making the Mac's boot image";
pub const DOWNLOADING: &str = "Downloading macOS from Apple";
pub const GETTING_READY: &str = "Getting the installer ready";

pub const NO_RESULT: &str = "reims-util ended without saying what it made.";

/// How far reims-util is, in whole percent, from the `done` and `total` it says, when it says both and
/// the total isn't nothing.
pub fn percent(done: Option<u64>, total: Option<u64>) -> Option<u64> {
    done.zip(total).filter(|(_, total)| *total > 0).map(|(done, total)| done * 100 / total)
}

/// A step and how far it is.
pub fn progress(doing: &str, percent: u64) -> String {
    format!("{doing}: {percent}%")
}

/// Said when Apple's installer couldn't be written out as a disk, with qemu-img's words.
pub fn not_ready(said: &str) -> String {
    format!("The installer could not be made ready: {}", said.trim())
}
