//! The client against the in-memory server, and against replies made to be wrong.

use crate::testserver::{Config, Pipe, Script, reply, spawn};
use crate::wire::{Decoder, Encoder, HEADER, NOTAG, msg};
use crate::*;
use std::io;

/// A pattern that tells where a byte came from.
fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

/// A client attached to a server whose share holds `docs/notes/a.txt` (3500 bytes), `docs/b.txt`
/// and `top.txt`. It gives the root fid.
fn mounted() -> (Client<Pipe>, u32, std::thread::JoinHandle<Result<testserver::Server>>) {
    let (pipe, handle) = spawn(Config::default(), |s| {
        s.put_file("docs/notes/a.txt", &pattern(3500));
        s.put_file("docs/b.txt", b"bee");
        s.put_file("top.txt", b"top");
    });
    let mut c = Client::new(pipe);
    c.version(DEFAULT_MSIZE).unwrap();
    let root = c.new_fid();
    c.attach(root, "share", "nobody", 65534).unwrap();
    (c, root, handle)
}

use crate::testserver;

fn walk_to(c: &mut Client<Pipe>, from: u32, path: &[&str]) -> u32 {
    let fid = c.new_fid();
    let qids = c.walk(from, fid, path).unwrap();
    assert_eq!(qids.len(), path.len());
    fid
}

/// A client that has agreed on `msize`, whose server is the replies given.
fn scripted(msize: u32, replies: &[Vec<u8>]) -> Client<Script> {
    let mut c = Client::new(Script::new(replies.concat()));
    c.msize = msize;
    c
}

fn expect_err<T: std::fmt::Debug>(r: Result<T>) -> Error {
    r.expect_err("the call should have failed")
}

// --- Version and attach ---

#[test]
fn version_takes_the_servers_smaller_msize() {
    let (pipe, _h) = spawn(Config::default(), |_| {});
    let mut c = Client::new(pipe);
    assert_eq!(c.version(65536).unwrap(), 4096);
    assert_eq!(c.msize(), 4096);
}

#[test]
fn version_sends_9p2000l_and_the_clients_msize_with_notag() {
    let mut body = Encoder::new(0, 0);
    body.u32(8192).str("9P2000.L").unwrap();
    let mut c = Client::new(Script::new(reply(msg::TVERSION + 1, NOTAG, &body.buf[7..])));
    c.version(8192).unwrap();
    let sent = c.into_inner().sent;
    assert_eq!(sent.len(), 4 + 1 + 2 + 4 + 2 + 8);
    assert_eq!(&sent[..4], &(sent.len() as u32).to_le_bytes());
    assert_eq!(sent[4], msg::TVERSION);
    assert_eq!(&sent[5..7], &NOTAG.to_le_bytes());
    assert_eq!(&sent[7..11], &8192u32.to_le_bytes());
    assert_eq!(&sent[11..13], &8u16.to_le_bytes());
    assert_eq!(&sent[13..], b"9P2000.L");
}

fn rversion(msize: u32, version: &str) -> Vec<u8> {
    let mut e = Encoder::new(0, 0);
    e.u32(msize).str(version).unwrap();
    reply(msg::TVERSION + 1, NOTAG, &e.buf[7..])
}

#[test]
fn version_refuses_another_version() {
    let mut c = Client::new(Script::new(rversion(4096, "9P2000.u")));
    assert!(matches!(expect_err(c.version(8192)), Error::BadVersion(v) if v == "9P2000.u"));
    let mut c = Client::new(Script::new(rversion(4096, "unknown")));
    assert!(matches!(expect_err(c.version(8192)), Error::BadVersion(_)));
}

#[test]
fn version_refuses_an_msize_over_the_clients_or_too_small() {
    let mut c = Client::new(Script::new(rversion(8193, "9P2000.L")));
    assert!(matches!(expect_err(c.version(8192)), Error::BadMsize(8193)));
    let mut c = Client::new(Script::new(rversion(MIN_MSIZE - 1, "9P2000.L")));
    assert!(matches!(expect_err(c.version(8192)), Error::BadMsize(_)));
    let mut c = Client::new(Script::new(Vec::new()));
    assert!(matches!(expect_err(c.version(MIN_MSIZE - 1)), Error::BadMsize(_)));
    assert!(c.into_inner().sent.is_empty());
}

#[test]
fn version_reply_over_the_clients_msize_is_refused() {
    let mut big = rversion(4096, "9P2000.L");
    big.extend(vec![0; 200]);
    let len = big.len() as u32;
    big[..4].copy_from_slice(&len.to_le_bytes());
    let mut c = Client::new(Script::new(big));
    assert!(matches!(expect_err(c.version(128)), Error::BadSize { .. }));
}

#[test]
fn attach_to_an_unknown_share_is_enoent() {
    let (pipe, _h) = spawn(Config::default(), |_| {});
    let mut c = Client::new(pipe);
    c.version(DEFAULT_MSIZE).unwrap();
    let fid = c.new_fid();
    let e = expect_err(c.attach(fid, "other", "nobody", 0));
    assert!(matches!(e, Error::Remote(2)));
    assert_eq!(e.errno(), Some(Errno::NoEntry));
}

