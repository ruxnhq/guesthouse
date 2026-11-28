//! The card's sensors publisher: the one process that writes a card's shared readings, which the
//! guests' daemons on that card and the app only read. See `guesthouse-sensors`.
//!
//! A daemon starts it, as `guesthouse-daemon --publish-sensors MAKER [ADDRESS]`, before it puts
//! itself in its jail, so the publisher is in no jail and serves no guest. It ends with the daemon
//! that started it: its input is a pipe whose other end only that daemon holds. Each daemon on a card
//! starts one, and the one holding the writer's turn reads the card. When its daemon ends, the turn
//! passes to the next, as each waits for it.

use guesthouse_backend::PciAddress;
use std::io::{self, Read};
use std::os::fd::{FromRawFd, OwnedFd};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// The first word of a publisher's command line, which no folder a daemon serves starts with.
pub const WORD: &str = "--publish-sensors";

/// The card makers whose readings a publisher reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Maker {
    Nvidia,
    Intel,
}

impl Maker {
    pub fn word(self) -> &'static str {
        match self {
            Self::Nvidia => "nvidia",
            Self::Intel => "intel",
        }
    }

    pub fn from_word(word: &str) -> Option<Self> {
        [Self::Nvidia, Self::Intel].into_iter().find(|maker| maker.word() == word)
    }
}

/// The daemon's end of the publisher's pipe, kept for as long as the daemon runs.
static KEPT_END: OnceLock<OwnedFd> = OnceLock::new();

/// A pipe, both ends closed when a program is run.
fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut ends = [0; 2];
    // SAFETY: `ends` has room for the two descriptors.
    if unsafe { libc::pipe2(ends.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both were just made, and nothing else owns them.
    Ok(unsafe { (OwnedFd::from_raw_fd(ends[0]), OwnedFd::from_raw_fd(ends[1])) })
}

/// Makes `files` ready for a reader, empty if they aren't there yet, in a folder only the user can
/// open: a jail can only name a file that is there.
pub fn make_ready(files: &[PathBuf]) -> io::Result<()> {
    for folder in files.iter().filter_map(|file| file.parent()) {
        guesthouse_sensors::words::make_private_folder(folder)?;
    }
    files.iter().try_for_each(|file| crate::jail::make_if_missing(file))
}

/// The files a publisher for the `maker` card at `card` writes.
pub fn files(maker: Maker, card: Option<&PciAddress>) -> Vec<PathBuf> {
    let card = card.map(ToString::to_string);
    let snapshot = guesthouse_sensors::snapshot_path_for(card.as_deref());
    match maker {
        Maker::Nvidia => vec![snapshot, guesthouse_sensors::card_processes::table_path_for(card.as_deref())],
        Maker::Intel => vec![snapshot],
    }
}

/// Starts this daemon's publisher for the `maker` card at `card`, or with none the one setup chose.
/// Call it before the jail. Its files are made ready first, so a reader can open them before its
/// first publish. It can only be started once in a process.
pub fn start(maker: Maker, card: Option<&PciAddress>) -> io::Result<()> {
    if KEPT_END.get().is_some() {
        return Err(io::Error::other("this daemon's publisher is already started"));
    }
    make_ready(&files(maker, card))?;
    let (watched, kept) = pipe()?;
    let mut command = Command::new(std::env::current_exe()?);
    command.arg(WORD).arg(maker.word()).args(card.map(ToString::to_string)).stdin(Stdio::from(watched));
    // Nothing waits for it: it ends after this process, or stays waiting for that.
    drop(command.spawn()?);
    let _ = KEPT_END.set(kept);
    Ok(())
}

/// Closes every descriptor the publisher was given but its input and its two outputs: the daemon's
/// memory and sockets are no business of this process.
fn close_the_rest() {
    // SAFETY: closing descriptors this process doesn't use; nothing here holds any yet.
    unsafe { libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, 0u32) };
}

/// Ends this process once its input ends, which is when the daemon that started it has gone.
fn end_with_the_daemon() {
    std::thread::spawn(|| {
        let mut input = io::stdin();
        let mut byte = [0u8; 1];
        loop {
            match input.read(&mut byte) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Ok(0) | Err(_) => std::process::exit(0),
                Ok(_) => {}
            }
        }
    });
}

/// The publisher's whole run, from its command line after [`WORD`]. It returns only for words it
/// can't read: otherwise it publishes, or waits for its turn, until the daemon that started it ends.
pub fn run(words: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let (maker, card) = match words {
        [maker] => (maker, None),
        [maker, card] => (maker, Some(PciAddress::parse(card).ok_or("not a PCI address")?)),
        _ => return Err("usage: guesthouse-daemon --publish-sensors MAKER [ADDRESS]".into()),
    };
    let maker = Maker::from_word(maker).ok_or("not a card maker with readings")?;
    close_the_rest();
    end_with_the_daemon();
    if let Err(error) = publish(maker, card.as_ref()) {
        log::info!("sensors publisher: no host sensors: {error}");
    }
    loop {
        std::thread::park();
    }
}

#[cfg(feature = "nvidia")]
fn publish_nvidia(card: Option<&PciAddress>) -> io::Result<()> {
    guesthouse_backend_nvidia::sensors::publish(card)
}

#[cfg(not(feature = "nvidia"))]
fn publish_nvidia(_card: Option<&PciAddress>) -> io::Result<()> {
    Err(io::Error::other("this build has no NVIDIA backend"))
}

#[cfg(feature = "intel")]
fn publish_intel(card: Option<&PciAddress>) -> io::Result<()> {
    guesthouse_backend_intel::sensors::publish(card)
}

#[cfg(not(feature = "intel"))]
fn publish_intel(_card: Option<&PciAddress>) -> io::Result<()> {
    Err(io::Error::other("this build has no Intel backend"))
}

fn publish(maker: Maker, card: Option<&PciAddress>) -> io::Result<()> {
    match maker {
        Maker::Nvidia => publish_nvidia(card),
        Maker::Intel => publish_intel(card),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_maker_is_known_by_its_word() {
        for maker in [Maker::Nvidia, Maker::Intel] {
            assert_eq!(Maker::from_word(maker.word()), Some(maker));
        }
        assert_eq!(Maker::from_word("amd"), None);
    }

    #[test]
    fn words_it_cant_read_are_refused() {
        assert!(run(&[]).is_err());
        assert!(run(&["amd".to_owned()]).is_err());
        assert!(run(&["nvidia".to_owned(), "not an address".to_owned()]).is_err());
    }
}
