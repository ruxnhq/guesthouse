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

#[cfg(test)]
mod tests;
