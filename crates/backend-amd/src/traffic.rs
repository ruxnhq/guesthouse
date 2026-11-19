//! One log line for each exchange with the guest's driver, in a fixed form a tester's log can be
//! searched by: `amd <kind> name=value ...`.
//!
//! The kind so far is `escape`. All of the lines are at debug, so they are in the log only with
//! `GUESTHOUSE_VERBOSE`, and nothing is read for them without it. A line names only what the
//! protocol modules read; the rest of a buffer is in its `amd dump` line, whole up to 4 KiB.
//!
//! Nothing here writes to a buffer of the guest's or decides an answer: every function takes what
//! it logs by shared reference, after the answer is made or, for an escape, before and after.

use crate::protocol::escape::{self, Answered, Count, Framing};
use crate::watch::{HexNumber, Kept, OrDash, PREFIX, bit, dump, watching};
use std::fmt;

/// How many bytes of an escape's payload a line shows.
const ESCAPE_BYTES_SHOWN: usize = 32;

/// An escape as the guest sent it, kept from before its answer was written into the same buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EscapeAsked {
    length: usize,
    framing: Framing,
    payload: Kept<ESCAPE_BYTES_SHOWN>,
}

impl EscapeAsked {
    fn read(data: &[u8]) -> Self {
        Self { length: data.len(), framing: escape::framing(data), payload: Kept::of(escape::payload(data)) }
    }
}

/// Dumps an escape as it came, and keeps what its line will say of it. Called before the escape is
/// answered. With debug lines off it reads nothing and returns `None`.
pub fn escape_asked(process: u32, data: &[u8]) -> Option<EscapeAsked> {
    if !watching() {
        return None;
    }
    dump(format_args!("escape process={process:#x}"), data);
    Some(EscapeAsked::read(data))
}

/// Logs an escape's line, once it is answered.
pub fn escape_answered(
    process: u32,
    asked: Option<EscapeAsked>,
    answered: &Result<Answered, escape::Refused>,
) {
    if let Some(asked) = asked {
        log::debug!("{}", EscapeLine { process, asked, answered });
    }
}

/// An escape's line. `code` is the code as sent, and `service`, `subfunction` and `client` are the
/// parts of the one the kernel driver dispatches, which differ for the few codes it renames.
struct EscapeLine<'a> {
    process: u32,
    asked: EscapeAsked,
    answered: &'a Result<Answered, escape::Refused>,
}

impl fmt::Display for EscapeLine<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { process, asked, answered } = self;
        let code = answered.as_ref().ok().map(|answered| answered.code);
        write!(
            f,
            "{PREFIX} escape process={process:#x} length={} wrapped={} code={} service={} subfunction={} \
             client={} input_bytes={} output_bytes={} {} payload={}",
            asked.length,
            bit(asked.framing.wrapped),
            OrDash(asked.framing.code_sent.map(HexNumber)),
            OrDash(code.map(|code| code.service)),
            OrDash(code.map(|code| HexNumber(code.subfunction))),
            OrDash(code.map(|code| code.client)),
            CountText(asked.framing.input),
            CountText(asked.framing.output),
            EscapeAnswer(answered),
            asked.payload
        )
    }
}

/// A payload's byte count: the count, `-` where the buffer ends before it, and a mark where the
/// buffer hasn't that many bytes after it.
struct CountText(Count);

impl fmt::Display for CountText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Count::Missing => f.write_str("-"),
            Count::Fits(count) => write!(f, "{count}"),
            Count::PastEnd(count) => write!(f, "{count}(past-end)"),
        }
    }
}

/// How an escape was answered, as `answer=`, with why for one that couldn't be read.
struct EscapeAnswer<'a>(&'a Result<Answered, escape::Refused>);

