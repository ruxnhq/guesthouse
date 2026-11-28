//! A 9P2000.L client, for a Windows guest to read and write a folder on the host. The host's end is
//! Hyper-V's own Plan 9 server (the `Plan9` device HCS gives a machine), which speaks 9P2000.L over
//! a Hyper-V socket.
//!
//! The client works over anything that is `Read + Write`, so it holds no socket and no Windows
//! calls, and it is tested on Linux against a server that lives in memory. Every reply is checked
//! against the size the client and server agreed on, and a bad one is an error, never a panic.
//!
//! A later guest program maps WinFsp's file-system calls onto these.

mod consts;
mod error;
mod fids;
mod wire;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod testserver;

pub use consts::{AT_REMOVEDIR, dt, getattr, mode, open, setattr};
pub use error::{Errno, Error, Result};
pub use fids::FidPool;
pub use wire::{NOFID, Qid};

use std::collections::HashMap;
use std::io::{Read, Write};
use wire::{Decoder, Encoder, HEADER, NOTAG, msg};

/// The version string 9P2000.L's clients and servers agree on.
pub const VERSION: &str = "9P2000.L";
/// What `read`, `write` and `readdir` leave of msize for the message's own fields.
pub const IOHDRSZ: u32 = 24;
/// The most names one `walk` may hold.
pub const MAX_WALK: usize = 16;
/// The smallest msize the client uses. The server's must leave room for a header and a little data.
pub const MIN_MSIZE: u32 = 128;
/// The msize `Client::new` asks for if the caller doesn't say.
pub const DEFAULT_MSIZE: u32 = 64 * 1024;

/// Seconds and nanoseconds, as `getattr` and `setattr` carry times.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Timespec {
    pub sec: u64,
    pub nsec: u64,
}

/// What `getattr` answers. `valid` says which fields the server filled in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Attr {
    pub valid: u64,
    pub qid: Qid,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub nlink: u64,
    pub rdev: u64,
    pub size: u64,
    pub blksize: u64,
    pub blocks: u64,
    pub atime: Timespec,
    pub mtime: Timespec,
    pub ctime: Timespec,
    pub btime: Timespec,
    pub generation: u64,
    pub data_version: u64,
}

/// What `setattr` changes. Only the fields whose bit is in `valid` count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SetAttr {
    pub valid: u32,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub atime: Timespec,
    pub mtime: Timespec,
}

/// What `statfs` answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StatFs {
    pub fs_type: u32,
    pub bsize: u32,
    pub blocks: u64,
    pub bfree: u64,
    pub bavail: u64,
    pub files: u64,
    pub ffree: u64,
    pub fsid: u64,
    pub namelen: u32,
}

/// One entry of a directory. `offset` is what to give `readdir` to carry on after this entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub qid: Qid,
    pub offset: u64,
    pub ty: u8,
    pub name: String,
}

/// A 9P2000.L client on a transport.
///
/// It is not thread-safe, and not meant to be: it sends one request and reads its reply before the
/// next, with the transport's one stream and one buffer, so two threads would mix their messages.
/// The guest program holds it behind a mutex.
pub struct Client<T: Read + Write> {
    io: T,
    msize: u32,
    tag: u16,
    fids: FidPool,
    iounits: HashMap<u32, u32>,
}

impl<T: Read + Write> Client<T> {
    /// A client that has not yet said `version`.
    pub fn new(io: T) -> Self {
        Client { io, msize: DEFAULT_MSIZE, tag: 0, fids: FidPool::new(), iounits: HashMap::new() }
    }

    /// The msize in use: what `version` asked for until the server answers, then what it chose.
    pub fn msize(&self) -> u32 {
        self.msize
    }

    /// Gives back the transport.
    pub fn into_inner(self) -> T {
        self.io
    }

    /// A fid nothing else is using. `clunk` gives it back.
    pub fn new_fid(&mut self) -> u32 {
        self.fids.alloc()
    }

    /// Gives back a fid from `new_fid` that never reached the server, as when a walk found
    /// nothing. A fid the server holds is given back by `clunk`.
    pub fn release_fid(&mut self, fid: u32) {
        self.fids.release(fid);
    }

    fn next_tag(&mut self) -> u16 {
        self.tag = self.tag.wrapping_add(1);
        if self.tag == NOTAG {
            self.tag = 0;
        }
        self.tag
    }

