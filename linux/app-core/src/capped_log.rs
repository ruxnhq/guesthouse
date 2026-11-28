//! Files QEMU writes into the run folder, held to a size: its output, and the guest's serial port.
//!
//! QEMU gets a pipe, and a small process of the task program reads it into the file. That process
//! outlives the app as QEMU does, and ends when QEMU closes its end. Past the size, the file keeps
//! its start and goes on from there with the newest lines, saying where the middle was left out, so
//! a guest that writes without end can't fill the run folder, which is in memory.

use crate::context::Context;
use crate::daemon::detached;
use crate::failure::Result;
use crate::tasks::TASK_PROGRAM;
use guesthouse_daemon::handover::KEPT_ABOVE;
use rustix::io::fcntl_dupfd_cloexec;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{self, ErrorKind, Read, Seek, SeekFrom, Write};
use std::os::fd::OwnedFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

/// The task program's word for keeping a log: `guesthouse-task keep-log FILE MOST_BYTES`.
pub const KEEP_LOG: &str = "keep-log";
/// The guest's serial port. A whole Windows or Ubuntu install says far less.
pub const SERIAL_MOST_BYTES: u64 = 64 << 20;
/// QEMU's own output.
pub const OUTPUT_MOST_BYTES: u64 = 16 << 20;
/// Said where the middle of a log was left out.
pub const LEFT_OUT: &[u8] =
    b"\n[Guesthouse: this log reached its size, so what came between here and its start was left out.]\n";
const CHUNK: usize = 64 * 1024;
/// How long a reader waits for the last of a log once its writer has gone.
const SETTLE: Duration = Duration::from_secs(2);

/// The keepers this process started, by file, each saying when it's done.
static KEEPERS: Mutex<Option<HashMap<PathBuf, Receiver<()>>>> = Mutex::new(None);

/// `file` as these options open it, following no link, and only if it's a plain file.
fn open_plain(options: &mut OpenOptions, file: &Path) -> io::Result<std::fs::File> {
    let flags = rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK;
    let opened = options.mode(0o600).custom_flags(flags.bits() as i32).open(file)?;
    if !opened.metadata()?.is_file() {
        return Err(io::Error::new(ErrorKind::InvalidInput, "not a plain file"));
    }
    Ok(opened)
}

/// Reads `input` to its end into `file`, which never holds more than `most` bytes: when the next
/// bytes wouldn't fit, the file goes back to its first half, says so, and carries on.
pub fn keep(mut input: impl Read, file: &Path, most: u64) -> io::Result<()> {
    let mut out = open_plain(OpenOptions::new().create(true).append(true), file)?;
    let start = most / 2;
    let mut size = out.metadata()?.len();
    let mut buffer = vec![0; CHUNK];
    loop {
        let read = match input.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(read) => read,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if size + read as u64 > most {
            out.set_len(start)?;
            out.seek(SeekFrom::End(0))?;
            out.write_all(LEFT_OUT)?;
            size = start + LEFT_OUT.len() as u64;
        }
        let kept = &buffer[..read.min(usize::try_from(most.saturating_sub(size)).unwrap_or(usize::MAX))];
        out.write_all(kept)?;
        size += kept.len() as u64;
    }
}

/// Empties `file` and starts the process that keeps it. Returns the pipe's end for QEMU, closed on
/// exec and above the numbers QEMU's descriptors are placed at.
pub fn start(context: &Context, file: &Path, most: u64) -> Result<OwnedFd> {
    // Made new: whatever was at the name, a link among them, goes, and nothing it named is touched.
    match std::fs::remove_file(file) {
        Err(error) if error.kind() != ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    open_plain(OpenOptions::new().write(true).create_new(true), file)?;
    let (reading, writing) = io::pipe()?;
    let mut keeper = detached(
        Command::new(context.paths.binary(TASK_PROGRAM)).arg(KEEP_LOG).arg(file).arg(most.to_string()),
    )
    .stdin(Stdio::from(OwnedFd::from(reading)))
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .spawn()?;
    // Waited for, so it leaves nothing behind in a long-lived app, and says when the file is whole.
    let (done, finished) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = keeper.wait();
        let _ = done.send(());
    });
    let mut keepers = KEEPERS.lock().unwrap_or_else(PoisonError::into_inner);
    keepers.get_or_insert_with(HashMap::new).insert(file.to_owned(), finished);
    Ok(fcntl_dupfd_cloexec(OwnedFd::from(writing), KEPT_ABOVE).map_err(io::Error::from)?)
}

/// For a reader of `file` once its writer has gone, as QEMU's last words are read: waits a moment
/// for the keeper this process started to write the last of it.
pub fn settled(file: &Path) {
    let finished = KEEPERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_mut()
        .and_then(|keepers| keepers.remove(file));
    if let Some(finished) = finished {
        let _ = finished.recv_timeout(SETTLE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;
    use std::fs;

    #[test]
    fn a_log_under_its_size_is_kept_whole() {
        let scratch = Scratch::new();
        let file = scratch.path("serial.log");
        keep(&b"BdsDxe: starting Boot0001\nmore\n"[..], &file, 1024).unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"BdsDxe: starting Boot0001\nmore\n");
    }

    /// A guest writing without end: the file stays under its size, with its start and its newest end.
    #[test]
    fn a_log_past_its_size_keeps_its_start_and_its_newest_end() {
        let scratch = Scratch::new();
        let file = scratch.path("serial.log");
        let most = 64 * 1024;
        let mut written = b"first boot line\n".to_vec();
        written.extend(std::iter::repeat_n(b'x', 10 * most as usize));
        written.extend_from_slice(b"\nthe newest line\n");
        keep(&written[..], &file, most).unwrap();
        let kept = fs::read(&file).unwrap();
        assert!(kept.len() as u64 <= most, "{} bytes kept", kept.len());
        assert!(kept.starts_with(b"first boot line\n"));
        assert!(kept.ends_with(b"\nthe newest line\n"));
        assert!(kept.windows(LEFT_OUT.len()).any(|part| part == LEFT_OUT));
    }

    /// The run folder is QEMU's to write in: a link put where the log goes leads nowhere.
    #[test]
    fn a_link_where_the_log_goes_is_not_followed() {
        let scratch = Scratch::new();
        let elsewhere = scratch.write("elsewhere.txt", "the user's");
        let file = scratch.path("serial.log");
        std::os::unix::fs::symlink(&elsewhere, &file).unwrap();
        assert!(keep(&b"words"[..], &file, 1024).is_err());
        assert_eq!(fs::read_to_string(&elsewhere).unwrap(), "the user's");
    }

    #[test]
    fn a_start_puts_a_new_file_where_a_link_was() {
        let scratch = Scratch::new();
        let context = crate::testing::scratch_context(&scratch);
        let elsewhere = scratch.write("elsewhere.txt", "the user's");
        let file = scratch.path("output");
        std::os::unix::fs::symlink(&elsewhere, &file).unwrap();
        drop(start(&context, &file, 1024).unwrap());
        settled(&file);
        assert!(fs::symlink_metadata(&file).unwrap().is_file());
        assert_eq!(fs::read_to_string(&elsewhere).unwrap(), "the user's");
    }

    #[test]
    fn one_read_bigger_than_the_whole_size_still_fits() {
        let scratch = Scratch::new();
        let file = scratch.path("out");
        keep(&vec![b'y'; CHUNK][..], &file, 1000).unwrap();
        assert!(fs::metadata(&file).unwrap().len() <= 1000);
    }
}
