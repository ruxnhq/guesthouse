//! A 9P2000.L server in memory, for the tests: a small tree of directories and files, and the
//! requests the client sends. It runs on a thread, on the far end of a pair of channels.

use crate::wire::{Decoder, Encoder, HEADER, Qid, msg};
use crate::{Errno, Result};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;

/// One end of a duplex pipe made of channels. Reading blocks until the other end writes, and
/// gives end of file once the other end is gone.
pub struct Pipe {
    rx: Receiver<Vec<u8>>,
    tx: Sender<Vec<u8>>,
    pending: Vec<u8>,
    pos: usize,
}

pub fn pipe() -> (Pipe, Pipe) {
    let (a_tx, b_rx) = channel();
    let (b_tx, a_rx) = channel();
    (
        Pipe { rx: a_rx, tx: a_tx, pending: Vec::new(), pos: 0 },
        Pipe { rx: b_rx, tx: b_tx, pending: Vec::new(), pos: 0 },
    )
}

impl Read for Pipe {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.pos == self.pending.len() {
            match self.rx.recv() {
                Ok(chunk) => {
                    self.pending = chunk;
                    self.pos = 0;
                }
                Err(_) => return Ok(0),
            }
        }
        let n = out.len().min(self.pending.len() - self.pos);
        out[..n].copy_from_slice(&self.pending[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

impl Write for Pipe {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.tx.send(data.to_vec()).map_err(|_| io::ErrorKind::BrokenPipe)?;
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A transport that answers with bytes made beforehand, and keeps what the client sent.
pub struct Script {
    pub replies: io::Cursor<Vec<u8>>,
    pub sent: Vec<u8>,
}

impl Script {
    pub fn new(replies: Vec<u8>) -> Self {
        Script { replies: io::Cursor::new(replies), sent: Vec::new() }
    }
}

impl Read for Script {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        self.replies.read(out)
    }
}

impl Write for Script {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.sent.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A whole reply message, for a `Script`.
pub fn reply(ty: u8, tag: u16, body: &[u8]) -> Vec<u8> {
    let mut e = Encoder::new(ty, tag);
    e.bytes(body);
    e.finish().unwrap()
}

enum Kind {
    Dir(Vec<(String, u64)>),
    File(Vec<u8>),
}

struct Node {
    kind: Kind,
    mode: u32,
    version: u32,
}

/// How the server behaves, apart from the protocol.
#[derive(Clone, Copy)]
pub struct Config {
    /// The largest msize the server accepts.
    pub msize: u32,
    /// The iounit it gives on open.
    pub iounit: u32,
    /// The most directory entries in one `Rreaddir`.
    pub dir_batch: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config { msize: 4096, iounit: 1000, dir_batch: 2 }
    }
}

struct Fid {
    node: u64,
    open: bool,
}

pub struct Server {
    cfg: Config,
    msize: u32,
    nodes: HashMap<u64, Node>,
    next_node: u64,
    fids: HashMap<u32, Fid>,
}

const ROOT: u64 = 1;

impl Server {
    fn new(cfg: Config) -> Self {
        let mut s =
            Server { cfg, msize: cfg.msize, nodes: HashMap::new(), next_node: ROOT, fids: HashMap::new() };
        let root = s.add(Kind::Dir(Vec::new()), 0o755);
        assert_eq!(root, ROOT);
        s
    }

    fn add(&mut self, kind: Kind, mode: u32) -> u64 {
        let id = self.next_node;
        self.next_node += 1;
        self.nodes.insert(id, Node { kind, mode, version: 0 });
        id
    }

    /// Makes `path` (slash-separated, under the root) with `data`, and any directories on the way.
    pub fn put_file(&mut self, path: &str, data: &[u8]) {
        let mut dir = ROOT;
        let parts: Vec<&str> = path.split('/').collect();
        for (i, part) in parts.iter().enumerate() {
            let last = i == parts.len() - 1;
            let existing = self.lookup(dir, part);
            dir = match existing {
                Some(id) => id,
                None => {
                    let id = if last {
                        self.add(Kind::File(data.to_vec()), 0o644)
                    } else {
                        self.add(Kind::Dir(Vec::new()), 0o755)
                    };
                    self.link(dir, part, id);
                    id
                }
            };
        }
    }

    fn link(&mut self, dir: u64, name: &str, id: u64) {
        if let Kind::Dir(entries) = &mut self.nodes.get_mut(&dir).unwrap().kind {
            entries.push((name.to_string(), id));
        }
    }

    fn lookup(&self, dir: u64, name: &str) -> Option<u64> {
        match &self.nodes.get(&dir)?.kind {
            Kind::Dir(entries) => entries.iter().find(|(n, _)| n == name).map(|(_, id)| *id),
            Kind::File(_) => None,
        }
    }

    fn qid(&self, id: u64) -> Qid {
        let n = &self.nodes[&id];
        Qid {
            ty: if matches!(n.kind, Kind::Dir(_)) { Qid::DIR } else { Qid::FILE },
            version: n.version,
            path: id,
        }
    }

    fn fid(&self, fid: u32) -> std::result::Result<u64, u32> {
        self.fids.get(&fid).map(|f| f.node).ok_or(Errno::EIO)
    }

    /// Answers one request, as a whole reply message. An `Err` is an errno for `Rlerror`.
    fn handle(&mut self, ty: u8, tag: u16, body: &[u8]) -> Vec<u8> {
        match self.dispatch(ty, tag, body) {
            Ok(e) => e.finish().unwrap(),
            Err(errno) => {
                let mut e = Encoder::new(msg::RLERROR, tag);
                e.u32(errno);
                e.finish().unwrap()
            }
        }
    }

    fn dispatch(&mut self, ty: u8, tag: u16, body: &[u8]) -> std::result::Result<Encoder, u32> {
        let bad = |_| Errno::EINVAL;
        let mut d = Decoder::new(body);
        let mut r = Encoder::new(ty + 1, tag);
        match ty {
            msg::TVERSION => {
                let msize = d.u32().map_err(bad)?;
                let v = d.str().map_err(bad)?;
                self.msize = msize.min(self.cfg.msize);
                r.u32(self.msize);
                r.str(if v == "9P2000.L" { "9P2000.L" } else { "unknown" }).unwrap();
            }
            msg::TATTACH => {
                let fid = d.u32().map_err(bad)?;
                let _afid = d.u32().map_err(bad)?;
                let _uname = d.str().map_err(bad)?;
                let aname = d.str().map_err(bad)?;
                if aname != "share" {
                    return Err(Errno::ENOENT);
                }
                self.fids.insert(fid, Fid { node: ROOT, open: false });
                self.qid(ROOT).put(&mut r);
            }
            msg::TWALK => {
                let fid = d.u32().map_err(bad)?;
                let newfid = d.u32().map_err(bad)?;
                let n = d.u16().map_err(bad)?;
                let mut at = self.fid(fid)?;
                let mut qids = Vec::new();
                for i in 0..n {
                    let name = d.str().map_err(bad)?;
                    match self.lookup(at, &name) {
                        Some(next) => {
                            at = next;
                            qids.push(self.qid(next));
                        }
                        None if i == 0 => return Err(Errno::ENOENT),
                        None => break,
                    }
                }
                if qids.len() == usize::from(n) {
                    self.fids.insert(newfid, Fid { node: at, open: false });
                }
                r.u16(qids.len() as u16);
                for q in qids {
                    q.put(&mut r);
                }
            }
            msg::TGETATTR => {
                let id = self.fid(d.u32().map_err(bad)?)?;
                let _mask = d.u64().map_err(bad)?;
                let n = &self.nodes[&id];
                let (is_dir, size) = match &n.kind {
                    Kind::Dir(e) => (true, e.len() as u64),
                    Kind::File(f) => (false, f.len() as u64),
                };
                r.u64(crate::getattr::BASIC);
                self.qid(id).put(&mut r);
                r.u32(n.mode | if is_dir { crate::mode::S_IFDIR } else { crate::mode::S_IFREG });
                r.u32(1000)
                    .u32(1001)
                    .u64(if is_dir { 2 } else { 1 })
                    .u64(0)
                    .u64(size)
                    .u64(4096)
                    .u64(size.div_ceil(512));
                for t in [1_700_000_000u64, 1_700_000_001, 1_700_000_002, 0] {
                    r.u64(t).u64(t % 7);
                }
                r.u64(0).u64(u64::from(n.version));
            }
            msg::TLOPEN => {
                let fid = d.u32().map_err(bad)?;
                let flags = d.u32().map_err(bad)?;
                let id = self.fid(fid)?;
                let node = self.nodes.get_mut(&id).unwrap();
                if let Kind::File(data) = &mut node.kind {
                    if flags & crate::open::O_DIRECTORY != 0 {
                        return Err(Errno::ENOTDIR);
                    }
                    if flags & crate::open::O_TRUNC != 0 {
                        data.clear();
                    }
                } else if flags & crate::open::O_ACCMODE != crate::open::O_RDONLY {
                    return Err(Errno::EISDIR);
                }
                self.fids.get_mut(&fid).unwrap().open = true;
                self.qid(id).put(&mut r);
                r.u32(self.cfg.iounit);
            }
            msg::TLCREATE => {
                let fid = d.u32().map_err(bad)?;
                let name = d.str().map_err(bad)?;
                let _flags = d.u32().map_err(bad)?;
                let mode = d.u32().map_err(bad)?;
                let _gid = d.u32().map_err(bad)?;
                let dir = self.fid(fid)?;
                if !matches!(self.nodes[&dir].kind, Kind::Dir(_)) {
                    return Err(Errno::ENOTDIR);
                }
                if self.lookup(dir, &name).is_some() {
                    return Err(Errno::EEXIST);
                }
                let id = self.add(Kind::File(Vec::new()), mode & 0o7777);
                self.link(dir, &name, id);
                *self.fids.get_mut(&fid).unwrap() = Fid { node: id, open: true };
                self.qid(id).put(&mut r);
                r.u32(self.cfg.iounit);
            }
            msg::TREAD => {
                let fid = d.u32().map_err(bad)?;
                let offset = d.u64().map_err(bad)? as usize;
                let count = d.u32().map_err(bad)? as usize;
                let id = self.fid(fid)?;
                let Kind::File(data) = &self.nodes[&id].kind else { return Err(Errno::EISDIR) };
                // A server may answer with less than asked; this one never gives over the iounit.
                let end = data.len().min(offset.saturating_add(count.min(self.cfg.iounit as usize)));
                let slice = if offset < end { &data[offset..end] } else { &[] };
                r.u32(slice.len() as u32).bytes(slice);
            }
            msg::TWRITE => {
                let fid = d.u32().map_err(bad)?;
                let offset = d.u64().map_err(bad)? as usize;
                let count = d.u32().map_err(bad)? as usize;
                let payload = d.take(count).map_err(bad)?;
                let id = self.fid(fid)?;
                let node = self.nodes.get_mut(&id).unwrap();
                let Kind::File(data) = &mut node.kind else { return Err(Errno::EISDIR) };
                if data.len() < offset + count {
                    data.resize(offset + count, 0);
                }
                data[offset..offset + count].copy_from_slice(payload);
                node.version += 1;
                r.u32(count as u32);
            }
            msg::TREADDIR => {
                let fid = d.u32().map_err(bad)?;
                let offset = d.u64().map_err(bad)? as usize;
                let count = d.u32().map_err(bad)? as usize;
                let id = self.fid(fid)?;
                let Kind::Dir(entries) = &self.nodes[&id].kind else { return Err(Errno::ENOTDIR) };
                let mut out = Encoder::new(0, 0);
                let mut used = 0;
                for (i, (name, child)) in entries.iter().enumerate().skip(offset).take(self.cfg.dir_batch) {
                    let size = Qid::SIZE + 8 + 1 + 2 + name.len();
                    if used + size > count {
                        break;
                    }
                    used += size;
                    let qid = self.qid(*child);
                    qid.put(&mut out);
                    out.u64(i as u64 + 1).u8(if qid.is_dir() { crate::dt::DIR } else { crate::dt::REG });
                    out.str(name).unwrap();
                }
                r.u32(used as u32).bytes(&out.buf[HEADER..]);
            }
            msg::TMKDIR => {
                let dir = self.fid(d.u32().map_err(bad)?)?;
                let name = d.str().map_err(bad)?;
                let mode = d.u32().map_err(bad)?;
                if self.lookup(dir, &name).is_some() {
                    return Err(Errno::EEXIST);
                }
                let id = self.add(Kind::Dir(Vec::new()), mode & 0o7777);
                self.link(dir, &name, id);
                self.qid(id).put(&mut r);
            }
            msg::TUNLINKAT => {
                let dir = self.fid(d.u32().map_err(bad)?)?;
                let name = d.str().map_err(bad)?;
                let flags = d.u32().map_err(bad)?;
                let id = self.lookup(dir, &name).ok_or(Errno::ENOENT)?;
                match &self.nodes[&id].kind {
                    Kind::Dir(e) if flags & crate::AT_REMOVEDIR == 0 => {
                        let _ = e;
                        return Err(Errno::EISDIR);
                    }
                    Kind::Dir(e) if !e.is_empty() => return Err(Errno::ENOTEMPTY),
                    Kind::File(_) if flags & crate::AT_REMOVEDIR != 0 => return Err(Errno::ENOTDIR),
                    _ => {}
                }
                self.unlink(dir, &name);
                self.nodes.remove(&id);
            }
            msg::TRENAMEAT => {
                let old_dir = self.fid(d.u32().map_err(bad)?)?;
                let old = d.str().map_err(bad)?;
                let new_dir = self.fid(d.u32().map_err(bad)?)?;
                let new = d.str().map_err(bad)?;
                let id = self.lookup(old_dir, &old).ok_or(Errno::ENOENT)?;
                if self.lookup(new_dir, &new).is_some() {
                    return Err(Errno::EEXIST);
                }
                self.unlink(old_dir, &old);
                self.link(new_dir, &new, id);
            }
            msg::TSETATTR => {
                let id = self.fid(d.u32().map_err(bad)?)?;
                let valid = d.u32().map_err(bad)?;
                let mode = d.u32().map_err(bad)?;
                let (_uid, _gid) = (d.u32().map_err(bad)?, d.u32().map_err(bad)?);
                let size = d.u64().map_err(bad)? as usize;
                let node = self.nodes.get_mut(&id).unwrap();
                if valid & crate::setattr::MODE != 0 {
                    node.mode = mode & 0o7777;
                }
                if valid & crate::setattr::SIZE != 0 {
                    let Kind::File(data) = &mut node.kind else { return Err(Errno::EISDIR) };
                    data.resize(size, 0);
                }
            }
            msg::TSTATFS => {
                self.fid(d.u32().map_err(bad)?)?;
                r.u32(0x0102_1997).u32(4096).u64(1000).u64(400).u64(300).u64(50).u64(20).u64(7).u32(255);
            }
            msg::TFSYNC => {
                self.fid(d.u32().map_err(bad)?)?;
            }
            msg::TCLUNK => {
                let fid = d.u32().map_err(bad)?;
                self.fids.remove(&fid).ok_or(Errno::EIO)?;
            }
            _ => return Err(Errno::EINVAL),
        }
        Ok(r)
    }

    fn unlink(&mut self, dir: u64, name: &str) {
        if let Kind::Dir(entries) = &mut self.nodes.get_mut(&dir).unwrap().kind {
            entries.retain(|(n, _)| n != name);
        }
    }

    fn serve(mut self, mut io: Pipe) -> Result<Server> {
        loop {
            let mut size = [0; 4];
            if io.read_exact(&mut size).is_err() {
                return Ok(self);
            }
            let size = u32::from_le_bytes(size) as usize;
            let mut rest = vec![0; size - 4];
            io.read_exact(&mut rest)?;
            let tag = u16::from_le_bytes([rest[1], rest[2]]);
            let out = self.handle(rest[0], tag, &rest[3..]);
            io.write_all(&out)?;
        }
    }
}

/// Starts a server with `setup` run on its tree first, and gives the client's end of the pipe. The
/// thread ends when the client's end is dropped, and gives back the server for its tree to be looked at.
pub fn spawn(cfg: Config, setup: impl FnOnce(&mut Server)) -> (Pipe, JoinHandle<Result<Server>>) {
    let (client, server_end) = pipe();
    let mut server = Server::new(cfg);
    setup(&mut server);
    (client, std::thread::spawn(move || server.serve(server_end)))
}

impl Server {
    /// The contents of the file at `path`, if there is one.
    pub fn file(&self, path: &str) -> Option<Vec<u8>> {
        let mut at = ROOT;
        for part in path.split('/') {
            at = self.lookup(at, part)?;
        }
        match &self.nodes.get(&at)?.kind {
            Kind::File(data) => Some(data.clone()),
            Kind::Dir(_) => None,
        }
    }
}
