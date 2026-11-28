//! Files between this PC and a Windows guest: what is dropped on the viewer, and what is dragged out
//! of it.
//!
//! A helper in the guest user's session does the guest's half, as only a program there can start a
//! drag or see one. It listens on a port of its own that QEMU forwards from loopback, and takes one
//! request a connection: a line that starts with the machine's token, then a file's bytes if the
//! request carries any. Files go straight from where they are to where they are wanted, so there is
//! no folder both sides share, and no limit on a file's size.

mod fetching;
mod names;
mod sending;
#[cfg(test)]
mod tests;

pub use fetching::{Entry, fetch};
pub use sending::{DroppedFile, dropped_files, send};

use crate::text::{base64_decode, base64_encode};
use std::io::{self, BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

/// What the helper in `guest/windows/files.ps1` calls itself. They change together.
const VERSION: &str = "files 1";
const CONNECT: Duration = Duration::from_secs(3);
/// How long the guest may take over a part of a file.
const PATIENCE: Duration = Duration::from_secs(30);
/// How long the helper may take over a request with no file in it. A helper that isn't there
/// says nothing at all, as Windows' firewall drops what comes for a port nothing listens on, and
/// the guest's pointer may be waiting for the answer.
const SOON: Duration = Duration::from_secs(2);
/// On top of the time a wait or a catch was given, for the answer's way back.
const AFTER_A_WAIT: Duration = Duration::from_secs(1);

/// How far the drag of dropped files has got in the guest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragState {
    Idle,
    /// The helper's window is under the pointer, waiting for the press.
    Shown,
    Dragging,
    /// What was under the pointer took the files.
    Taken,
    /// Nothing took them, so the helper put them on the Desktop.
    OnDesktop,
    Cancelled,
}

impl DragState {
    fn from_word(word: &str) -> Option<Self> {
        Some(match word {
            "idle" => Self::Idle,
            "shown" => Self::Shown,
            "dragging" => Self::Dragging,
            "taken" => Self::Taken,
            "desktop" => Self::OnDesktop,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }

    pub fn has_started(self) -> bool {
        self != Self::Shown
    }

    pub fn has_ended(self) -> bool {
        !matches!(self, Self::Shown | Self::Dragging)
    }
}

/// What a wait is for.
#[derive(Clone, Copy)]
pub enum Until {
    Started,
    Ended,
}

/// The way to one guest's helper.
#[derive(Clone)]
pub struct Helper {
    port: u16,
    token: String,
}

fn refused(answer: &str) -> io::Error {
    io::Error::other(format!("the guest's helper answered {answer:?}"))
}

impl Helper {
    pub fn new(port: u16, token: &str) -> Self {
        Self { port, token: token.to_owned() }
    }

    fn ask(&self, request: &str, patience: Duration) -> io::Result<TcpStream> {
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, self.port));
        let mut stream = TcpStream::connect_timeout(&address, CONNECT)?;
        stream.set_read_timeout(Some(patience))?;
        stream.set_write_timeout(Some(PATIENCE))?;
        writeln!(stream, "{} {request}", self.token)?;
        Ok(stream)
    }

    fn answer(stream: impl Read) -> io::Result<String> {
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line)?;
        // The helper closes without a word on a wrong token, and QEMU does when nothing listens.
        if line.is_empty() {
            return Err(ErrorKind::ConnectionRefused.into());
        }
        Ok(line.trim_end().to_owned())
    }

    fn expect(&self, request: &str, wanted: &str) -> io::Result<()> {
        let answer = Self::answer(self.ask(request, SOON)?)?;
        if answer == wanted { Ok(()) } else { Err(refused(&answer)) }
    }

    /// Whether the helper is there, and is this app's: one left by another version is not.
    pub fn answers(&self) -> bool {
        self.expect("hello", VERSION).is_ok()
    }

    /// Puts a file into a drop. `name` is its place there, with `/` between folders.
    pub fn put(&self, drop: &str, name: &str, file: &mut impl Read, length: u64) -> io::Result<()> {
        let request = format!("put {drop} {} {length}", base64_encode(name.as_bytes()));
        let mut stream = self.ask(&request, PATIENCE)?;
        let sent = io::copy(&mut file.take(length), &mut stream)?;
        if sent != length {
            return Err(io::Error::new(ErrorKind::UnexpectedEof, "the file got shorter while it was read"));
        }
        let answer = Self::answer(stream)?;
        if answer == "ok" { Ok(()) } else { Err(refused(&answer)) }
    }

    pub fn folder(&self, drop: &str, name: &str) -> io::Result<()> {
        self.expect(&format!("folder {drop} {}", base64_encode(name.as_bytes())), "ok")
    }

    /// Shows the helper's window under the guest's pointer, for the press that starts the drag.
    /// False where a press couldn't reach it, like the lock screen: nothing is shown then.
    pub fn drag(&self, drop: &str) -> io::Result<bool> {
        match Self::answer(self.ask(&format!("drag {drop}"), SOON)?)?.as_str() {
            "shown" => Ok(true),
            "covered" => Ok(false),
            answer => Err(refused(answer)),
        }
    }

    /// How the drag stands once it has got as far as `until`, or when the time is up.
    pub fn wait(&self, until: Until, time: Duration) -> io::Result<DragState> {
        let what = match until {
            Until::Started => "started",
            Until::Ended => "ended",
        };
        let stream = self.ask(&format!("wait {what} {}", time.as_millis()), time + AFTER_A_WAIT)?;
        let answer = Self::answer(stream)?;
        DragState::from_word(&answer).ok_or_else(|| refused(&answer))
    }

    /// Moves a drop that had no drag to the guest's Desktop.
    pub fn keep(&self, drop: &str) -> io::Result<()> {
        self.expect(&format!("keep {drop}"), "desktop")
    }

    /// Forgets a drop. A drag of it that has started ends when the guest's pointer next moves.
    pub fn cancel(&self, drop: &str) -> io::Result<()> {
        self.expect(&format!("cancel {drop}"), "ok")
    }

    /// What a drag in the guest carries, if there is one under its pointer and it has files. The
    /// helper looks for as long as `time`.
    pub fn catch(&self, time: Duration) -> io::Result<Option<Vec<Entry>>> {
        let stream = self.ask(&format!("catch {}", time.as_millis()), time + AFTER_A_WAIT)?;
        let mut lines = BufReader::new(stream).lines();
        match lines.next().transpose()?.as_deref() {
            Some("files") => {}
            Some("none") => return Ok(None),
            answer => return Err(refused(answer.unwrap_or_default())),
        }
        let mut entries = Vec::new();
        for line in lines {
            entries.push(Entry::from_line(&line?).ok_or(ErrorKind::InvalidData)?);
        }
        Ok(Some(entries))
    }

    /// Copies one file of the last drag caught into `to`, and says how long it was.
    pub fn get(&self, name: &str, to: &mut impl Write) -> io::Result<u64> {
        let stream = self.ask(&format!("get {}", base64_encode(name.as_bytes())), PATIENCE)?;
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let length = line.trim_end().strip_prefix("ok ").and_then(|length| length.parse::<u64>().ok());
        let length = length.ok_or_else(|| refused(line.trim_end()))?;
        if io::copy(&mut reader.take(length), to)? != length {
            return Err(ErrorKind::UnexpectedEof.into());
        }
        Ok(length)
    }
}

fn unpacked(name: &str) -> Option<String> {
    String::from_utf8(base64_decode(name)?).ok()
}
