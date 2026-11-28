//! What the command-line program writes, as text a terminal shows and does nothing with: anything
//! that could move the cursor, retitle the window or write to the clipboard is shown as a code, like
//! `<1b>`, whoever's words it was. Line breaks and tabs stay.

use guesthouse_basics::text::shown;
use std::io::{self, Write};

pub(super) struct Plain<'a>(pub &'a mut dyn Write);

impl Write for Plain<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let text = String::from_utf8_lossy(bytes);
        self.0.write_all(shown(&text, true).as_bytes())?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_sequences_are_shown_and_lines_are_left() {
        let mut taken = Vec::new();
        let mut out = Plain(&mut taken);
        writeln!(out, "name \x1b]52;c;QUJD\x07 \u{9b}31m ok").unwrap();
        write!(out, "two\tcolumns\r\n").unwrap();
        out.flush().unwrap();
        assert_eq!(
            String::from_utf8(taken).unwrap(),
            "name <1b>]52;c;QUJD<07> <9b>31m ok\ntwo\tcolumns<0d>\n"
        );
    }
}
