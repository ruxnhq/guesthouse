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

pub fn write_log(context: &Context, text: &str) -> io::Result<()> {
    writeln!(open_for_appending(context)?, "{} {}", clock_text(), text.trim_end())
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
