//! The part a macOS guest needs on the Linux host (its QEMU, ROM and kexts), as a release archive:
//! its name, address, SHA-256 and size.

/// The part's folder among the fetched parts, and the top folder of its archive.
pub const PART_NAME: &str = "guesthouse-macos-part-1";
pub const ARCHIVE_URL: &str = "https://github.com/ruxnlabs/guesthouse/releases/download/macos-part-1/\
                           guesthouse-macos-part-1-x86_64.tar.xz";
pub const ARCHIVE_SHA256: &str = "578710e662fc755b3105cc3800de35d6eae812cdd716624ac02bad322981a8bc";
/// The archive's size, for how far a download is and for saying it beforehand.
pub const ARCHIVE_BYTES: u64 = 22_435_448;
