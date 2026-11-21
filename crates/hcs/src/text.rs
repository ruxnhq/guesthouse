//! Windows' wide strings, to and from Rust's.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::core::{GUID, PWSTR};

/// A string as Windows takes it: UTF-16, ending in a zero.
pub(crate) fn wide(text: impl AsRef<OsStr>) -> Vec<u16> {
    text.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}

/// The text in a buffer Windows filled, up to its first zero.
pub(crate) fn from_wide(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|&unit| unit == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

/// Takes a string Windows allocated with LocalAlloc for its caller to free, as HCS and HCN do for
/// their answers, and frees it.
///
/// # Safety
/// `text` is null or a zero-ended string from LocalAlloc that nothing else will free.
pub(crate) unsafe fn take_local(text: PWSTR) -> Option<String> {
    if text.is_null() {
        return None;
    }
    let mut length = 0;
    // SAFETY: the caller says the string ends in a zero, so every unit up to it can be read.
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    // SAFETY: the `length` units before the zero were just read one by one.
    let owned = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
    // SAFETY: the caller says the string came from LocalAlloc and is ours to free.
    unsafe { LocalFree(text.cast()) };
    Some(owned)
}

/// A GUID as HCS and HCN write one: lower case, with no braces.
pub(crate) fn guid_text(guid: &GUID) -> String {
    let tail = guid.data4;
    format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        guid.data1,
        guid.data2,
        guid.data3,
        tail[0],
        tail[1],
        tail[2],
        tail[3],
        tail[4],
        tail[5],
        tail[6],
        tail[7]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_guid_is_written_without_braces_in_lower_case() {
        let guid = GUID::from_u128(0x064092b3_625e_43bf_9eb5_dc845897dd59);
        assert_eq!(guid_text(&guid), "064092b3-625e-43bf-9eb5-dc845897dd59");
    }

    #[test]
    fn a_filled_buffer_ends_at_its_first_zero() {
        let mut buffer = wide("C:\\x");
        buffer.extend([0x41, 0]);
        assert_eq!(from_wide(&buffer), "C:\\x");
    }
}
