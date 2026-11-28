//! The app's log, which holds the whole story of a failure.

use crate::context::Context;
use jiff::ZonedDateTime;
use std::backtrace::Backtrace;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::panic;

/// How much of the end of the log is shown, as it can grow for months.
pub const MOST_LOG_BYTES: u64 = 1 << 18;

/// The time at the start of each line.
pub fn clock_text() -> String {
    ZonedDateTime::now().strftime("%H:%M:%S").to_string()
}

pub fn open_for_appending(context: &Context) -> io::Result<File> {
    let path = context.paths.log_file();
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder)?;
    }
    OpenOptions::new().create(true).append(true).open(path)
}

/// The size at which the log is set aside as `guesthouse.log.old` (the one before is replaced) and a
/// new one is begun, so the log's folder never holds more than twice this.
pub const MOST_LOG_FILE_BYTES: u64 = 8 << 20;

/// The most one entry holds, in characters. A longer one is cut.
pub const MOST_ENTRY_CHARS: usize = 1 << 14;

fn set_aside_when_large(context: &Context) {
    let path = context.paths.log_file();
    if fs::metadata(&path).is_ok_and(|found| found.len() > MOST_LOG_FILE_BYTES) {
        let mut old = path.clone().into_os_string();
        old.push(".old");
        let _ = fs::rename(&path, old);
    }
}

/// Adds one entry. Characters that move a terminal's cursor or hide text are written as codes, and
/// an entry past [`MOST_ENTRY_CHARS`] is cut.
pub fn write_log(context: &Context, text: &str) -> io::Result<()> {
    set_aside_when_large(context);
    let text = text.trim_end();
    let cut = text.char_indices().nth(MOST_ENTRY_CHARS).map_or(text, |(end, _)| &text[..end]);
    let more = if cut.len() < text.len() { "..." } else { "" };
    writeln!(
        open_for_appending(context)?,
        "{} {}{more}",
        clock_text(),
        guesthouse_basics::text::shown(cut, true)
    )
}

/// Writes any panic to the log too, with where it happened and how it got there, as the terminal
/// that shows it by default is rarely there when the app was opened from a desktop. The default
/// message still goes to the terminal as well.
pub fn log_panics(context: &Context) {
    let context = context.clone();
    let earlier = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let trace = Backtrace::force_capture();
        let _ = write_log(&context, &format!("panic: {info}\n{trace}"));
        earlier(info);
    }));
}

/// The end of the log.
pub fn read_log(context: &Context) -> String {
    let Ok(mut file) = File::open(context.paths.log_file()) else { return String::new() };
    let size = file.metadata().map_or(0, |found| found.len());
    let mut data = Vec::new();
    if file.seek(SeekFrom::Start(size.saturating_sub(MOST_LOG_BYTES))).is_err()
        || file.read_to_end(&mut data).is_err()
    {
        return String::new();
    }
    String::from_utf8_lossy(&data).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Scratch, scratch_context};

    #[test]
    fn a_panic_ends_up_in_the_log() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        log_panics(&context);
        let caught = panic::catch_unwind(|| panic!("the planted panic"));
        // Back to the default, so other tests' panics don't land in this one's log.
        let _ = panic::take_hook();
        assert!(caught.is_err());
        let log = read_log(&context);
        assert!(log.contains("panic: ") && log.contains("the planted panic"), "{log}");
        assert!(log.contains("log.rs"), "it says where");
    }

    #[test]
    fn an_entry_is_cut_and_shows_its_hidden_characters_as_codes() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        write_log(&context, "plain\x1b]0;title\x07 words").unwrap();
        write_log(&context, &"y".repeat(MOST_ENTRY_CHARS * 4)).unwrap();
        let text = read_log(&context);
        assert!(text.contains("plain<1b>]0;title<07> words"), "{text}");
        assert!(!text.contains('\x1b'));
        assert!(text.len() < MOST_ENTRY_CHARS + 200, "{}", text.len());
    }

    #[test]
    fn a_log_past_its_size_is_set_aside_and_a_new_one_begun() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        write_log(&context, "first").unwrap();
        let filler = "x".repeat(MOST_LOG_FILE_BYTES as usize);
        write!(open_for_appending(&context).unwrap(), "{filler}").unwrap();
        write_log(&context, "second").unwrap();
        let text = read_log(&context);
        assert!(text.ends_with("second\n") && text.len() < 100, "{}", text.len());
        let old = fs::read_to_string(format!("{}.old", context.paths.log_file().display())).unwrap();
        assert!(old.starts_with(|c: char| c.is_ascii_digit()) && old.contains("first"));
    }

    #[test]
    fn only_the_end_of_a_long_log_is_read() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        assert_eq!(read_log(&context), "");
        write_log(&context, "one thing\n").unwrap();
        assert!(read_log(&context).ends_with("one thing\n"));
        let filler = "x".repeat(MOST_LOG_BYTES as usize * 2);
        write!(open_for_appending(&context).unwrap(), "{filler}").unwrap();
        write_log(&context, "last thing").unwrap();
        let text = read_log(&context);
        assert!(text.len() <= MOST_LOG_BYTES as usize && text.ends_with("last thing\n"));
    }
}