    /// Sends a request and reads its reply, and gives the reply's body. `Rlerror` is an error.
    fn call(&mut self, req: Encoder) -> Result<Vec<u8>> {
        let mut buf = req.finish()?;
        let ty = buf[4];
        let tag = if ty == msg::TVERSION { NOTAG } else { self.next_tag() };
        buf[5..7].copy_from_slice(&tag.to_le_bytes());
        if buf.len() > self.msize as usize {
            return Err(Error::TooLarge);
        }
        self.io.write_all(&buf)?;
        self.io.flush()?;

        let mut size = [0; 4];
        self.io.read_exact(&mut size)?;
        let size = u32::from_le_bytes(size);
        if (size as usize) < HEADER || size > self.msize {
            return Err(Error::BadSize { size, max: self.msize });
        }
        // The size is within msize, so this allocation is as well.
        let mut rest = vec![0; size as usize - 4];
        self.io.read_exact(&mut rest)?;
        let rty = rest[0];
        let rtag = u16::from_le_bytes([rest[1], rest[2]]);
        if rtag != tag {
            return Err(Error::UnexpectedTag { expected: tag, got: rtag });
        }
        if rty == msg::RLERROR {
            let mut d = Decoder::new(&rest[3..]);
            let errno = d.u32()?;
            d.done()?;
            return Err(Error::Remote(errno));
        }
        if rty != ty + 1 {
            return Err(Error::UnexpectedType { expected: ty + 1, got: rty });
        }
        rest.drain(..3);
        Ok(rest)
    }

    /// Agrees on 9P2000.L and a message size. The server may choose a smaller msize than asked
    /// for, and that's the one used from then on; a larger one, or one too small to use, is
    /// refused, as is any other version. It gives the msize chosen.
    pub fn version(&mut self, msize: u32) -> Result<u32> {
        if msize < MIN_MSIZE {
            return Err(Error::BadMsize(msize));
        }
        self.msize = msize;
        let mut e = Encoder::new(msg::TVERSION, NOTAG);
        e.u32(msize).str(VERSION)?;
        let body = self.call(e)?;
        let mut d = Decoder::new(&body);
        let got = d.u32()?;
        let version = d.str()?;
        d.done()?;
        if version != VERSION {
            return Err(Error::BadVersion(version));
        }
        if !(MIN_MSIZE..=msize).contains(&got) {
            return Err(Error::BadMsize(got));
        }
        self.msize = got;
        Ok(got)
    }

    /// Attaches `fid` to the root of what the server shares as `aname`. No authentication.
    pub fn attach(&mut self, fid: u32, aname: &str, uname: &str, n_uname: u32) -> Result<Qid> {
        let mut e = Encoder::new(msg::TATTACH, 0);
        e.u32(fid).u32(NOFID).str(uname)?.str(aname)?.u32(n_uname);
        let body = self.call(e)?;
        let mut d = Decoder::new(&body);
        let qid = d.qid()?;
        d.done()?;
        Ok(qid)
    }

    /// Walks from `fid` down `names`, and makes `newfid` of where it ends. With no names `newfid`
    /// is a copy of `fid`. The qids are one for each name walked: fewer than `names` means the
    /// walk stopped, and then `newfid` was not made.
    pub fn walk(&mut self, fid: u32, newfid: u32, names: &[&str]) -> Result<Vec<Qid>> {
        if names.len() > MAX_WALK {
            return Err(Error::TooLarge);
        }
        let mut e = Encoder::new(msg::TWALK, 0);
        e.u32(fid).u32(newfid).u16(names.len() as u16);
        for n in names {
            e.str(n)?;
        }
        let body = self.call(e)?;
        let mut d = Decoder::new(&body);
        let n = usize::from(d.u16()?);
        if n > names.len() {
            return Err(Error::Malformed("a walk found more than it was asked"));
        }
        if d.remaining() != n * Qid::SIZE {
            return Err(Error::Malformed("a walk's qids don't fit its count"));
        }
        let qids = (0..n).map(|_| d.qid()).collect::<Result<Vec<_>>>()?;
        d.done()?;
        Ok(qids)
    }

