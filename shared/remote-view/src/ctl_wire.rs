//! `ctl` commands carried over a Windows guest's serial port, for a host with no agent in the guest:
//! the host writes a request line, the guest runs remoteman's `ctl` with its words and writes the
//! answer back in numbered lines. Both ends are here, the guest's in the shape `report.ps1` follows.
//!
//! A request is `guesthouse: ctl ask ID WORD...`, the words single-spaced, ID a number from 1 the
//! host chooses. The guest answers it with `guesthouse: ctl answer ID CODE N/TOTAL CHUNK` lines:
//! CODE is the exit code `ctl` ended with, and the chunks, in order, are the base64 of everything it
//! printed, split every [`CHUNK`] characters, an empty one written `-`. Or with one line,
//! `guesthouse: ctl failed ID REASON`, when it ran nothing (see [`Failure`]).
//!
//! The guest's port is read by a host that also prints every other line the guest says, so each
//! line stays well inside the host's line limit, and no line of the channel may be printed:
//! `status` answers with the names of the devices a person trusts and the codes they pair with.
//! [`is_channel`] says which lines those are, whether or not they read as one.
//!
//! The words are a strict grammar ([`allowed`]), the same as the guest's validator, of what
//! [`crate::ctl`] makes on this host: every word is lower-case ASCII letters, digits and `-`, and a
//! device is named only by its fingerprint, so nothing the serial line carries needs quoting or can
//! be read by a shell as anything but a word. `sharing off` and `drop` are left out, as both cut
//! Guesthouse's own viewer, which this host must never do; `sharing on` cuts nobody and is in.

use crate::ctl::{FINGERPRINT_LENGTH, Words};
use crate::share::PERMISSIONS;

/// What every line of the channel begins with, both ways.
pub const CHANNEL: &str = "guesthouse: ctl ";
/// A request, from the host.
pub const ASK: &str = "guesthouse: ctl ask ";
/// A part of an answer, from the guest.
pub const ANSWER: &str = "guesthouse: ctl answer ";
/// A request the guest ran nothing for.
pub const FAILED: &str = "guesthouse: ctl failed ";
/// The base64 characters in one answer line. A multiple of 4, so each chunk decodes on its own too,
/// and with the rest of the line about 1 KB, far from the host's 4096-byte limit.
pub const CHUNK: usize = 1024;
/// The most an answer may hold, decoded. `status` is a few KB and grows with the devices trusted.
pub const MAX_ANSWER: usize = 64 * 1024;
/// The most parts an answer may have: [`MAX_ANSWER`] in base64, in [`CHUNK`]s.
pub const MAX_PARTS: u32 = MAX_ANSWER.div_ceil(3).saturating_mul(4).div_ceil(CHUNK) as u32;
/// How a chunk with nothing in it is written, so a line never ends in a space.
const EMPTY_CHUNK: &str = "-";

/// A request's number.
pub type Id = u32;

/// Why the guest ran nothing. Each is one fixed word on the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The words aren't in the grammar.
    Refused,
    /// remoteman's host isn't installed in the guest yet.
    Missing,
    /// `ctl` didn't end in the guest's time for it, and was stopped.
    Timeout,
    /// The answer was longer than [`MAX_ANSWER`].
    TooLong,
    /// `ctl` couldn't be started.
    Error,
}

impl Failure {
    const ALL: [Failure; 5] =
        [Failure::Refused, Failure::Missing, Failure::Timeout, Failure::TooLong, Failure::Error];

    /// Its word on the line.
    pub fn word(self) -> &'static str {
        match self {
            Failure::Refused => "refused",
            Failure::Missing => "missing",
            Failure::Timeout => "timeout",
            Failure::TooLong => "toolong",
            Failure::Error => "error",
        }
    }

    fn from_word(word: &str) -> Option<Failure> {
        Failure::ALL.into_iter().find(|failure| failure.word() == word)
    }

    /// What it means, in a sentence.
    pub fn sentence(self) -> &'static str {
        match self {
            Failure::Refused => "The guest refused the command.",
            Failure::Missing => "remoteman's host isn't installed in the guest yet.",
            Failure::Timeout => "remoteman's host in the guest didn't answer in time.",
            Failure::TooLong => "remoteman's host in the guest answered at more length than is taken.",
            Failure::Error => "The guest couldn't run remoteman's host.",
        }
    }
}

/// What `ctl` said, and the code it ended with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub code: i32,
    pub text: String,
}

const ON_OFF: [&str; 2] = ["on", "off"];

