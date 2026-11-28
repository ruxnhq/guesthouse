//! The Linux host's answer-file pieces, as they were in `app-core`:
//!
//! - [`xml_text`], its XML escape, which escapes the apostrophe too;
//! - [`editions`], the Windows 11 editions a guest runs and the key that picks each;
//! - [`wim`], the reader of a WIM's XML block, which lists a disc's images;
//! - [`zones`], the IANA zones and Windows' names for them;
//! - [`windows_guest`], a Windows guest's `autounattend.xml`: the template and its fill;
//! - [`lean`], lean Windows' step in Setup's specialize pass;
//! - [`ubuntu`], an Ubuntu guest's `user-data`: the template and its fill, and `meta-data`.
//!
//! The host still reads its templates from the program folder when it installs, and they are the
//! same files as the ones here.

pub mod editions;
pub mod lean;
pub mod ubuntu;
pub mod wim;
pub mod windows_guest;
pub mod zones;

/// The text as it goes inside an XML element or attribute.
pub fn xml_text(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for letter in text.chars() {
        match letter {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            other => escaped.push(other),
        }
    }
    escaped
}

#[cfg(test)]
#[path = "linux_host/tests.rs"]
mod tests;