#[test]
fn attach_sends_nofid_for_afid_and_the_names() {
    let mut qid = Encoder::new(0, 0);
    Qid { ty: Qid::DIR, version: 1, path: 2 }.put(&mut qid);
    let mut c = scripted(4096, &[reply(msg::TATTACH + 1, 1, &qid.buf[7..])]);
    let q = c.attach(5, "share", "me", 1000).unwrap();
    assert!(q.is_dir());
    let sent = c.into_inner().sent;
    let mut d = Decoder::new(&sent[7..]);
    assert_eq!(d.u32().unwrap(), 5);
    assert_eq!(d.u32().unwrap(), NOFID);
    assert_eq!(d.str().unwrap(), "me");
    assert_eq!(d.str().unwrap(), "share");
    assert_eq!(d.u32().unwrap(), 1000);
    d.done().unwrap();
}

// --- Walking and attributes ---

#[test]
fn walk_to_a_nested_file_and_getattr() {
    let (mut c, root, _h) = mounted();
    let fid = c.new_fid();
    let qids = c.walk(root, fid, &["docs", "notes", "a.txt"]).unwrap();
    assert_eq!(qids.len(), 3);
    assert!(qids[0].is_dir() && qids[1].is_dir() && !qids[2].is_dir());
    let a = c.getattr(fid, getattr::BASIC).unwrap();
    assert_eq!(a.size, 3500);
    assert_eq!(a.mode, mode::S_IFREG | 0o644);
    assert_eq!((a.uid, a.gid, a.nlink, a.blksize, a.blocks), (1000, 1001, 1, 4096, 7));
    assert_eq!(a.atime, Timespec { sec: 1_700_000_000, nsec: 1_700_000_000 % 7 });
    assert_eq!(a.mtime.sec, 1_700_000_001);
    assert_eq!(a.ctime.sec, 1_700_000_002);
    assert_eq!(a.qid, qids[2]);
    assert_eq!(a.valid, getattr::BASIC);
    let root_attr = c.getattr(root, getattr::BASIC).unwrap();
    assert_eq!(root_attr.mode & mode::S_IFMT, mode::S_IFDIR);
    assert_eq!(root_attr.nlink, 2);
}

#[test]
fn walk_with_no_names_copies_the_fid() {
    let (mut c, root, _h) = mounted();
    let copy = c.new_fid();
    assert!(c.walk(root, copy, &[]).unwrap().is_empty());
    assert!(c.getattr(copy, getattr::MODE).unwrap().qid.is_dir());
}

#[test]
fn walk_that_stops_part_way_gives_fewer_qids() {
    let (mut c, root, _h) = mounted();
    let fid = c.new_fid();
    let qids = c.walk(root, fid, &["docs", "missing", "x"]).unwrap();
    assert_eq!(qids.len(), 1);
    // The new fid was not made.
    assert!(matches!(expect_err(c.getattr(fid, getattr::MODE)), Error::Remote(_)));
}

#[test]
fn walk_to_a_missing_first_name_is_enoent_and_too_many_names_is_refused() {
    let (mut c, root, _h) = mounted();
    let fid = c.new_fid();
    assert_eq!(expect_err(c.walk(root, fid, &["nope"])).errno(), Some(Errno::NoEntry));
    let names = vec!["a"; MAX_WALK + 1];
    assert!(matches!(expect_err(c.walk(root, fid, &names)), Error::TooLarge));
    // Nothing was sent for the refused one: the next call still works.
    assert_eq!(c.walk(root, fid, &["top.txt"]).unwrap().len(), 1);
}

#[test]
fn walk_reply_with_more_qids_than_names_is_malformed() {
    let mut e = Encoder::new(0, 0);
    e.u16(2);
    Qid::default().put(&mut e);
    Qid::default().put(&mut e);
    let mut c = scripted(4096, &[reply(msg::TWALK + 1, 1, &e.buf[7..])]);
    assert!(matches!(expect_err(c.walk(0, 1, &["a"])), Error::Malformed(_)));
}

#[test]
fn walk_reply_whose_count_does_not_match_its_qids_is_malformed() {
    let mut e = Encoder::new(0, 0);
    e.u16(2);
    Qid::default().put(&mut e);
    let mut c = scripted(4096, &[reply(msg::TWALK + 1, 1, &e.buf[7..])]);
    assert!(matches!(expect_err(c.walk(0, 1, &["a", "b"])), Error::Malformed(_)));
}

// --- Reading and writing ---

