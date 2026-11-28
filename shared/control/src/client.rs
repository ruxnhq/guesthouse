//! The app's end of the service's pipe. The service makes a new pipe instance for each request, so
//! between two requests the pipe is away for a moment, and another window's request may hold it: an
//! open that fails is tried again a few times before the service counts as not running.
//!
//! Each request carries the pipe's version, and a service of another version is told from one
//! whose answer can't be read (see `protocol`).

use crate::protocol::{from_other_version, request_line};
use crate::{Answer, PIPE, Request, read_line, write_line};
use serde_json::Value;
use std::fs::{File, OpenOptions};
use std::io::BufReader;
use std::thread;
use std::time::Duration;

/// How many times an open is tried, and how long apart.
const TRIES: u32 = 20;
const APART: Duration = Duration::from_millis(30);
/// Windows' answer to an open the pipe's security refuses.
const ACCESS_DENIED: i32 = 5;
/// Windows' answer to a read once the service has disconnected its end of the pipe.
const PIPE_NOT_CONNECTED: i32 = 233;
/// What the app asks of the pipe: to read it and write to it (`FILE_GENERIC_READ |
/// FILE_WRITE_DATA`), which is all the pipe's security grants a user. Asking to write as a file is
/// written would ask to append too, which for a pipe is making an instance of it, and is refused.
#[cfg(windows)]
const PIPE_ACCESS: u32 = 0x0012_008b;

/// Why the service couldn't be asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unreached {
    /// Nothing answers: the service isn't installed or isn't running.
    NoService,
    /// The pipe is there, but Windows refused it to this user.
    Refused,
    /// The service answered with something that isn't an answer, or nothing at all.
    Garbled,
    /// The service is of another version than the app, older or newer.
    OtherVersion,
}

impl Unreached {
    /// In words for the person.
    pub fn words(self) -> &'static str {
        match self {
            Unreached::NoService => "The Guesthouse service isn't running.",
            Unreached::Refused => {
                "The Guesthouse service refused this user. Only the signed-in user may use it."
            }
            Unreached::Garbled => "The Guesthouse service's answer couldn't be read.",
            Unreached::OtherVersion => crate::protocol::OTHER_VERSION,
        }
    }
}

/// Asks the service one request, and gives its one answer.
pub fn ask(request: &Request) -> Result<Answer, Unreached> {
    let mut answer = None;
    stream(request, |each| {
        answer = Some(each);
        false
    })?;
    answer.ok_or(Unreached::Garbled)
}

/// Asks the service one request, and hands each answer to `each` as it comes, until the service
/// ends it or `each` returns false.
pub fn stream(request: &Request, mut each: impl FnMut(Answer) -> bool) -> Result<(), Unreached> {
    let mut pipe = open()?;
    write_line(&mut pipe, &request_line(request)).map_err(|_| Unreached::NoService)?;
    let mut reader = BufReader::new(pipe);
    loop {
        match read_line::<Value>(&mut reader) {
            Ok(Some(line)) => {
                if !each(answer_from(line)?) {
                    return Ok(());
                }
            }
            Ok(None) => return Ok(()),
            // The service closed its end, which a service older than this app said this way.
            Err(error) if error.raw_os_error() == Some(PIPE_NOT_CONNECTED) => return Ok(()),
            Err(_) => return Err(Unreached::Garbled),
        }
    }
}

/// The answer in a line the service wrote. A service of another version is told apart before the
/// rest is read, as its states may not be this app's.
pub fn answer_from(line: Value) -> Result<Answer, Unreached> {
    if from_other_version(&line) {
        return Err(Unreached::OtherVersion);
    }
    serde_json::from_value(line).map_err(|_| Unreached::Garbled)
}

fn open() -> Result<File, Unreached> {
    for _ in 0..TRIES {
        match options().open(PIPE) {
            Ok(pipe) => return Ok(pipe),
            Err(error) if error.raw_os_error() == Some(ACCESS_DENIED) => return Err(Unreached::Refused),
            // Not there, or busy with another request: wait a moment and try again.
            Err(_) => thread::sleep(APART),
        }
    }
    Err(Unreached::NoService)
}

#[cfg(windows)]
fn options() -> OpenOptions {
    use std::os::windows::fs::OpenOptionsExt;
    let mut options = OpenOptions::new();
    options.access_mode(PIPE_ACCESS);
    options
}

#[cfg(not(windows))]
fn options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    options
}
