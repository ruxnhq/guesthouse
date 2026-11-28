//! The part a macOS guest needs on the Linux host (its QEMU, ROM and kexts), as an archive on
//! files.ruxn.com: its name, address, SHA-256 and size.

/// The part's folder among the fetched parts, and the top folder of its archive.
pub const PART_NAME: &str = "guesthouse-macos-part-2";
pub const ARCHIVE_URL: &str = "https://files.ruxn.com/p/guesthouse/guesthouse-macos-part-2-x86_64.tar.xz";
pub const ARCHIVE_SHA256: &str = "454f0800e5d231cde7dc29432053739a1393ff95b444b9a9dcad2f0bb08cec96";
/// The archive's size, for how far a download is and for saying it beforehand.
pub const ARCHIVE_BYTES: u64 = 22_772_560;
