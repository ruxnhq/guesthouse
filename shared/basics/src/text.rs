//! Small text helpers: shell quoting, and standard base64 with padding. The Linux host quotes the
//! commands it shows and sends its guest's agent base64.

/// Quotes a word for a shell, leaving plain ones as they are so that commands stay readable.
pub fn shell_quote(word: &str) -> String {
    let plain = |byte: u8| byte.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&byte);
    if !word.is_empty() && word.bytes().all(plain) {
        return word.to_owned();
    }
    format!("'{}'", word.replace('\'', r"'\''"))
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const PAD: char = '=';

pub fn base64_encode(data: &[u8]) -> String {
    let mut text = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let bytes = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let group = u32::from_be_bytes([0, bytes[0], bytes[1], bytes[2]]);
        for index in 0..4 {
            if index <= chunk.len() {
                text.push(ALPHABET[(group >> (18 - 6 * index) & 63) as usize].into());
            } else {
                text.push(PAD);
            }
        }
    }
    text
}

/// `None` if this isn't base64.
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut data = Vec::with_capacity(text.len() / 4 * 3);
    let (mut group, mut bits) = (0u32, 0);
    for byte in text.trim_end_matches(PAD).bytes() {
        let value = ALPHABET.iter().position(|&letter| letter == byte)?;
        group = group << 6 | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            data.push((group >> bits) as u8);
        }
    }
    Some(data)
}

/// A character that draws nothing or moves what is drawn around it: a control, or one of the marks
/// that reverse or hide text.
pub fn is_hidden(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{feff}')
}

/// `text` with every hidden character shown as its code, like `<1b>`, so a terminal, a log or a row
/// of text shows what was there and obeys none of it. Line breaks stay when `keep_lines` is set.
pub fn shown(text: &str, keep_lines: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if is_hidden(c) && !(keep_lines && c == '\n') && c != '\t' {
            out.push_str(&format!("<{:02x}>", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

/// `shown`, cut to at most `longest` characters of the original, with `...` after a cut.
pub fn shown_short(text: &str, longest: usize) -> String {
    let cut = text.chars().count() > longest;
    let head: String = text.chars().take(longest).collect();
    let mut out = shown(&head, false);
    if cut {
        out.push_str("...");
    }
    out
}

#[cfg(test)]
mod tests;
