// SPDX-License-Identifier: GPL-2.0-or-later
//! The Mac's keyboard layout as a Linux guest names it (XKB's layout and variant), so the guest's
//! console, its sign-in screen and its desktop put the same characters on the same keys. QEMU
//! sends the keys' places, not characters, so the guest's layout is what makes them.
//!
//! The Mac's is the input source macOS says is in use for the keyboard
//! (`com.apple.HIToolbox`'s `AppleCurrentKeyboardLayoutInputSourceID`, as `defaults` reads it).
//! Its name is matched to XKB's for the common layouts; one not in the table gives nothing, and
//! the guest keeps what it has.

/// A layout as XKB names it: `de`, or `ch` with the variant `fr`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Xkb {
    pub layout: &'static str,
    pub variant: &'static str,
}

impl Xkb {
    /// The form the guest reads from its firmware file: the layout, and the variant after a
    /// space when there is one (`de`, `ch fr`).
    pub fn text(&self) -> String {
        if self.variant.is_empty() {
            self.layout.to_string()
        } else {
            format!("{} {}", self.layout, self.variant)
        }
    }
}

/// macOS's names (after `com.apple.keylayout.`) and XKB's.
const TABLE: &[(&str, &str, &str)] = &[
    ("ABC", "us", ""),
    ("US", "us", ""),
    ("USExtended", "us", ""),
    ("USInternational-PC", "us", "intl"),
    ("Dvorak", "us", "dvorak"),
    ("Colemak", "us", "colemak"),
    ("British", "gb", ""),
    ("British-PC", "gb", ""),
    ("Irish", "ie", ""),
    ("German", "de", ""),
    ("Austrian", "at", ""),
    ("SwissGerman", "ch", ""),
    ("SwissFrench", "ch", "fr"),
    ("French", "fr", ""),
    ("French-PC", "fr", ""),
    ("French-numerical", "fr", ""),
    ("Belgian", "be", ""),
    ("Dutch", "nl", ""),
    ("Spanish", "es", ""),
    ("Spanish-ISO", "es", ""),
    ("Italian", "it", ""),
    ("Italian-Pro", "it", ""),
    ("Portuguese", "pt", ""),
    ("Brazilian", "br", ""),
    ("Brazilian-ABNT2", "br", ""),
    ("Brazilian-Pro", "br", ""),
    ("Swedish", "se", ""),
    ("Swedish-Pro", "se", ""),
    ("Norwegian", "no", ""),
    ("Danish", "dk", ""),
    ("Finnish", "fi", ""),
    ("Icelandic", "is", ""),
    ("Polish", "pl", ""),
    ("PolishPro", "pl", ""),
    ("Czech", "cz", ""),
    ("Czech-QWERTY", "cz", "qwerty"),
    ("Slovak", "sk", ""),
    ("Hungarian", "hu", ""),
    ("Slovenian", "si", ""),
    ("Croatian", "hr", ""),
    ("Romanian", "ro", ""),
    ("Turkish-QWERTY-PC", "tr", ""),
    ("Greek", "gr", ""),
    ("Russian", "ru", ""),
    ("Russian-PC", "ru", ""),
    ("Ukrainian", "ua", ""),
    ("Estonian", "ee", ""),
    ("Latvian", "lv", ""),
    ("Lithuanian", "lt", ""),
    ("Hebrew", "il", ""),
];

/// XKB's layout for a macOS input source (`com.apple.keylayout.German`), if the table has it.
pub fn from_input_source(id: &str) -> Option<Xkb> {
    let name = id.trim().strip_prefix("com.apple.keylayout.")?;
    TABLE.iter().find(|(mac, _, _)| *mac == name).map(|&(_, layout, variant)| Xkb { layout, variant })
}

/// The layout the Mac's keyboard has now, if macOS says and the table knows it.
pub fn mac_layout() -> Option<Xkb> {
    // `defaults` asks the preferences daemon, which can be slow or stuck on a busy Mac; the
    // start of a machine waits for this, so it waits only a few seconds (`probe::run_briefly`).
    let ran = crate::probe::run_briefly(
        "/usr/bin/defaults",
        &["read", "com.apple.HIToolbox", "AppleCurrentKeyboardLayoutInputSourceID"],
    )?;
    if !ran.succeeded {
        return None;
    }
    from_input_source(&ran.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_names_are_xkb_s_and_others_are_nothing() {
        let text = |id: &str| from_input_source(id).map(|xkb| xkb.text());
        assert_eq!(text("com.apple.keylayout.German\n").as_deref(), Some("de"));
        assert_eq!(text("com.apple.keylayout.ABC").as_deref(), Some("us"));
        assert_eq!(text("com.apple.keylayout.SwissFrench").as_deref(), Some("ch fr"));
        assert_eq!(text("com.apple.keylayout.Klingon"), None);
        assert_eq!(text("com.apple.inputmethod.Kotoeri.Japanese"), None);
        assert_eq!(text(""), None);
    }

    #[test]
    fn every_name_in_the_table_is_plain_text_for_a_firmware_file_and_the_answers() {
        for (mac, layout, variant) in TABLE {
            assert!(!mac.is_empty() && !layout.is_empty(), "{mac}");
            for word in [layout, variant] {
                assert!(word.bytes().all(|b| b.is_ascii_lowercase()), "{mac}: {word}");
            }
        }
    }

    #[test]
    fn this_mac_s_layout_reads_without_failing() {
        // Whatever this Mac has; only that asking does not fail or hang.
        let _ = mac_layout();
    }
}
