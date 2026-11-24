//! What can go wrong: the transport, a server that breaks the protocol, or a server that says no.

use std::fmt;
use std::io;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    /// The transport failed, or ended in the middle of a message.
    Io(io::Error),
    /// The server answered with `Rlerror`. The number is the Linux errno.
    Remote(u32),
    /// A message ended before what it said it held.
    Truncated,
    /// A reply's size is under the header or over the negotiated msize.
    BadSize { size: u32, max: u32 },
    /// A reply of another type than the request's.
    UnexpectedType { expected: u8, got: u8 },
    /// A reply for another request than the one in flight.
    UnexpectedTag { expected: u16, got: u16 },
    /// The server speaks another version than 9P2000.L.
    BadVersion(String),
    /// The server's msize is over the client's, or too small to use.
    BadMsize(u32),
    /// A request that wouldn't fit in msize, or `walk` with more names than 9P allows.
    TooLarge,
    /// The server sent something that doesn't fit the protocol.
    Malformed(&'static str),
}

impl Error {
    /// The Linux errno, if the server sent one.
    pub fn errno(&self) -> Option<Errno> {
        match self {
            Error::Remote(n) => Some(Errno::from_raw(*n)),
            _ => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "9P transport: {e}"),
            Error::Remote(n) => write!(f, "9P server error {n} ({:?})", Errno::from_raw(*n)),
            Error::Truncated => f.write_str("9P message is cut short"),
            Error::BadSize { size, max } => write!(f, "9P message size {size} is outside 7..={max}"),
            Error::UnexpectedType { expected, got } => {
                write!(f, "9P reply type {got}, expected {expected}")
            }
            Error::UnexpectedTag { expected, got } => {
                write!(f, "9P reply tag {got}, expected {expected}")
            }
            Error::BadVersion(v) => write!(f, "9P server speaks {v:?}, not 9P2000.L"),
            Error::BadMsize(n) => write!(f, "9P server's msize {n} is not usable"),
            Error::TooLarge => f.write_str("9P request is too large"),
            Error::Malformed(why) => write!(f, "9P message is malformed: {why}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

/// The Linux errnos a host folder's server is likely to send, and the rest as they came. The guest
/// program turns these into NTSTATUS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Errno {
    NoEntry,
    AccessDenied,
    Exists,
    NotDir,
    IsDir,
    NotEmpty,
    NoSpace,
    ReadOnly,
    Invalid,
    Io,
    Other(u32),
}

impl Errno {
    pub const ENOENT: u32 = 2;
    pub const EIO: u32 = 5;
    pub const EACCES: u32 = 13;
    pub const EEXIST: u32 = 17;
    pub const ENOTDIR: u32 = 20;
    pub const EISDIR: u32 = 21;
    pub const EINVAL: u32 = 22;
    pub const ENOSPC: u32 = 28;
    pub const EROFS: u32 = 30;
    pub const ENOTEMPTY: u32 = 39;

    pub fn from_raw(n: u32) -> Errno {
        match n {
            Self::ENOENT => Errno::NoEntry,
            Self::EIO => Errno::Io,
            Self::EACCES => Errno::AccessDenied,
            Self::EEXIST => Errno::Exists,
            Self::ENOTDIR => Errno::NotDir,
            Self::EISDIR => Errno::IsDir,
            Self::EINVAL => Errno::Invalid,
            Self::ENOSPC => Errno::NoSpace,
            Self::EROFS => Errno::ReadOnly,
            Self::ENOTEMPTY => Errno::NotEmpty,
            other => Errno::Other(other),
        }
    }
}