impl fmt::Display for EscapeAnswer<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Ok(answered) if answered.done => f.write_str("answer=done"),
            Ok(answered) if answered.code.needed_by_the_guest() => f.write_str("answer=needed-but-misshaped"),
            Ok(_) => f.write_str("answer=failed"),
            Err(refused) => write!(f, "answer=unreadable why=\"{}\"", refused.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::bytes::put_dword;
    use crate::protocol::escape::{COUNT_SIZE, HEADER_SIZE, WRAPPER_KIND, WRAPPER_SIZE, WRAPPER_VERSION};
    use crate::watch::lines;
    use log::Level;

    const PROCESS: u32 = 0x4000_0002;
    /// Where the header keeps its version and its code, and the oldest version the kernel takes.
    const HEADER_VERSION_AT: usize = 0x04;
    const HEADER_VERSION: u32 = 0x0001_0000;
    const CODE_AT: usize = 0x08;

    /// A framed escape with an input and an output of these sizes, each filled with its own mark.
    fn escape_of(code: u32, input_bytes: usize, output_bytes: usize) -> Vec<u8> {
        let output_count = HEADER_SIZE + COUNT_SIZE + input_bytes;
        let mut data = vec![0u8; output_count + COUNT_SIZE + output_bytes];
        put_dword(&mut data, 0, HEADER_SIZE as u32);
        put_dword(&mut data, HEADER_VERSION_AT, HEADER_VERSION);
        put_dword(&mut data, CODE_AT, code);
        put_dword(&mut data, HEADER_SIZE, input_bytes as u32);
        data[HEADER_SIZE + COUNT_SIZE..output_count].fill(0x11);
        put_dword(&mut data, output_count, output_bytes as u32);
        data[output_count + COUNT_SIZE..].fill(0xCC);
        data
    }

    fn wrapped(inner: &[u8]) -> Vec<u8> {
        let mut data = vec![0u8; WRAPPER_SIZE];
        put_dword(&mut data, 0, WRAPPER_KIND);
        put_dword(&mut data, 4, WRAPPER_VERSION);
        data.extend_from_slice(inner);
        data
    }

    /// The line an escape gets, read before the answer and written after it, as the backend does.
    fn line_of(mut data: Vec<u8>) -> String {
        let asked = EscapeAsked::read(&data);
        let answered = escape::answer(&mut data);
        EscapeLine { process: PROCESS, asked, answered: &answered }.to_string()
    }

    #[test]
    fn an_escape_answered_as_done_has_its_line() {
        assert_eq!(
            line_of(escape_of(0x0300_6203, 0x20, 0x20)),
            "amd escape process=0x40000002 length=200 wrapped=0 code=0x3006203 service=3 subfunction=0x62 \
             client=3 input_bytes=32 output_bytes=32 answer=done \
             payload=2000000011111111111111111111111111111111111111111111111111111111..(+40)"
        );
    }

    #[test]
    fn a_wrapped_escape_nothing_answers_is_failed_and_says_it_was_wrapped() {
        let line = line_of(wrapped(&escape_of(0x0300_7E03, 0x100, 0x100)));
        assert!(
            line.starts_with(
                "amd escape process=0x40000002 length=724 wrapped=1 code=0x3007e03 service=3 \
                 subfunction=0x7e client=3 input_bytes=256 output_bytes=256 answer=failed payload=00010000"
            ),
            "{line}"
        );
    }

    #[test]
    fn a_renamed_code_shows_both_what_was_sent_and_what_is_dispatched() {
        let line = line_of(escape_of(0x0100_2603, 4, 4));
        assert!(line.contains(" code=0x1002603 service=3 subfunction=0x26 client=3 "), "{line}");
    }

    #[test]
    fn a_needed_escape_of_the_wrong_shape_says_so_and_marks_the_count_that_does_not_fit() {
        let mut short = escape_of(0x0300_5D03, 0x20, 0x60);
        short.pop();
        let line = line_of(short);
        assert!(
            line.contains(" input_bytes=32 output_bytes=96(past-end) answer=needed-but-misshaped payload="),
            "{line}"
        );
    }

    #[test]
    fn an_escape_that_cannot_be_read_has_dashes_for_what_nothing_read() {
        assert_eq!(
            line_of(vec![0xEE; 6]),
            "amd escape process=0x40000002 length=6 wrapped=0 code=- service=- subfunction=- client=- \
             input_bytes=- output_bytes=- answer=unreadable why=\"shorter than its header\" payload="
        );
        let mut old = escape_of(0x0200_0103, 0, 0);
        put_dword(&mut old, HEADER_VERSION_AT, HEADER_VERSION - 1);
        assert_eq!(
            line_of(old),
            "amd escape process=0x40000002 length=136 wrapped=0 code=0x2000103 service=- subfunction=- \
             client=- input_bytes=0 output_bytes=0 answer=unreadable why=\"its header is too old\" \
             payload=0000000000000000"
        );
    }

    #[test]
    fn the_line_shows_the_payload_as_sent_even_where_the_answer_wrote_over_it() {
        let mut data = escape_of(0x0300_7E03, 0, 0x20);
        let asked = EscapeAsked::read(&data);
        // No answer made today writes this near the payload's start. One that did would look so.
        data[HEADER_SIZE..].fill(0);
        let line = EscapeLine { process: PROCESS, asked, answered: &escape::answer(&mut data) }.to_string();
        assert!(
            line.ends_with("payload=0000000020000000cccccccccccccccccccccccccccccccccccccccccccccccc..(+8)"),
            "{line}"
        );
    }

    #[test]
    fn an_escape_is_dumped_as_it_came_and_then_gets_its_line() {
        let mut data = escape_of(0x0300_6203, 0x20, 0x20);
        let sent = data.clone();
        let lines = lines::logged(|| {
            let asked = escape_asked(PROCESS, &data);
            let answered = escape::answer(&mut data);
            escape_answered(PROCESS, asked, &answered);
        });
        assert_ne!(data, sent, "the answer changed the buffer");
        let debug = lines::at(Level::Debug, &lines);
        assert_eq!(debug.len(), lines.len());
        let hex: String = sent.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(debug[0], format!("amd dump escape process=0x40000002 length=200 bytes={hex}"));
        assert!(debug[1].starts_with("amd escape process=0x40000002 length=200 "), "{}", debug[1]);
        assert_eq!(debug.len(), 2);
    }

    /// What the backend's own handler leaves in the buffer and the log, for each way an escape can
    /// be answered: `(escape, warnings, info lines)`.
    fn every_kind_of_escape() -> Vec<(Vec<u8>, usize, usize)> {
        let mut misshaped = escape_of(0x0300_5D03, 0x20, 0x60);
        misshaped.pop();
        vec![
            (escape_of(0x0300_5D03, 0x20, 0x60), 0, 0),
            (escape_of(0x0300_6203, 0x20, 0x20), 0, 0),
            (escape_of(0x0300_6C03, 0x100, 0x20), 0, 0),
            (wrapped(&escape_of(0x0300_6203, 0x20, 0x20)), 0, 0),
            (wrapped(&escape_of(0x0300_7E03, 0x100, 0x100)), 0, 0),
            (escape_of(0x0200_0103, 8, 8), 0, 0),
            (misshaped, 1, 0),
            (vec![0xEE; 6], 0, 1),
            (Vec::new(), 0, 1),
        ]
    }

    #[test]
    fn the_answer_is_the_same_bytes_whether_or_not_the_escape_is_logged() {
        for (sent, warnings, infos) in every_kind_of_escape() {
            let mut alone = sent.clone();
            let _ = escape::answer(&mut alone);

            let mut quiet = sent.clone();
            crate::answer_escape(PROCESS, &mut quiet);
            assert_eq!(quiet, alone, "with debug lines off");

            let mut watched = sent.clone();
            let lines = lines::logged(|| crate::answer_escape(PROCESS, &mut watched));
            assert_eq!(watched, alone, "with debug lines on");

            // The dump is first and holds what was sent, and the line is last.
            let hex: String = sent.iter().map(|byte| format!("{byte:02x}")).collect();
            let length = sent.len();
            let debug = lines::at(Level::Debug, &lines);
            assert_eq!(debug.len(), 2, "{lines:?}");
            assert_eq!(lines[0].1, format!("amd dump escape process=0x40000002 length={length} bytes={hex}"));
            assert!(lines.last().unwrap().1.starts_with("amd escape process=0x40000002 "), "{lines:?}");
            assert_eq!(lines::at(Level::Warn, &lines).len(), warnings, "{lines:?}");
            assert_eq!(lines::at(Level::Info, &lines).len(), infos, "{lines:?}");
            assert_eq!(lines.len(), 2 + warnings + infos);
        }
    }

    #[test]
    fn the_lines_the_log_had_before_are_worded_as_they_were() {
        let mut misshaped = escape_of(0x0300_5D03, 0x20, 0x60);
        misshaped.pop();
        let lines = lines::logged(|| crate::answer_escape(7, &mut misshaped));
        assert_eq!(
            lines::at(Level::Warn, &lines),
            ["escape from 7: service 3 subfunction 0x5d, not shaped as the guest's driver sends it, \
              answered as failed; the guest can't go on without it"]
        );
        let lines = lines::logged(|| crate::answer_escape(7, &mut [0xEE; 6]));
        assert_eq!(
            lines::at(Level::Info, &lines),
            ["an escape from 7 is shorter than its header (6 bytes, starting 0xeeeeeeee, 0xeeee)"]
        );
    }

    #[test]
    fn nothing_is_read_from_an_escape_with_debug_lines_off() {
        let data = escape_of(0x0300_6203, 0x20, 0x20);
        assert_eq!(escape_asked(PROCESS, &data), None);
        let lines = lines::logged(|| {
            // What the backend holds when the lines came on between the two calls.
            escape_answered(PROCESS, None, &Err(escape::Refused("unused")));
        });
        assert!(lines.is_empty());
    }
}