#[test]
fn read_a_file_larger_than_iounit() {
    let (mut c, root, _h) = mounted();
    let fid = walk_to(&mut c, root, &["docs", "notes", "a.txt"]);
    let (qid, iounit) = c.lopen(fid, open::O_RDONLY).unwrap();
    assert!(!qid.is_dir());
    assert_eq!(iounit, 1000);
    assert_eq!(c.read(fid, 0, 3500).unwrap(), pattern(3500));
    // From the middle, across a piece boundary.
    assert_eq!(c.read(fid, 900, 300).unwrap(), pattern(3500)[900..1200]);
    // Asking for more than there is gives what there is, and at the end nothing.
    assert_eq!(c.read(fid, 3000, 10_000).unwrap(), pattern(3500)[3000..]);
    assert!(c.read(fid, 3500, 100).unwrap().is_empty());
    assert!(c.read(fid, 9000, 100).unwrap().is_empty());
    assert!(c.read(fid, 0, 0).unwrap().is_empty());
}

#[test]
fn read_asks_for_no_more_than_the_iounit() {
    // An iounit of 100 and a reply of 100 bytes to each of three requests.
    let open = {
        let mut e = Encoder::new(0, 0);
        Qid::default().put(&mut e);
        e.u32(100);
        reply(msg::TLOPEN + 1, 1, &e.buf[7..])
    };
    let chunk = |tag| {
        let mut e = Encoder::new(0, 0);
        e.u32(100).bytes(&[7; 100]);
        reply(msg::TREAD + 1, tag, &e.buf[7..])
    };
    let mut c = scripted(4096, &[open, chunk(2), chunk(3), chunk(4)]);
    c.lopen(9, open::O_RDONLY).unwrap();
    assert_eq!(c.read(9, 50, 300).unwrap().len(), 300);
    let sent = c.into_inner().sent;
    let reads: Vec<(u64, u32)> = sent[HEADER + 8..]
        .chunks(7 + 16)
        .map(|m| {
            assert_eq!(m[4], msg::TREAD);
            let mut d = Decoder::new(&m[7..]);
            d.u32().unwrap();
            (d.u64().unwrap(), d.u32().unwrap())
        })
        .collect();
    assert_eq!(reads, [(50, 100), (150, 100), (250, 100)]);
}

#[test]
fn read_without_an_iounit_is_cut_to_msize() {
    let c = scripted(256, &[]);
    assert_eq!(c.chunk(1), 256 - IOHDRSZ as usize);
}

#[test]
fn a_short_read_ends_the_read() {
    let mut e = Encoder::new(0, 0);
    e.u32(10).bytes(&[1; 10]);
    let mut c = scripted(4096, &[reply(msg::TREAD + 1, 1, &e.buf[7..])]);
    assert_eq!(c.read(1, 0, 100).unwrap(), vec![1; 10]);
}

#[test]
fn a_read_that_gives_more_than_asked_is_refused() {
    let mut e = Encoder::new(0, 0);
    e.u32(20).bytes(&[1; 20]);
    let mut c = scripted(4096, &[reply(msg::TREAD + 1, 1, &e.buf[7..])]);
    assert!(matches!(expect_err(c.read(1, 0, 10)), Error::Malformed(_)));
}

#[test]
fn a_read_whose_count_is_more_than_its_data_is_truncated() {
    let mut e = Encoder::new(0, 0);
    e.u32(10).bytes(&[1; 4]);
    let mut c = scripted(4096, &[reply(msg::TREAD + 1, 1, &e.buf[7..])]);
    assert!(matches!(expect_err(c.read(1, 0, 10)), Error::Truncated));
}

#[test]
fn write_and_read_back() {
    let (mut c, root, h) = mounted();
    let fid = walk_to(&mut c, root, &["docs", "b.txt"]);
    c.lopen(fid, open::O_RDWR).unwrap();
    let data = pattern(2600);
    assert_eq!(c.write(fid, 0, &data).unwrap(), 2600);
    assert_eq!(c.read(fid, 0, 5000).unwrap(), data);
    // Overwrite in the middle, across a piece boundary.
    assert_eq!(c.write(fid, 990, &[0xEE; 30]).unwrap(), 30);
    let back = c.read(fid, 985, 40).unwrap();
    assert_eq!(&back[..5], &data[985..990]);
    assert_eq!(&back[5..35], &[0xEE; 30]);
    assert_eq!(&back[35..], &data[1020..1025]);
    // An empty write sends nothing.
    assert_eq!(c.write(fid, 0, &[]).unwrap(), 0);
    c.clunk(fid).unwrap();
    drop(c);
    let server = h.join().unwrap().unwrap();
    let mut expect = data;
    expect[990..1020].fill(0xEE);
    assert_eq!(server.file("docs/b.txt").unwrap(), expect);
}

#[test]
fn write_is_split_by_iounit_and_a_short_write_stops() {
    let open = {
        let mut e = Encoder::new(0, 0);
        Qid::default().put(&mut e);
        e.u32(100);
        reply(msg::TLOPEN + 1, 1, &e.buf[7..])
    };
    let wrote = |tag, n: u32| reply(msg::TWRITE + 1, tag, &n.to_le_bytes());
    let mut c = scripted(4096, &[open, wrote(2, 100), wrote(3, 40)]);
    c.lopen(2, open::O_WRONLY).unwrap();
    // 250 bytes in three pieces; the second is taken in part, so the third is never sent.
    assert_eq!(c.write(2, 10, &[3; 250]).unwrap(), 140);
    let sent = c.into_inner().sent;
    let tail = &sent[7 + 8..];
    assert_eq!(tail.len(), 2 * (7 + 4 + 8 + 4) + 200);
}