fn fingerprint(word: &str) -> bool {
    word.len() == FINGERPRINT_LENGTH && word.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Whether these words may be sent to the guest, or why not. The grammar, every word as it is,
/// case and all, FP a fingerprint in lower case:
///
/// ```text
/// status
/// pair on|off|accept|decline
/// revoke FP
/// device FP allow clipboard|files|sound|mic|camera on|off
/// device FP view-only on|off
/// allow clipboard|files on|off
/// set updates off
/// sharing on
/// ```
pub fn allowed(words: &[String]) -> Result<(), String> {
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let fits = match words.as_slice() {
        ["status"] => true,
        ["pair", what] => ["on", "off", "accept", "decline"].contains(what),
        ["revoke", fp] => fingerprint(fp),
        ["device", fp, "allow", what, on] => {
            fingerprint(fp) && *what != "input" && PERMISSIONS.contains(what) && ON_OFF.contains(on)
        }
        ["device", fp, "view-only", on] => fingerprint(fp) && ON_OFF.contains(on),
        ["allow", "clipboard" | "files", on] => ON_OFF.contains(on),
        ["set", "updates", "off"] => true,
        ["sharing", "on"] => true,
        _ => false,
    };
    if fits { Ok(()) } else { Err(format!("`{}` isn't a command the guest takes.", words.join(" "))) }
}

/// The line asking the guest to run `words`, with no line ending.
pub fn request_line(id: Id, words: &[String]) -> Result<String, String> {
    if id == 0 {
        return Err("A request's number starts at 1.".to_string());
    }
    allowed(words)?;
    Ok(format!("{ASK}{id} {}", words.join(" ")))
}

/// A request's number as the line has it: decimal, from 1, with no leading zero.
fn id(word: &str) -> Option<Id> {
    let digits = !word.is_empty() && word.bytes().all(|byte| byte.is_ascii_digit()) && !word.starts_with('0');
    digits.then(|| word.parse().ok()).flatten()
}

/// What the guest makes of a line it reads: nothing when it isn't a request or has no number to
/// answer, else the number and the words, or the number and [`Failure::Refused`].
pub fn read_request(line: &str) -> Option<Result<(Id, Words), Id>> {
    let rest = line.trim_end_matches(['\r', '\n']).strip_prefix(ASK)?;
    let mut parts = rest.split(' ');
    let id = id(parts.next()?)?;
    let words: Words = parts.map(str::to_string).collect();
    Some(if allowed(&words).is_ok() { Ok((id, words)) } else { Err(id) })
}

/// The lines that answer request `id` with what `ctl` printed and its exit code, or
/// [`Failure::TooLong`].
pub fn answer_lines(id: Id, code: i32, said: &[u8]) -> Result<Vec<String>, Failure> {
    if said.len() > MAX_ANSWER {
        return Err(Failure::TooLong);
    }
    let text = base64_encode(said);
    let chunks: Vec<&str> =
        if text.is_empty() { vec![EMPTY_CHUNK] } else { text.as_bytes().chunks(CHUNK).map(ascii).collect() };
    let total = chunks.len();
    Ok(chunks
        .iter()
        .enumerate()
        .map(|(index, chunk)| format!("{ANSWER}{id} {code} {}/{total} {chunk}", index + 1))
        .collect())
}

fn ascii(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).expect("base64 is ASCII")
}

/// The line saying the guest ran nothing for request `id`.
pub fn failed_line(id: Id, failure: Failure) -> String {
    format!("{FAILED}{id} {}", failure.word())
}

/// Whether a line the guest said belongs to the channel, and must not be printed or kept, whatever
/// else it is: a request echoed, an answer, or garbage that begins as one.
pub fn is_channel(line: &str) -> bool {
    line.starts_with(CHANNEL)
}

/// One line of the channel, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// A request, as the guest sees it, or the host's own echoed.
    Ask(Id),
    /// Part `index` (from 1) of `total` of the answer to `id`.
    Part { id: Id, code: i32, index: u32, total: u32, chunk: String },
    /// The guest ran nothing for `id`.
    Failed { id: Id, failure: Failure },
    /// It begins as the channel's lines do and reads as none of them.
    Garbage,
}

/// The line read, or None when it isn't the channel's at all.
pub fn parse_line(line: &str) -> Option<Line> {
    let line = line.trim_end_matches(['\r', '\n']);
    if !is_channel(line) {
        return None;
    }
    Some(parse_channel(line).unwrap_or(Line::Garbage))
}

