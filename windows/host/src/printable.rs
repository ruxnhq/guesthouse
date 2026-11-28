//! A guest's text made safe to print: anything a terminal could take as a command, such as an
//! escape sequence that moves the cursor, retitles the window or writes to the clipboard, is shown
//! as a code instead, and no line can grow without end. The rule is in the crate every host can
//! share.

pub use guesthouse_serial_words_core::windows_host::printable::{LINE_LIMIT, printable};