#[test]
fn a_write_that_takes_more_than_it_was_given_is_refused() {
    let mut c = scripted(4096, &[reply(msg::TWRITE + 1, 1, &11u32.to_le_bytes())]);
    assert!(matches!(expect_err(c.write(1, 0, &[0; 10])), Error::Malformed(_)));
}

#[test]
fn a_write_that_takes_nothing_stops_instead_of_looping() {
    let mut c = scripted(4096, &[reply(msg::TWRITE + 1, 1, &0u32.to_le_bytes())]);
    assert_eq!(c.write(1, 0, &[0; 10]).unwrap(), 0);
}

#[test]
fn a_request_over_msize_is_refused_before_it_is_sent() {
    let mut c = scripted(MIN_MSIZE, &[]);
    let long = "x".repeat(300);
    assert!(matches!(expect_err(c.mkdir(1, &long, 0o755, 0)), Error::TooLarge));
    assert!(c.into_inner().sent.is_empty());
}

#[test]
fn a_string_over_65535_bytes_is_refused() {
    let mut c = scripted(DEFAULT_MSIZE, &[]);
    let long = "x".repeat(70_000);
    assert!(matches!(expect_err(c.mkdir(1, &long, 0o755, 0)), Error::Malformed(_)));
}

// --- Creating, listing, moving, removing ---

#[test]
fn create_write_and_find_it_in_the_directory() {
    let (mut c, root, h) = mounted();
    let dir = walk_to(&mut c, root, &["docs"]);
    let file = c.new_fid();
    c.walk(dir, file, &[]).unwrap();
    let (qid, iounit) = c.lcreate(file, "new.txt", open::O_RDWR | open::O_CREAT, 0o640, 1001).unwrap();
    assert!(!qid.is_dir());
    assert_eq!(iounit, 1000);
    assert_eq!(c.write(file, 0, b"fresh").unwrap(), 5);
    assert_eq!(c.read(file, 0, 100).unwrap(), b"fresh");
    // The fid now is the new file, and creating it again is EEXIST.
    assert_eq!(c.getattr(file, getattr::SIZE).unwrap().size, 5);
    let again = c.new_fid();
    c.walk(root, again, &["docs"]).unwrap();
    let e = expect_err(c.lcreate(again, "new.txt", open::O_RDWR | open::O_CREAT, 0o640, 0));
    assert_eq!(e.errno(), Some(Errno::Exists));
    drop(c);
    assert_eq!(h.join().unwrap().unwrap().file("docs/new.txt").unwrap(), b"fresh");
}

#[test]
fn mkdir_and_walk_into_it() {
    let (mut c, root, _h) = mounted();
    let q = c.mkdir(root, "fresh", 0o755, 0).unwrap();
    assert!(q.is_dir());
    let fid = walk_to(&mut c, root, &["fresh"]);
    assert!(c.getattr(fid, getattr::MODE).unwrap().mode & mode::S_IFMT == mode::S_IFDIR);
    assert_eq!(expect_err(c.mkdir(root, "fresh", 0o755, 0)).errno(), Some(Errno::Exists));
}

#[test]
fn readdir_across_several_calls() {
    let (mut c, root, _h) = mounted();
    let dir = walk_to(&mut c, root, &["docs"]);
    for n in ["c", "d", "e"] {
        c.mkdir(dir, n, 0o755, 0).unwrap();
    }
    let (q, _) = c.lopen(dir, open::O_RDONLY | open::O_DIRECTORY).unwrap();
    assert!(q.is_dir());
    let mut names = Vec::new();
    let mut offset = 0;
    let mut calls = 0;
    loop {
        let batch = c.readdir(dir, offset, 4096).unwrap();
        calls += 1;
        let Some(last) = batch.last() else { break };
        assert!(batch.len() <= 2);
        for e in &batch {
            let expect = if e.name == "notes" || ["c", "d", "e"].contains(&e.name.as_str()) {
                dt::DIR
            } else {
                dt::REG
            };
            assert_eq!(e.ty, expect);
            assert_eq!(e.qid.is_dir(), expect == dt::DIR);
        }
        offset = last.offset;
        names.extend(batch.into_iter().map(|e| e.name));
    }
    assert_eq!(names, ["notes", "b.txt", "c", "d", "e"]);
    assert_eq!(calls, 4);
}

#[test]
fn readdir_with_a_small_count_gives_fewer_entries() {
    let (mut c, root, _h) = mounted();
    let (qid, _) = c.lopen(root, open::O_RDONLY).unwrap();
    assert!(qid.is_dir());
    // "docs" is 13 + 8 + 1 + 2 + 4 = 28 bytes; 40 holds one.
    assert_eq!(c.readdir(root, 0, 40).unwrap().len(), 1);
    assert!(c.readdir(root, 0, 10).unwrap().is_empty());
}