    pub fn getattr(&mut self, fid: u32, mask: u64) -> Result<Attr> {
        let mut e = Encoder::new(msg::TGETATTR, 0);
        e.u32(fid).u64(mask);
        let body = self.call(e)?;
        let mut d = Decoder::new(&body);
        let valid = d.u64()?;
        let qid = d.qid()?;
        let mode = d.u32()?;
        let uid = d.u32()?;
        let gid = d.u32()?;
        let nlink = d.u64()?;
        let rdev = d.u64()?;
        let size = d.u64()?;
        let blksize = d.u64()?;
        let blocks = d.u64()?;
        let mut time = || -> Result<Timespec> { Ok(Timespec { sec: d.u64()?, nsec: d.u64()? }) };
        let (atime, mtime, ctime, btime) = (time()?, time()?, time()?, time()?);
        let generation = d.u64()?;
        let data_version = d.u64()?;
        d.done()?;
        Ok(Attr {
            valid,
            qid,
            mode,
            uid,
            gid,
            nlink,
            rdev,
            size,
            blksize,
            blocks,
            atime,
            mtime,
            ctime,
            btime,
            generation,
            data_version,
        })
    }

    /// Opens `fid` with Linux open `flags`. It gives the qid and the iounit: the most the server
    /// wants in one read or write, or 0 for no preference.
    pub fn lopen(&mut self, fid: u32, flags: u32) -> Result<(Qid, u32)> {
        let mut e = Encoder::new(msg::TLOPEN, 0);
        e.u32(fid).u32(flags);
        let body = self.call(e)?;
        let out = self.open_reply(&body)?;
        self.iounits.insert(fid, out.1);
        Ok(out)
    }

    /// Makes the file `name` in the directory `fid`, and opens it: `fid` is then the new file.
    pub fn lcreate(&mut self, fid: u32, name: &str, flags: u32, mode: u32, gid: u32) -> Result<(Qid, u32)> {
        let mut e = Encoder::new(msg::TLCREATE, 0);
        e.u32(fid).str(name)?.u32(flags).u32(mode).u32(gid);
        let body = self.call(e)?;
        let out = self.open_reply(&body)?;
        self.iounits.insert(fid, out.1);
        Ok(out)
    }

    fn open_reply(&self, body: &[u8]) -> Result<(Qid, u32)> {
        let mut d = Decoder::new(body);
        let qid = d.qid()?;
        let iounit = d.u32()?;
        d.done()?;
        Ok((qid, iounit))
    }

    /// The most data one message may carry for `fid`: its iounit if it has one, and msize less the
    /// message's own fields in any case.
    fn chunk(&self, fid: u32) -> usize {
        let room = self.msize.saturating_sub(IOHDRSZ);
        match self.iounits.get(&fid) {
            Some(&n) if n != 0 => n.min(room) as usize,
            _ => room as usize,
        }
    }

    /// Reads up to `count` bytes at `offset`, in as many messages as the iounit needs. It gives
    /// fewer bytes if the file ends or the server answers short, and none at the end of the file.
    pub fn read(&mut self, fid: u32, offset: u64, count: u32) -> Result<Vec<u8>> {
        let chunk = self.chunk(fid);
        let mut out = Vec::new();
        while out.len() < count as usize {
            let want = (count as usize - out.len()).min(chunk) as u32;
            let at = offset.checked_add(out.len() as u64).ok_or(Error::TooLarge)?;
            let mut e = Encoder::new(msg::TREAD, 0);
            e.u32(fid).u64(at).u32(want);
            let body = self.call(e)?;
            let mut d = Decoder::new(&body);
            let got = d.u32()?;
            if got > want {
                return Err(Error::Malformed("a read gave more than it was asked"));
            }
            let data = d.take(got as usize)?;
            d.done()?;
            out.extend_from_slice(data);
            if got < want {
                break;
            }
        }
        Ok(out)
    }

    /// Writes `data` at `offset`, in as many messages as the iounit needs, and gives how many
    /// bytes the server took. That is less than `data.len()` if the server answered short.
    pub fn write(&mut self, fid: u32, offset: u64, data: &[u8]) -> Result<usize> {
        let chunk = self.chunk(fid);
        let mut done = 0;
        while done < data.len() {
            let piece = &data[done..data.len().min(done + chunk)];
            let at = offset.checked_add(done as u64).ok_or(Error::TooLarge)?;
            let mut e = Encoder::new(msg::TWRITE, 0);
            e.u32(fid).u64(at).u32(piece.len() as u32).bytes(piece);
            let body = self.call(e)?;
            let mut d = Decoder::new(&body);
            let n = d.u32()? as usize;
            d.done()?;
            if n > piece.len() {
                return Err(Error::Malformed("a write took more than it was given"));
            }
            done += n;
            if n < piece.len() {
                break;
            }
        }
        Ok(done)
    }

