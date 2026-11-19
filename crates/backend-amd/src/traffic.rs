//! One log line for each exchange with the guest's driver, in a fixed form a tester's log can be
//! searched by: `amd <kind> name=value ...`.
//!
//! The kinds so far are `escape`, `query` and `registry`. All of the lines are at debug, so they are
//! in the log only with `GUESTHOUSE_VERBOSE`, and nothing is read for them without it. A line names
//! only what the protocol modules read; the rest of a buffer is in its `amd dump` line, whole up to
//! 4 KiB. A registry lookup left to the core has neither line: a guest makes them by the hundred
//! thousand as it starts, and the core logs each name it finds no value for. A value this backend
//! gives has a `registry` line with its name.
//!
//! Nothing here writes to a buffer of the guest's or decides an answer: every function takes what
//! it logs by shared reference, after the answer is made or, for an escape, before and after.

use crate::protocol::escape::{self, Answered, Count, Framing};
use crate::watch::{Hex, HexNumber, Kept, OrDash, PREFIX, bit, dump, watching};
use guesthouse_backend::{RegistryName, RegistryValue, Status, VendorQuery};
use guesthouse_messages::adapter::AdapterQuery;
use std::fmt;

/// How many bytes of an escape's payload a line shows.
const ESCAPE_BYTES_SHOWN: usize = 32;
/// How many bytes of the data that came with a query a line shows.
const QUERY_BYTES_SHOWN: usize = 32;

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

/// Logs an adapter query this backend was offered, and what came of it: `answer` is the status it
/// answered with, or `None` for a query it left to the core. Then dumps the data that came with it.
pub fn query(query: &VendorQuery<'_>, answer: Option<Status>) {
    // The core reads a registry lookup itself and logs the name it finds no value for, and a guest
    // makes them by the hundred thousand as it starts, so one left to the core gets nothing here.
    if !watching() || (answer.is_none() && query.kind == AdapterQuery::REGISTRY) {
        return;
    }
    log::debug!("{}", QueryLine { query, answer });
    dump(format_args!("query kind={}", query.kind), query.data);
}

/// A query's line. The program is quoted, as a name can hold a space, and empty when the guest gave
/// none.
struct QueryLine<'a> {
    query: &'a VendorQuery<'a>,
    answer: Option<Status>,
}

impl fmt::Display for QueryLine<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { query, answer } = self;
        write!(
            f,
            "{PREFIX} query kind={} size={} wow64={} program=\"{}\" data_bytes={} ",
            query.kind,
            query.size,
            bit(query.process_is_32_bit),
            query.program,
            query.data.len()
        )?;
        match answer {
            Some(status) => write!(f, "answer=given status={:#x}", status.0)?,
            None => f.write_str("answer=left-to-the-core")?,
        }
        write!(f, " data={}", Hex { bytes: query.data, most: QUERY_BYTES_SHOWN })
    }
}

/// Logs a registry value this backend gives, which nothing else names: the lookup has no line as a
/// query, and the core logs only the names no value was found for. What the guest gets of the value
/// is the core's to say, as it refuses a lookup that asks for another type.
pub fn registry(name: &RegistryName, value: &RegistryValue) {
    log::debug!("{}", RegistryLine { name, value });
}

/// A registry value's line. A driver path has no name the guest spells, and no type to match.
struct RegistryLine<'a> {
    name: &'a RegistryName,
    value: &'a RegistryValue,
}