#[test]
fn readdir_on_a_file_is_enotdir() {
    let (mut c, root, _h) = mounted();
    let f = walk_to(&mut c, root, &["top.txt"]);
    c.lopen(f, open::O_RDONLY).unwrap();
    assert_eq!(expect_err(c.readdir(f, 0, 4096)).errno(), Some(Errno::NotDir));
}

#[test]
fn a_readdir_entry_cut_short_is_refused() {
    let mut data = Encoder::new(0, 0);
    Qid::default().put(&mut data);
    data.u64(1).u8(dt::REG).u16(10).bytes(b"abc");
    let body = &data.buf[7..];
    let mut e = Encoder::new(0, 0);
    e.u32(body.len() as u32).bytes(body);
    let mut c = scripted(4096, &[reply(msg::TREADDIR + 1, 1, &e.buf[7..])]);
    assert!(matches!(expect_err(c.readdir(1, 0, 4096)), Error::Truncated));
}

#[test]
fn a_readdir_that_gives_more_than_asked_is_refused() {
    let mut e = Encoder::new(0, 0);
    e.u32(100).bytes(&[0; 100]);
    let mut c = scripted(4096, &[reply(msg::TREADDIR + 1, 1, &e.buf[7..])]);
    assert!(matches!(expect_err(c.readdir(1, 0, 50)), Error::Malformed(_)));
}

#[test]
fn rename_moves_between_directories() {
    let (mut c, root, h) = mounted();
    let docs = walk_to(&mut c, root, &["docs"]);
    c.renameat(docs, "b.txt", root, "moved.txt").unwrap();
    let fid = c.new_fid();
    assert_eq!(expect_err(c.walk(docs, fid, &["b.txt"])).errno(), Some(Errno::NoEntry));
    assert_eq!(c.walk(root, fid, &["moved.txt"]).unwrap().len(), 1);
    assert_eq!(expect_err(c.renameat(root, "missing", root, "x")).errno(), Some(Errno::NoEntry));
    assert_eq!(expect_err(c.renameat(root, "moved.txt", root, "top.txt")).errno(), Some(Errno::Exists));
    drop(c);
    assert_eq!(h.join().unwrap().unwrap().file("moved.txt").unwrap(), b"bee");
}

#[test]
fn unlink_files_and_directories() {
    let (mut c, root, _h) = mounted();
    c.unlinkat(root, "top.txt", 0).unwrap();
    let fid = c.new_fid();
    assert_eq!(expect_err(c.walk(root, fid, &["top.txt"])).errno(), Some(Errno::NoEntry));
    assert_eq!(expect_err(c.unlinkat(root, "top.txt", 0)).errno(), Some(Errno::NoEntry));
    // A directory with something in it won't go, and one without a flag is a directory.
    assert_eq!(expect_err(c.unlinkat(root, "docs", AT_REMOVEDIR)).errno(), Some(Errno::NotEmpty));
    assert_eq!(expect_err(c.unlinkat(root, "docs", 0)).errno(), Some(Errno::IsDir));
    c.mkdir(root, "empty", 0o755, 0).unwrap();
    c.unlinkat(root, "empty", AT_REMOVEDIR).unwrap();
    assert_eq!(expect_err(c.unlinkat(root, "docs", 0)).errno(), Some(Errno::IsDir));
}

#[test]
fn setattr_truncates_and_changes_the_mode() {
    let (mut c, root, h) = mounted();
    let fid = walk_to(&mut c, root, &["docs", "notes", "a.txt"]);
    c.setattr(fid, &SetAttr { valid: setattr::SIZE, size: 100, ..Default::default() }).unwrap();
    assert_eq!(c.getattr(fid, getattr::SIZE).unwrap().size, 100);
    c.setattr(
        fid,
        &SetAttr { valid: setattr::SIZE | setattr::MODE, size: 200, mode: 0o600, ..Default::default() },
    )
    .unwrap();
    let a = c.getattr(fid, getattr::BASIC).unwrap();
    assert_eq!((a.size, a.mode & 0o7777), (200, 0o600));
    drop(c);
    let data = h.join().unwrap().unwrap().file("docs/notes/a.txt").unwrap();
    assert_eq!(data.len(), 200);
    assert_eq!(&data[..100], &pattern(100)[..]);
    assert!(data[100..].iter().all(|b| *b == 0));
}