fn parse_channel(line: &str) -> Option<Line> {
    if let Some(rest) = line.strip_prefix(ASK) {
        return Some(Line::Ask(id(rest.split(' ').next()?)?));
    }
    if let Some(rest) = line.strip_prefix(FAILED) {
        let (number, word) = rest.split_once(' ')?;
        return Some(Line::Failed { id: id(number)?, failure: Failure::from_word(word)? });
    }
    let rest = line.strip_prefix(ANSWER)?;
    let fields: Vec<&str> = rest.split(' ').collect();
    let [number, code, place, chunk] = fields.as_slice() else { return None };
    let (index, total) = place.split_once('/')?;
    let (index, total) = (id(index)?, id(total)?);
    let code = code.parse().ok().filter(|_| !code.starts_with('+'))?;
    if index > total || total > MAX_PARTS {
        return None;
    }
    let chunk = if *chunk == EMPTY_CHUNK {
        String::new()
    } else if !chunk.is_empty() && chunk.len() <= CHUNK && chunk.bytes().all(base64_byte) {
        (*chunk).to_string()
    } else {
        return None;
    };
    Some(Line::Part { id: id(number)?, code, index, total, chunk })
}

/// How a request's answer is coming along.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    /// The line wasn't about this request: another's, an older one's, a request echoed.
    Ignored,
    /// A part came, and more are to come.
    Partial,
    Done(Answer),
    /// The guest said it ran nothing.
    Failed(Failure),
    /// The parts don't make an answer: they disagree, or don't decode.
    Garbled(String),
}

/// The parts of one request's answer, as they come, in any order.
#[derive(Debug, Clone)]
pub struct Reassembly {
    id: Id,
    code: Option<i32>,
    parts: Vec<Option<String>>,
}

impl Reassembly {
    pub fn new(id: Id) -> Reassembly {
        Reassembly { id, code: None, parts: Vec::new() }
    }

    /// The request it waits on.
    pub fn id(&self) -> Id {
        self.id
    }

    /// Takes one line of the channel. A part sent twice the same is taken once. A part that
    /// disagrees with those before it, in its code, its total or its text, garbles the answer.
    pub fn feed(&mut self, line: &Line) -> Progress {
        match line {
            Line::Failed { id, failure } if *id == self.id => Progress::Failed(*failure),
            Line::Part { id, code, index, total, chunk } if *id == self.id => {
                self.part(*code, *index, *total, chunk)
            }
            _ => Progress::Ignored,
        }
    }

    fn part(&mut self, code: i32, index: u32, total: u32, chunk: &str) -> Progress {
        if self.parts.is_empty() {
            // `total` is at most MAX_PARTS, as parse_line checks.
            self.parts = vec![None; total as usize];
            self.code = Some(code);
        }
        if self.code != Some(code) || self.parts.len() != total as usize {
            return Progress::Garbled("The answer's parts disagree.".to_string());
        }
        let slot = &mut self.parts[index as usize - 1];
        match slot {
            Some(had) if had != chunk => {
                return Progress::Garbled("A part of the answer came twice, different.".into());
            }
            Some(_) => return Progress::Partial,
            None => *slot = Some(chunk.to_string()),
        }
        if self.parts.iter().any(Option::is_none) {
            return Progress::Partial;
        }
        let text: String = self.parts.iter().flatten().map(String::as_str).collect();
        match base64_decode(&text) {
            Some(bytes) if bytes.len() <= MAX_ANSWER => {
                Progress::Done(Answer { code, text: String::from_utf8_lossy(&bytes).into_owned() })
            }
            Some(_) => Progress::Garbled("The answer is longer than is taken.".to_string()),
            None => Progress::Garbled("The answer isn't base64.".to_string()),
        }
    }
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=')
}

/// Standard base64 with padding, as .NET's `Convert.ToBase64String` writes it.
pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        let joined =
            group.iter().enumerate().fold(0u32, |all, (at, byte)| all | (u32::from(*byte) << (16 - 8 * at)));
        for at in 0..4 {
            if at <= group.len() {
                out.push(ALPHABET[(joined >> (18 - 6 * at)) as usize & 0x3f] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Standard base64 with padding, strictly: None for anything else.
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let groups = bytes.len() / 4;
    for (number, group) in bytes.chunks(4).enumerate() {
        let padding = group.iter().rev().take_while(|byte| **byte == b'=').count();
        if padding > 2 || (padding > 0 && number + 1 != groups) {
            return None;
        }
        let mut joined = 0u32;
        for (at, byte) in group[..4 - padding].iter().enumerate() {
            let value = ALPHABET.iter().position(|letter| letter == byte)? as u32;
            joined |= value << (18 - 6 * at);
        }
        let kept = 3 - padding;
        // The bits padding leaves over must be zero, so each text has one meaning.
        if joined & ((1 << (8 * (3 - kept))) - 1) != 0 {
            return None;
        }
        out.extend((0..kept).map(|at| (joined >> (16 - 8 * at)) as u8));
    }
    Some(out)
}

pub mod samples;

#[cfg(test)]
mod tests;
