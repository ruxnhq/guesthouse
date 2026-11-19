//! The small views the traffic lines are written with, and the question asked before one is made.
//!
//! This backend runs guests every day, so its lines about the guest's traffic are all at debug,
//! where `GUESTHOUSE_VERBOSE` shows them, and the log without it stays as it was. Nothing is read or
//! kept for a line while debug lines are off: `log::debug!` formats nothing then, and [`watching`]
//! is asked first where a line needs something read ahead of it. A view borrows what it shows and
//! writes it into the line as it is formatted, with no string of its own. That is how the code
//! reads; no test here counts what a line nobody keeps costs.

use std::fmt;

/// The start of every line this backend logs about the guest's traffic.
pub const PREFIX: &str = "amd";
/// The most a dump shows of one buffer.
pub const DUMP_BYTES: usize = 4096;

/// Whether traffic lines are kept, as they are with `GUESTHOUSE_VERBOSE`.
pub fn watching() -> bool {
    log::log_enabled!(log::Level::Debug)
}

/// The first bytes of a buffer in hex, and how many more there were.
pub struct Hex<'a> {
    pub bytes: &'a [u8],
    pub most: usize,
}

impl fmt::Display for Hex<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let shown = &self.bytes[..self.bytes.len().min(self.most)];
        for byte in shown {
            write!(f, "{byte:02x}")?;
        }
        let left = self.bytes.len() - shown.len();
        if left > 0 {
            write!(f, "..(+{left})")?;
        }
        Ok(())
    }
}

/// The first `N` bytes of a buffer, copied out of it. An answer is written into the buffer its
/// question came in, so a line written after the answer shows the question from this. It reads as
/// [`Hex`] would have of the whole buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Kept<const N: usize> {
    bytes: [u8; N],
    /// How long the buffer was, which can be more or less than `N`.
    length: usize,
}

impl<const N: usize> Kept<N> {
    pub fn of(buffer: &[u8]) -> Self {
        let mut bytes = [0; N];
        let kept = buffer.len().min(N);
        bytes[..kept].copy_from_slice(&buffer[..kept]);
        Self { bytes, length: buffer.len() }
    }
}

impl<const N: usize> fmt::Display for Kept<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kept = self.length.min(N);
        Hex { bytes: &self.bytes[..kept], most: N }.fmt(f)?;
        if self.length > kept {
            write!(f, "..(+{})", self.length - kept)?;
        }
        Ok(())
    }
}

/// An option written as its value, or `-`.
pub struct OrDash<T>(pub Option<T>);

impl<T: fmt::Display> fmt::Display for OrDash<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Some(value) => value.fmt(f),
            None => f.write_str("-"),
        }
    }
}

/// A number in hex, as a value a line can hold without making a string.
pub struct HexNumber(pub u32);

impl fmt::Display for HexNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#x}", self.0)
    }
}

/// Values with a space between them, as a line lists them inside brackets.
pub struct Spaced<I>(pub I);

impl<I: Iterator<Item: fmt::Display> + Clone> fmt::Display for Spaced<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, value) in self.0.clone().enumerate() {
            let space = if index == 0 { "" } else { " " };
            write!(f, "{space}{value}")?;
        }
        Ok(())
    }
}

/// The first `most` dwords of a buffer in hex. A short last dword is read as if padded with zeros.
pub fn dwords(bytes: &[u8], most: usize) -> Spaced<impl Iterator<Item = HexNumber> + Clone + '_> {
    Spaced(bytes.chunks(size_of::<u32>()).take(most).map(|chunk| {
        let mut dword = [0; size_of::<u32>()];
        dword[..chunk.len()].copy_from_slice(chunk);
        HexNumber(u32::from_le_bytes(dword))
    }))
}

/// A bool as the lines write it.
pub fn bit(value: bool) -> u8 {
    value.into()
}

/// A whole buffer on one line under a label, with its length, cut at [`DUMP_BYTES`].
pub struct Dump<'a> {
    pub label: fmt::Arguments<'a>,
    pub bytes: &'a [u8],
}

impl fmt::Display for Dump<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let shown = Hex { bytes: self.bytes, most: DUMP_BYTES };
        write!(f, "{PREFIX} dump {} length={} bytes={shown}", self.label, self.bytes.len())
    }
}

/// Logs a buffer whole, as [`Dump`] writes it.
pub fn dump(label: fmt::Arguments<'_>, bytes: &[u8]) {
    log::debug!("{}", Dump { label, bytes });
}