#[test]
fn setattr_sends_every_field_in_order() {
    let mut c = scripted(4096, &[reply(msg::TSETATTR + 1, 1, &[])]);
    let attr = SetAttr {
        valid: 0x1ff,
        mode: 0o640,
        uid: 7,
        gid: 8,
        size: 9,
        atime: Timespec { sec: 10, nsec: 11 },
        mtime: Timespec { sec: 12, nsec: 13 },
    };
    c.setattr(3, &attr).unwrap();
    let sent = c.into_inner().sent;
    let mut d = Decoder::new(&sent[7..]);
    assert_eq!(d.u32().unwrap(), 3);
    assert_eq!(
        [d.u32().unwrap(), d.u32().unwrap(), d.u32().unwrap(), d.u32().unwrap()],
        [0x1ff, 0o640, 7, 8]
    );
    assert_eq!(
        [d.u64().unwrap(), d.u64().unwrap(), d.u64().unwrap(), d.u64().unwrap(), d.u64().unwrap()],
        [9, 10, 11, 12, 13]
    );
    d.done().unwrap();
}

#[test]
fn statfs_fsync_and_clunk() {
    let (mut c, root, _h) = mounted();
    let s = c.statfs(root).unwrap();
    assert_eq!(
        s,
        StatFs {
            fs_type: 0x0102_1997,
            bsize: 4096,
            blocks: 1000,
            bfree: 400,
            bavail: 300,
            files: 50,
            ffree: 20,
            fsid: 7,
            namelen: 255
        }
    );
    let fid = walk_to(&mut c, root, &["top.txt"]);
    c.lopen(fid, open::O_RDWR).unwrap();
    c.fsync(fid).unwrap();
    c.clunk(fid).unwrap();
    // The server forgot the fid.
    assert!(matches!(expect_err(c.getattr(fid, getattr::MODE)), Error::Remote(_)));
    assert!(matches!(expect_err(c.clunk(fid)), Error::Remote(_)));
}

#[test]
fn clunk_frees_the_fid_for_reuse() {
    let (mut c, root, _h) = mounted();
    let a = c.new_fid();
    let b = c.new_fid();
    assert_ne!(a, b);
    c.walk(root, a, &["top.txt"]).unwrap();
    c.clunk(a).unwrap();
    assert_eq!(c.new_fid(), a);
    assert_ne!(c.new_fid(), a);
}

#[test]
fn fids_are_not_freed_by_a_transport_failure() {
    let mut c = scripted(4096, &[]);
    let fid = c.new_fid();
    assert!(matches!(expect_err(c.clunk(fid)), Error::Io(_)));
    assert_ne!(c.new_fid(), fid);
}

#[test]
fn fid_pool_hands_out_distinct_numbers_and_ignores_strays() {
    let mut p = FidPool::new();
    let (a, b, c) = (p.alloc(), p.alloc(), p.alloc());
    assert_eq!([a, b, c], [0, 1, 2]);
    p.release(b);
    p.release(b);
    p.release(99);
    assert_eq!(p.alloc(), b);
    assert_eq!(p.alloc(), 3);
}

// --- Errors ---

#[test]
fn errnos_map_to_their_kinds() {
    use Errno::*;
    let cases = [
        (2, NoEntry),
        (5, Io),
        (13, AccessDenied),
        (17, Exists),
        (20, NotDir),
        (21, IsDir),
        (22, Invalid),
        (28, NoSpace),
        (30, ReadOnly),
        (39, NotEmpty),
        (95, Other(95)),
        (0, Other(0)),
    ];
    for (n, kind) in cases {
        assert_eq!(Errno::from_raw(n), kind);
        assert_eq!(Error::Remote(n).errno(), Some(kind));
    }
    assert_eq!(Error::Truncated.errno(), None);
}

#[test]
fn rlerror_carries_the_errno_and_the_client_goes_on() {
    let mut c =
        scripted(4096, &[reply(msg::RLERROR, 1, &13u32.to_le_bytes()), reply(msg::TCLUNK + 1, 2, &[])]);
    let e = expect_err(c.getattr(1, 0));
    assert!(matches!(e, Error::Remote(13)));
    assert_eq!(e.errno(), Some(Errno::AccessDenied));
    assert!(e.to_string().contains("13"));
    c.clunk(1).unwrap();
}

#[test]
fn an_rlerror_with_a_bad_body_is_refused() {
    let mut c = scripted(4096, &[reply(msg::RLERROR, 1, &[13, 0])]);
    assert!(matches!(expect_err(c.getattr(1, 0)), Error::Truncated));
    let mut c = scripted(4096, &[reply(msg::RLERROR, 1, &[13, 0, 0, 0, 0])]);
    assert!(matches!(expect_err(c.getattr(1, 0)), Error::Malformed(_)));
}

// --- Framing ---

#[test]
fn a_reply_over_msize_is_refused_without_reading_it() {
    let mut bytes = (MIN_MSIZE + 1).to_le_bytes().to_vec();
    bytes.extend(vec![0; 10]);
    let mut c = scripted(MIN_MSIZE, &[bytes]);
    assert!(
        matches!(expect_err(c.clunk(1)), Error::BadSize { size, max } if size == MIN_MSIZE + 1 && max == MIN_MSIZE)
    );
}

#[test]
fn a_huge_size_does_not_allocate() {
    let mut c = scripted(4096, &[u32::MAX.to_le_bytes().to_vec()]);
    assert!(matches!(expect_err(c.clunk(1)), Error::BadSize { .. }));
}

