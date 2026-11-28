// SPDX-License-Identifier: GPL-2.0-or-later
//! What the Mac host takes out of a report beyond what the Linux and Windows hosts do (their
//! `Scrubber` takes the user name, host name, home folder, UUIDs, MAC addresses, e-mail addresses
//! and machine names): passwords and the like, pairing keys and device fingerprints, pairing codes,
//! serial numbers, and any other user's home folder.
//!
//! Nothing of these is collected on purpose (see `collect`); this is for what a log might quote.
//! Like the others it is a best effort on free text, and the manifest says so.

const PASSWORD_MARK: &str = "<hidden>";
const FINGERPRINT_MARK: &str = "<fingerprint>";
const CODE_MARK: &str = "<code>";
const SERIAL_MARK: &str = "<serial>";
const HOME_MARK: &str = "~";
/// A fingerprint is a SHA-256: 32 bytes, 64 hex digits, with or without a colon between bytes.
const FINGERPRINT_BYTES: usize = 32;
/// The guest password the app makes: four groups of four from this alphabet.
const PASSWORD_ALPHABET: &str = "abcdefghjkmnpqrstuvwxyz23456789";
const PASSWORD_GROUPS: usize = 4;
const PASSWORD_GROUP_LENGTH: usize = 4;

/// Words after which the value that follows (past `:`, `=`, quotes and spaces) is hidden.
const SECRET_KEYS: [&str; 5] = ["password", "passwd", "passphrase", "secret", "token"];
/// These only with a `:` or `=` after them, as they are also ordinary words ("the Keychain item").
const SECRET_KEYS_WITH_SEPARATOR: [&str; 2] = ["keychain", "private key"];
const CODE_KEYS: [&str; 1] = ["code"];
const SERIAL_KEYS: [&str; 3] = ["serial number", "serialnumber", "serial"];

/// Takes it all out of `text`.
pub fn scrub(text: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(scrub_line).collect();
    if text.ends_with('\n') {
        lines.push(String::new());
    }
    lines.join("\n")
}

fn scrub_line(line: &str) -> String {
    let line = other_homes(line);
    let line = hide_values(&line, &SECRET_KEYS, PASSWORD_MARK, false);
    let line = hide_values(&line, &SECRET_KEYS_WITH_SEPARATOR, PASSWORD_MARK, true);
    let line = hide_values(&line, &SERIAL_KEYS, SERIAL_MARK, true);
    let line = hide_values(&line, &CODE_KEYS, CODE_MARK, true);
    words(&line)
}

/// Any user's home folder, as `~`: the Scrubber has this Mac's user only.
fn other_homes(line: &str) -> String {
    const START: &str = "/Users/";
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(at) = rest.find(START) {
        let name_start = at + START.len();
        let name_end = rest[name_start..]
            .find(|c: char| !(c.is_alphanumeric() || matches!(c, '.' | '-' | '_')))
            .map_or(rest.len(), |n| name_start + n);
        let name = &rest[name_start..name_end];
        out.push_str(&rest[..at]);
        if name.is_empty() || name == "Shared" {
            out.push_str(&rest[at..name_end]);
        } else {
            out.push_str(HOME_MARK);
        }
        rest = &rest[name_end..];
    }
    out.push_str(rest);
    out
}

/// After each of `keys` (found whatever its case, at the start of a word), hides the value that
/// follows it. With `needs_separator`, only when a `:` or `=` comes between, so that "serial.log"
/// and "the serial port" are left alone.
fn hide_values(line: &str, keys: &[&str], mark: &str, needs_separator: bool) -> String {
    let lower = line.to_ascii_lowercase();
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut at = 0;
    while at < line.len() {
        let starts_word = at == 0 || !(bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_');
        let hit = keys
            .iter()
            .filter(|key| starts_word && lower[at..].starts_with(**key))
            .max_by_key(|key| key.len());
        let Some(key) = hit else {
            let next = line[at..].chars().next().map_or(1, char::len_utf8);
            out.push_str(&line[at..at + next]);
            at += next;
            continue;
        };
        let after_key = at + key.len();
        out.push_str(&line[at..after_key]);
        let mut cursor = after_key;
        let mut separated = false;
        while cursor < bytes.len() && matches!(bytes[cursor], b' ' | b'\t' | b':' | b'=' | b'"' | b'\'') {
            separated |= matches!(bytes[cursor], b':' | b'=');
            cursor += 1;
        }
        let value_end = line[cursor..]
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ',' | '}' | ')' | ';'))
            .map_or(line.len(), |n| cursor + n);
        if cursor < value_end && cursor > after_key && (separated || !needs_separator) {
            out.push_str(&line[after_key..cursor]);
            out.push_str(mark);
            at = value_end;
        } else {
            at = after_key;
        }
    }
    out
}

/// Fingerprints (long hex) and the app's own password shape, wherever they stand as a word.
fn words(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut word = String::new();
    for c in line.chars() {
        if c.is_ascii_alphanumeric() || c == ':' || c == '-' {
            word.push(c);
        } else {
            out.push_str(&mark_word(&word));
            word.clear();
            out.push(c);
        }
    }
    out.push_str(&mark_word(&word));
    out
}

fn mark_word(word: &str) -> String {
    if is_fingerprint(word) {
        FINGERPRINT_MARK.to_owned()
    } else if is_generated_password(word) {
        PASSWORD_MARK.to_owned()
    } else {
        word.to_owned()
    }
}

fn is_fingerprint(word: &str) -> bool {
    let bytes: Vec<&str> = word.split(':').collect();
    let with_colons = bytes.len() >= FINGERPRINT_BYTES
        && bytes.iter().all(|byte| byte.len() == 2 && byte.chars().all(|c| c.is_ascii_hexdigit()));
    let plain = word.len() >= FINGERPRINT_BYTES * 2 && word.chars().all(|c| c.is_ascii_hexdigit());
    with_colons || plain
}

fn is_generated_password(word: &str) -> bool {
    let groups: Vec<&str> = word.split('-').collect();
    groups.len() == PASSWORD_GROUPS
        && groups.iter().all(|group| {
            group.len() == PASSWORD_GROUP_LENGTH && group.chars().all(|c| PASSWORD_ALPHABET.contains(c))
        })
}

#[cfg(test)]
mod tests;