/// What one test's own thread logs. Tests run side by side in one process with one logger, so the
/// logger here keeps lines by thread, and only while a test asks for them: every other test sees
/// debug lines off, as the daemon has them without `GUESTHOUSE_VERBOSE`.
#[cfg(test)]
pub(crate) mod lines {
    use log::{Level, LevelFilter, Log, Metadata, Record};
    use std::cell::{Cell, RefCell};

    thread_local! {
        static KEEPING: Cell<bool> = const { Cell::new(false) };
        static LINES: RefCell<Vec<(Level, String)>> = const { RefCell::new(Vec::new()) };
    }

    struct ByThread;

    impl Log for ByThread {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            KEEPING.get()
        }

        fn log(&self, record: &Record<'_>) {
            if KEEPING.get() {
                LINES.with_borrow_mut(|lines| lines.push((record.level(), record.args().to_string())));
            }
        }

        fn flush(&self) {}
    }

    /// Runs `run` with every level on for this thread, and returns the lines it logged.
    pub(crate) fn logged(run: impl FnOnce()) -> Vec<(Level, String)> {
        // Only the first call sets the logger, and the later ones find it there.
        let _ = log::set_logger(&ByThread);
        log::set_max_level(LevelFilter::Debug);
        KEEPING.set(true);
        run();
        KEEPING.set(false);
        LINES.take()
    }

    /// The text of the lines at one level.
    pub(crate) fn at(level: Level, lines: &[(Level, String)]) -> Vec<&str> {
        lines.iter().filter(|(theirs, _)| *theirs == level).map(|(_, text)| text.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_buffer_is_cut_short_and_says_by_how_much() {
        assert_eq!(Hex { bytes: &[1, 0xAB, 3], most: 2 }.to_string(), "01ab..(+1)");
        assert_eq!(Hex { bytes: &[1, 0xAB], most: 2 }.to_string(), "01ab");
        assert_eq!(Hex { bytes: &[], most: 2 }.to_string(), "");
    }

    #[test]
    fn kept_bytes_read_as_the_buffer_did_before_it_was_written_to() {
        for length in [0, 1, 3, 4, 5, 9] {
            let mut buffer: Vec<u8> = (0..length).map(|byte| 0xA0 + byte).collect();
            let kept = Kept::<4>::of(&buffer);
            let before = Hex { bytes: &buffer, most: 4 }.to_string();
            buffer.fill(0);
            assert_eq!(kept.to_string(), before, "{length} bytes");
        }
    }

    #[test]
    fn a_missing_value_is_a_dash() {
        assert_eq!(OrDash(Some(HexNumber(0x1F))).to_string(), "0x1f");
        assert_eq!(OrDash::<u8>(None).to_string(), "-");
        assert_eq!((bit(true), bit(false)), (1, 0));
    }

    #[test]
    fn a_list_has_spaces_between_and_none_around() {
        assert_eq!(Spaced([5, 4, 0].iter()).to_string(), "5 4 0");
        assert_eq!(Spaced([7].iter()).to_string(), "7");
        assert_eq!(Spaced(std::iter::empty::<u8>()).to_string(), "");
    }

    #[test]
    fn dwords_stop_at_their_limit_and_pad_a_short_last_one() {
        let bytes = [1, 0, 0, 0, 0xEF, 0xBE, 0xAD, 0xDE, 0x34, 0x12];
        assert_eq!(dwords(&bytes, 4).to_string(), "0x1 0xdeadbeef 0x1234");
        assert_eq!(dwords(&bytes, 1).to_string(), "0x1");
        assert_eq!(dwords(&[], 4).to_string(), "");
    }

    #[test]
    fn a_dump_names_its_buffer_and_stops_at_its_limit() {
        let dump = Dump { label: format_args!("queue id={:#x}", 0x40), bytes: &[0x12, 0x34] };
        assert_eq!(dump.to_string(), "amd dump queue id=0x40 length=2 bytes=1234");
        let long = vec![0u8; DUMP_BYTES + 3];
        let text = Dump { label: format_args!("x"), bytes: &long }.to_string();
        assert!(text.starts_with(&format!("amd dump x length={} bytes=00", DUMP_BYTES + 3)), "{text}");
        assert!(text.ends_with("00..(+3)"), "{text}");
    }

    #[test]
    fn lines_are_kept_only_while_a_test_asks() {
        assert!(!watching());
        let lines = lines::logged(|| {
            assert!(watching());
            dump(format_args!("x"), &[7]);
        });
        assert_eq!(lines::at(log::Level::Debug, &lines), ["amd dump x length=1 bytes=07"]);
        assert!(!watching());
        dump(format_args!("y"), &[8]);
        assert!(lines::logged(|| {}).is_empty(), "a line from outside was kept");
    }
}