#[test]
fn a_reply_smaller_than_a_header_is_refused() {
    for size in [0u32, 1, 4, 6] {
        let mut c = scripted(4096, &[size.to_le_bytes().to_vec(), vec![0; 10]]);
        assert!(matches!(expect_err(c.clunk(1)), Error::BadSize { .. }), "size {size}");
    }
}

#[test]
fn a_reply_exactly_msize_is_taken_and_one_more_byte_is_not() {
    let body = [0u8; 153];
    let msg = reply(msg::TGETATTR + 1, 1, &body);
    assert_eq!(msg.len(), 160);
    assert!(scripted(160, std::slice::from_ref(&msg)).getattr(1, 0).is_ok());
    assert!(matches!(
        expect_err(scripted(159, &[msg]).getattr(1, 0)),
        Error::BadSize { size: 160, max: 159 }
    ));
}

#[test]
fn a_message_cut_off_in_the_header_or_body_is_an_io_error() {
    for cut in [0, 2, 4, 6, 8] {
        let full = reply(msg::TCLUNK + 1, 1, &[1, 2, 3, 4]);
        let mut c = scripted(4096, &[full[..cut].to_vec()]);
        let e = expect_err(c.clunk(1));
        assert!(matches!(&e, Error::Io(io) if io.kind() == io::ErrorKind::UnexpectedEof), "cut {cut}: {e}");
    }
}

#[test]
fn a_reply_with_the_wrong_tag_is_refused() {
    let mut c = scripted(4096, &[reply(msg::TCLUNK + 1, 9, &[])]);
    assert!(matches!(expect_err(c.clunk(1)), Error::UnexpectedTag { expected: 1, got: 9 }));
    // An Rlerror for another tag is no better.
    let mut c = scripted(4096, &[reply(msg::RLERROR, 9, &2u32.to_le_bytes())]);
    assert!(matches!(expect_err(c.clunk(1)), Error::UnexpectedTag { .. }));
}

#[test]
fn a_reply_of_the_wrong_type_is_refused() {
    let mut c = scripted(4096, &[reply(msg::TREAD + 1, 1, &[])]);
    assert!(
        matches!(expect_err(c.clunk(1)), Error::UnexpectedType { expected, got } if expected == msg::TCLUNK + 1 && got == msg::TREAD + 1)
    );
    // Rerror, 9P2000's own, isn't 9P2000.L's.
    let mut c = scripted(4096, &[reply(107, 1, &[0, 0])]);
    assert!(matches!(expect_err(c.clunk(1)), Error::UnexpectedType { got: 107, .. }));
    // A request type from the server.
    let mut c = scripted(4096, &[reply(msg::TCLUNK, 1, &[])]);
    assert!(matches!(expect_err(c.clunk(1)), Error::UnexpectedType { .. }));
}

#[test]
fn a_reply_with_extra_bytes_or_too_few_is_refused() {
    let mut c = scripted(4096, &[reply(msg::TCLUNK + 1, 1, &[0])]);
    assert!(matches!(expect_err(c.clunk(1)), Error::Malformed(_)));
    let mut c = scripted(4096, &[reply(msg::TMKDIR + 1, 1, &[0; 12])]);
    assert!(matches!(expect_err(c.mkdir(1, "d", 0o755, 0)), Error::Truncated));
    let mut c = scripted(4096, &[reply(msg::TMKDIR + 1, 1, &[0; 14])]);
    assert!(matches!(expect_err(c.mkdir(1, "d", 0o755, 0)), Error::Malformed(_)));
}

#[test]
fn a_getattr_reply_cut_short_is_refused() {
    let mut c = scripted(4096, &[reply(msg::TGETATTR + 1, 1, &[0; 100])]);
    assert!(matches!(expect_err(c.getattr(1, 0)), Error::Truncated));
}

#[test]
fn the_server_end_going_away_is_an_io_error() {
    let (pipe, handle) = spawn(Config::default(), |_| {});
    let mut c = Client::new(pipe);
    c.version(DEFAULT_MSIZE).unwrap();
    drop(c);
    handle.join().unwrap().unwrap();
    let (mut pipe, _server_end) = testserver::pipe();
    drop(_server_end);
    let mut buf = [0; 4];
    assert_eq!(io::Read::read(&mut pipe, &mut buf).unwrap(), 0);
    let mut c = Client::new(pipe);
    assert!(matches!(expect_err(c.version(DEFAULT_MSIZE)), Error::Io(_)));
}

#[test]
fn tags_count_up_skip_notag_and_wrap() {
    let mut c = scripted(4096, &[]);
    assert_eq!([c.next_tag(), c.next_tag()], [1, 2]);
    c.tag = 0xFFFD;
    assert_eq!(c.next_tag(), 0xFFFE);
    assert_eq!(c.next_tag(), 0);
    assert_eq!(c.next_tag(), 1);
}

