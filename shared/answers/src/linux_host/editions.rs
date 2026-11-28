//! The Windows 11 editions a guest runs on the Linux host, each with the key that picks it in Setup.

/// What the answer file and its key install when the disc has no other edition this project runs.
pub const WINDOWS_EDITION: &str = "Windows 11 Pro";

/// The Windows 11 editions a guest runs here, best first, each with the KMS client key Microsoft
/// publishes for it. The key only tells Setup which edition to install, so it never stops to ask;
/// Windows stays unactivated. A disc with both installs Pro, as it always did. These are the editions
/// an install has been run with here. Another one goes in with its key once it has been.
pub const WINDOWS_EDITIONS: [(&str, &str); 2] = [
    (WINDOWS_EDITION, "W269N-WFGWX-YVC9B-4J6C9-T83GX"),
    // The IoT LTSC disc carries no Pro image at all.
    ("Windows 11 IoT Enterprise LTSC", "KBN8V-HFGQ4-MGXVD-347P6-PDQGT"),
];

/// The key that picks `edition` in Setup, when it is one of the editions above.
pub fn edition_key(edition: &str) -> Option<&'static str> {
    WINDOWS_EDITIONS.iter().find(|(name, _)| same_edition(edition, name)).map(|(_, key)| *key)
}

/// Whether an image is the edition `name`, or that edition's own yearly image: LTSC media name their
/// images "Windows 11 IoT Enterprise LTSC 2024", and Setup shows them without the year.
pub fn same_edition(image: &str, name: &str) -> bool {
    if image == name {
        return true;
    }
    let Some(rest) = image.strip_prefix(name) else { return false };
    rest.len() == 5 && rest.starts_with(' ') && rest[1..].chars().all(|digit| digit.is_ascii_digit())
}