    /// Reads the entries of the open directory `fid`, from `offset`: 0 to begin, then the last
    /// entry's `offset`. `count` is the most bytes of entries to ask for. No entries is the end.
    pub fn readdir(&mut self, fid: u32, offset: u64, count: u32) -> Result<Vec<DirEntry>> {
        let count = count.min(self.msize.saturating_sub(IOHDRSZ));
        let mut e = Encoder::new(msg::TREADDIR, 0);
        e.u32(fid).u64(offset).u32(count);
        let body = self.call(e)?;
        let mut d = Decoder::new(&body);
        let n = d.u32()?;
        if n > count {
            return Err(Error::Malformed("a readdir gave more than it was asked"));
        }
        let mut data = Decoder::new(d.take(n as usize)?);
        d.done()?;
        let mut entries = Vec::new();
        while data.remaining() > 0 {
            entries.push(DirEntry {
                qid: data.qid()?,
                offset: data.u64()?,
                ty: data.u8()?,
                name: data.str()?,
            });
        }
        Ok(entries)
    }

    /// Makes the directory `name` in the directory `dfid`.
    pub fn mkdir(&mut self, dfid: u32, name: &str, mode: u32, gid: u32) -> Result<Qid> {
        let mut e = Encoder::new(msg::TMKDIR, 0);
        e.u32(dfid).str(name)?.u32(mode).u32(gid);
        let body = self.call(e)?;
        let mut d = Decoder::new(&body);
        let qid = d.qid()?;
        d.done()?;
        Ok(qid)
    }

    /// Removes `name` from the directory `dirfid`. `flags` is `AT_REMOVEDIR` for a directory.
    pub fn unlinkat(&mut self, dirfid: u32, name: &str, flags: u32) -> Result<()> {
        let mut e = Encoder::new(msg::TUNLINKAT, 0);
        e.u32(dirfid).str(name)?.u32(flags);
        self.empty(e)
    }

    /// Moves `oldname` in the directory `olddirfid` to `newname` in `newdirfid`.
    pub fn renameat(&mut self, olddirfid: u32, oldname: &str, newdirfid: u32, newname: &str) -> Result<()> {
        let mut e = Encoder::new(msg::TRENAMEAT, 0);
        e.u32(olddirfid).str(oldname)?.u32(newdirfid).str(newname)?;
        self.empty(e)
    }

    pub fn setattr(&mut self, fid: u32, attr: &SetAttr) -> Result<()> {
        let mut e = Encoder::new(msg::TSETATTR, 0);
        e.u32(fid).u32(attr.valid).u32(attr.mode).u32(attr.uid).u32(attr.gid).u64(attr.size);
        e.u64(attr.atime.sec).u64(attr.atime.nsec).u64(attr.mtime.sec).u64(attr.mtime.nsec);
        self.empty(e)
    }

    pub fn statfs(&mut self, fid: u32) -> Result<StatFs> {
        let mut e = Encoder::new(msg::TSTATFS, 0);
        e.u32(fid);
        let body = self.call(e)?;
        let mut d = Decoder::new(&body);
        let s = StatFs {
            fs_type: d.u32()?,
            bsize: d.u32()?,
            blocks: d.u64()?,
            bfree: d.u64()?,
            bavail: d.u64()?,
            files: d.u64()?,
            ffree: d.u64()?,
            fsid: d.u64()?,
            namelen: d.u32()?,
        };
        d.done()?;
        Ok(s)
    }

    /// Makes the server write `fid`'s data to disk.
    pub fn fsync(&mut self, fid: u32) -> Result<()> {
        let mut e = Encoder::new(msg::TFSYNC, 0);
        e.u32(fid).u32(0);
        self.empty(e)
    }

    /// Lets go of `fid`, and frees it for `new_fid`. The server forgets the fid even if it
    /// answers with an error, so it is freed then too.
    pub fn clunk(&mut self, fid: u32) -> Result<()> {
        let mut e = Encoder::new(msg::TCLUNK, 0);
        e.u32(fid);
        let out = self.empty(e);
        self.iounits.remove(&fid);
        if matches!(out, Ok(()) | Err(Error::Remote(_))) {
            self.fids.release(fid);
        }
        out
    }

    /// A request whose reply has nothing in it.
    fn empty(&mut self, req: Encoder) -> Result<()> {
        let body = self.call(req)?;
        Decoder::new(&body).done()
    }
}
