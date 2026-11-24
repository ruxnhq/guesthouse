//! The bytes of 9P2000.L: little-endian numbers, strings and qids, and the message header.

use crate::{Error, Result};

/// The size, type and tag that start every message.
pub const HEADER: usize = 7;
/// The tag of a `Tversion`, which no other request may use.
pub const NOTAG: u16 = 0xFFFF;
/// The fid that means "no fid", as `Tattach` sends for the authentication fid.
pub const NOFID: u32 = 0xFFFF_FFFF;

/// Message types, as 9P2000.L numbers them. A reply is its request plus one.
pub mod msg {
    pub const RLERROR: u8 = 7;
    pub const TSTATFS: u8 = 8;
    pub const TLOPEN: u8 = 12;
    pub const TLCREATE: u8 = 14;
    pub const TGETATTR: u8 = 24;
    pub const TSETATTR: u8 = 26;
    pub const TREADDIR: u8 = 40;
    pub const TFSYNC: u8 = 50;
    pub const TMKDIR: u8 = 72;
    pub const TRENAMEAT: u8 = 74;
    pub const TUNLINKAT: u8 = 76;
    pub const TVERSION: u8 = 100;
    pub const TATTACH: u8 = 104;
    pub const TWALK: u8 = 110;
    pub const TREAD: u8 = 116;
    pub const TWRITE: u8 = 118;
    pub const TCLUNK: u8 = 120;
}

/// Builds a message's body. Numbers go in little-endian.
#[derive(Default)]
pub struct Encoder {
    pub(crate) buf: Vec<u8>,
}

impl Encoder {
    /// Starts a message: the size is left for `finish`.
    pub fn new(ty: u8, tag: u16) -> Self {
        let mut buf = Vec::with_capacity(64);
        buf.extend_from_slice(&[0; 4]);
        buf.push(ty);
        buf.extend_from_slice(&tag.to_le_bytes());
        Encoder { buf }
    }

    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }

    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// A string: a u16 length, then the UTF-8. A string over 65535 bytes is an error.
    pub fn str(&mut self, s: &str) -> Result<&mut Self> {
        let len = u16::try_from(s.len()).map_err(|_| Error::Malformed("a string is too long"))?;
        self.u16(len);
        self.buf.extend_from_slice(s.as_bytes());
        Ok(self)
    }

    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(b);
        self
    }

    /// Writes the size in front and gives the message.
    pub fn finish(mut self) -> Result<Vec<u8>> {
        let len = u32::try_from(self.buf.len()).map_err(|_| Error::Malformed("a message is too long"))?;
        self.buf[..4].copy_from_slice(&len.to_le_bytes());
        Ok(self.buf)
    }
}

/// Reads a message's body, and says `Truncated` rather than reading past its end.
pub struct Decoder<'a> {
    data: &'a [u8],
}

impl<'a> Decoder<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Decoder { data }
    }

    /// How many bytes are left.
    pub fn remaining(&self) -> usize {
        self.data.len()
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.data.len() {
            return Err(Error::Truncated);
        }
        let (head, tail) = self.data.split_at(n);
        self.data = tail;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut out = [0; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    pub fn str(&mut self) -> Result<String> {
        let len = usize::from(self.u16()?);
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| Error::Malformed("a string is not UTF-8"))
    }

    pub fn qid(&mut self) -> Result<Qid> {
        Ok(Qid { ty: self.u8()?, version: self.u32()?, path: self.u64()? })
    }

    /// Succeeds only if the whole body was read, so a reply with extra bytes is refused.
    pub fn done(&self) -> Result<()> {
        if self.data.is_empty() { Ok(()) } else { Err(Error::Malformed("a reply has extra bytes")) }
    }
}

/// What the server calls a file: its kind, a version that changes with it, and a number that is
/// its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Qid {
    pub ty: u8,
    pub version: u32,
    pub path: u64,
}

impl Qid {
    /// A qid is 13 bytes on the wire.
    pub const SIZE: usize = 13;
    pub const DIR: u8 = 0x80;
    pub const APPEND: u8 = 0x40;
    pub const EXCL: u8 = 0x20;
    pub const MOUNT: u8 = 0x10;
    pub const AUTH: u8 = 0x08;
    pub const TMP: u8 = 0x04;
    pub const SYMLINK: u8 = 0x02;
    pub const FILE: u8 = 0x00;

    pub fn is_dir(&self) -> bool {
        self.ty & Self::DIR != 0
    }

    pub fn put(&self, e: &mut Encoder) {
        e.u8(self.ty).u32(self.version).u64(self.path);
    }
}