#[test]
fn each_request_has_the_next_tag() {
    let mut c = scripted(4096, &[reply(msg::TCLUNK + 1, 1, &[]), reply(msg::TCLUNK + 1, 2, &[])]);
    c.clunk(5).unwrap();
    c.clunk(6).unwrap();
    let sent = c.into_inner().sent;
    assert_eq!(sent.len(), 22);
    assert_eq!(&sent[5..7], &1u16.to_le_bytes());
    assert_eq!(&sent[16..18], &2u16.to_le_bytes());
}

// --- Wire ---

#[test]
fn numbers_and_strings_round_trip_little_endian() {
    let mut e = Encoder::new(msg::TCLUNK, 0x0102);
    e.u8(1).u16(0x0203).u32(0x0405_0607).u64(0x0809_0a0b_0c0d_0e0f);
    e.str("héllo").unwrap();
    let buf = e.finish().unwrap();
    assert_eq!(&buf[..7], &[buf.len() as u8, 0, 0, 0, msg::TCLUNK, 2, 1]);
    assert_eq!(&buf[7..10], &[1, 3, 2]);
    assert_eq!(&buf[10..14], &[7, 6, 5, 4]);
    let mut d = Decoder::new(&buf[7..]);
    assert_eq!(
        (d.u8().unwrap(), d.u16().unwrap(), d.u32().unwrap(), d.u64().unwrap()),
        (1, 0x0203, 0x0405_0607, 0x0809_0a0b_0c0d_0e0f)
    );
    assert_eq!(d.str().unwrap(), "héllo");
    d.done().unwrap();
    assert!(matches!(d.u8(), Err(Error::Truncated)));
}

#[test]
fn a_string_longer_than_its_message_or_not_utf8_is_refused() {
    assert!(matches!(Decoder::new(&[5, 0, b'a']).str(), Err(Error::Truncated)));
    assert!(matches!(Decoder::new(&[2, 0, 0xff, 0xfe]).str(), Err(Error::Malformed(_))));
    assert!(matches!(Decoder::new(&[1]).str(), Err(Error::Truncated)));
    assert_eq!(Decoder::new(&[0, 0]).str().unwrap(), "");
}

#[test]
fn a_qid_is_thirteen_bytes() {
    let mut e = Encoder::new(0, 0);
    Qid { ty: Qid::DIR, version: 0x0102_0304, path: 0x1112_1314_1516_1718 }.put(&mut e);
    assert_eq!(e.buf.len() - 7, Qid::SIZE);
    assert_eq!(&e.buf[7..], &[0x80, 4, 3, 2, 1, 0x18, 0x17, 0x16, 0x15, 0x14, 0x13, 0x12, 0x11]);
    let q = Decoder::new(&e.buf[7..]).qid().unwrap();
    assert_eq!(q.path, 0x1112_1314_1516_1718);
    assert!(q.is_dir());
    assert!(matches!(Decoder::new(&e.buf[7..19]).qid(), Err(Error::Truncated)));
}

#[test]
fn constants_are_the_numbers_9p2000l_uses() {
    assert_eq!(
        [
            msg::TSTATFS,
            msg::TLOPEN,
            msg::TLCREATE,
            msg::TGETATTR,
            msg::TSETATTR,
            msg::TREADDIR,
            msg::TFSYNC,
            msg::TMKDIR,
            msg::TRENAMEAT,
            msg::TUNLINKAT
        ],
        [8, 12, 14, 24, 26, 40, 50, 72, 74, 76]
    );
    assert_eq!(
        [msg::TVERSION, msg::TATTACH, msg::TWALK, msg::TREAD, msg::TWRITE, msg::TCLUNK],
        [100, 104, 110, 116, 118, 120]
    );
    assert_eq!(
        [open::O_CREAT, open::O_EXCL, open::O_TRUNC, open::O_APPEND, open::O_DIRECTORY],
        [64, 128, 512, 1024, 65536]
    );
    assert_eq!(
        (getattr::BASIC, getattr::ALL, setattr::SIZE, setattr::MTIME_SET, AT_REMOVEDIR),
        (0x7ff, 0x3fff, 8, 0x100, 0x200)
    );
}

#[test]
fn create_flags_reach_the_server_as_given() {
    let mut e = Encoder::new(0, 0);
    Qid::default().put(&mut e);
    e.u32(0);
    let mut c = scripted(4096, &[reply(msg::TLCREATE + 1, 1, &e.buf[7..])]);
    c.lcreate(4, "f", open::O_WRONLY | open::O_CREAT | open::O_EXCL, 0o600, 55).unwrap();
    let sent = c.into_inner().sent;
    let mut d = Decoder::new(&sent[7..]);
    assert_eq!(d.u32().unwrap(), 4);
    assert_eq!(d.str().unwrap(), "f");
    assert_eq!(d.u32().unwrap(), 1 | 0o100 | 0o200);
    assert_eq!((d.u32().unwrap(), d.u32().unwrap()), (0o600, 55));
    d.done().unwrap();
}
