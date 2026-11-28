//! The shell around a host's per-machine daemon that is no one host's, so each host's daemon builds
//! on one copy: its log lines to standard error in each host's style, in `logger`, and the line it
//! stops with, [`fail`].
//!
//! A piece moves here from a host's daemon as it was, and keeps working at its old path there: the
//! Linux daemon's `guesthouse_daemon::logger` is this crate's `logger`.

use std::fmt::Display;
use std::process::ExitCode;

// Unix only: the logger writes to standard error by its descriptor and reads its log file back where
// it lies. On Windows, which has no daemon of this kind, the crate has `fail` alone.
#[cfg(unix)]
pub mod logger;

/// Says why the daemon stops, and gives the exit code it stops with, 1. The line is `prefix` and then
/// `error` on standard error, bare: it is no log line, so it has no level word or time in any style.
/// The Linux daemon's prefix is empty.
pub fn fail(prefix: &str, error: impl Display) -> ExitCode {
    eprintln!("{prefix}{error}");
    ExitCode::FAILURE
}
