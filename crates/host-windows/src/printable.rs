//! A guest's text made safe to print: anything a terminal could take as a command, such as an
//! escape sequence that moves the cursor, retitles the window or writes to the clipboard, is shown
//! as a code instead, and no line can grow without end.

/// The longest line kept. A guest that sends more gets its line cut there.
pub const LINE_LIMIT: usize = 4096;

/// The bytes of one line, with no line ending, as text a terminal shows and does nothing with.
pub fn printable(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(LINE_LIMIT)]);
    let mut out = String::with_capacity(text.len());
    for character in text.trim_end_matches(['\r', '\n']).chars() {
        if character == '\t' || !character.is_control() {
            out.push(character);
        } else {
            out.push_str(&format!("<{:02x}>", character as u32));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_sequences_are_shown_not_obeyed() {
        assert_eq!(printable(b"\x1b]0;owned\x07card: ok\r\n"), "<1b>]0;owned<07>card: ok");
        assert_eq!(printable("caf\u{e9}\ttab".as_bytes()), "caf\u{e9}\ttab");
        assert_eq!(printable(b"\x9bA"), "\u{fffd}A");
    }

    #[test]
    fn a_line_stops_at_the_limit() {
        assert_eq!(printable(&vec![b'a'; LINE_LIMIT * 2]).len(), LINE_LIMIT);
    }
}