impl fmt::Display for RegistryLine<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self.name {
            RegistryName::DriverStorePath => "<driver store path>",
            RegistryName::DriverImagePath => "<driver image path>",
            RegistryName::Value(name) => name,
        };
        write!(
            f,
            "{PREFIX} registry name=\"{name}\" type={} bytes={}",
            OrDash(self.value.value_type),
            self.value.bytes.len()
        )
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

    fn query_of<'a>(kind: u32, data: &'a [u8], program: &'a str) -> VendorQuery<'a> {
        VendorQuery { kind, size: 16, data, process_is_32_bit: false, program }
    }

    #[test]
    fn a_query_says_who_asked_what_and_who_answers() {
        let given =
            QueryLine { query: &query_of(1, &[0x0C, 0, 0, 0], "dwm.exe"), answer: Some(Status::SUCCESS) };
        assert_eq!(
            given.to_string(),
            "amd query kind=1 size=16 wow64=0 program=\"dwm.exe\" data_bytes=4 answer=given status=0x0 \
             data=0c000000"
        );
        let refused = QueryLine {
            query: &VendorQuery { process_is_32_bit: true, ..query_of(24, &[], "") },
            answer: Some(Status::INVALID_PARAMETER),
        };
        assert_eq!(
            refused.to_string(),
            "amd query kind=24 size=16 wow64=1 program=\"\" data_bytes=0 answer=given status=0xc000000d data="
        );
        let left = QueryLine { query: &query_of(15, &[0xAB; 40], "a game.exe"), answer: None };
        assert_eq!(
            left.to_string(),
            format!(
                "amd query kind=15 size=16 wow64=0 program=\"a game.exe\" data_bytes=40 \
                 answer=left-to-the-core data={}..(+8)",
                "ab".repeat(QUERY_BYTES_SHOWN)
            )
        );
    }

    /// Queries of each kind this backend answers, one it answers as too short, and one it leaves.
    fn queries() -> Vec<VendorQuery<'static>> {
        use crate::protocol::{
            adapter_header, adapter_info, adapter_registry, api_drivers, gpu_mmu_caps, wddm_caps,
        };
        let sized = |kind: u32, size: usize| VendorQuery { size, ..query_of(kind, &[0; 16], "dwm.exe") };
        vec![
            sized(guesthouse_backend::guest_text::QUERY_USER_DRIVER_NAME, 528),
            sized(api_drivers::QUERY_OPENGL, 528),
            sized(adapter_registry::QUERY_TYPE, 64),
            sized(wddm_caps::QUERY_WDDM_2_0_CAPS, 4),
            sized(wddm_caps::QUERY_WDDM_2_0_CAPS, 2),
            sized(gpu_mmu_caps::QUERY_GPU_MMU_CAPS, 12),
            sized(adapter_info::QUERY_TYPE, adapter_header::HEADER_SIZE),
            sized(adapter_info::QUERY_TYPE, 1),
            sized(u32::MAX, 4),
        ]
    }

    #[test]
    fn a_query_is_answered_the_same_whether_or_not_it_is_logged() {
        let info = crate::DeviceInfo::default();
        let mut answers = 0;
        for query in queries() {
            let mut alone = vec![0xA5; 4];
            let status = crate::own_answer(&info, &query, &mut alone);
            answers += usize::from(status.is_some());

            let mut quiet = vec![0xA5; 4];
            assert_eq!(crate::answer_query(&info, &query, &mut quiet), status);
            assert_eq!(quiet, alone, "with debug lines off");

            let mut watched = vec![0xA5; 4];
            let lines =
                lines::logged(|| assert_eq!(crate::answer_query(&info, &query, &mut watched), status));
            assert_eq!(watched, alone, "with debug lines on");

            // The modules that answer have lines of their own, which come first.
            let kind = query.kind;
            let [.., line, dump] = lines::at(Level::Debug, &lines)[..] else { panic!("{lines:?}") };
            assert!(line.starts_with(&format!("amd query kind={kind} size=")), "{lines:?}");
            assert_eq!(dump, format!("amd dump query kind={kind} length=16 bytes={}", "00".repeat(16)));
            assert_eq!(line.contains("answer=left-to-the-core"), status.is_none(), "{lines:?}");
            let ours = lines.iter().filter(|(_, text)| text.starts_with("amd ")).count();
            assert_eq!(ours, 2, "{lines:?}");
        }
        assert_eq!(answers, queries().len() - 1, "only the last is left to the core");
    }

    #[test]
    fn a_registry_lookup_left_to_the_core_has_no_line_and_another_query_left_to_it_has() {
        let info = crate::DeviceInfo::default();
        let lookup = query_of(AdapterQuery::REGISTRY, &[0x5A; 16], "dwm.exe");
        let mut out = vec![0xA5; 4];
        let lines = lines::logged(|| assert_eq!(crate::answer_query(&info, &lookup, &mut out), None));
        assert_eq!(out, [0xA5; 4]);
        assert!(!lines.iter().any(|(_, text)| text.starts_with("amd ")), "{lines:?}");

        let other = VendorQuery { kind: u32::MAX, ..lookup };
        let lines = lines::logged(|| assert_eq!(crate::answer_query(&info, &other, &mut out), None));
        assert_eq!(out, [0xA5; 4]);
        let [line, dump] = lines::at(Level::Debug, &lines)[..] else { panic!("{lines:?}") };
        assert!(line.starts_with("amd query kind=4294967295 size=16 "), "{lines:?}");
        assert!(line.contains(" answer=left-to-the-core "), "{lines:?}");
        assert_eq!(dump, format!("amd dump query kind=4294967295 length=16 bytes={}", "5a".repeat(16)));

        // Being left to the core is what keeps a lookup out: one this backend answered has its lines.
        let lines = lines::logged(|| query(&lookup, Some(Status::SUCCESS)));
        let [line, _dump] = lines::at(Level::Debug, &lines)[..] else { panic!("{lines:?}") };
        assert!(line.starts_with("amd query kind=48 size=16 "), "{lines:?}");
    }

    #[test]
    fn a_registry_value_the_backend_gives_is_named_and_one_it_has_not_is_left_to_the_core() {
        use guesthouse_messages::registry::TYPE_STRING;
        let vulkan = RegistryName::Value("vulkandrivername".into());
        let given = crate::registry_answer(&vulkan);
        let lines = lines::logged(|| assert_eq!(crate::registry_answer(&vulkan), given));
        let bytes = given.expect("the backend has this value").bytes.len();
        let line = format!("amd registry name=\"vulkandrivername\" type={TYPE_STRING} bytes={bytes}");
        assert_eq!(lines, [(Level::Debug, line)]);

        let store = RegistryName::DriverStorePath;
        let given = crate::registry_answer(&store);
        let lines = lines::logged(|| assert_eq!(crate::registry_answer(&store), given));
        let bytes = given.expect("the backend has this path").bytes.len();
        assert_eq!(
            lines,
            [(Level::Debug, format!("amd registry name=\"<driver store path>\" type=- bytes={bytes}"))]
        );

        let unknown = RegistryName::Value("opencldrivername".into());
        let lines = lines::logged(|| assert_eq!(crate::registry_answer(&unknown), None));
        assert!(lines.is_empty(), "{lines:?}");
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
