//! The file system: WinFsp's callbacks, each answered with 9P calls to the host.
//!
//! One 9P client sits behind one mutex, and a callback holds it for all of its calls, so WinFsp's
//! threads take turns. The next thing to improve is to let them overlap: a client that tags
//! requests and matches replies to them, or a few connections.
//!
//! Each open file or directory has a fid of its own, opened for it, and a path. Anything that
//! needs another file, such as the children of an open directory, a file's parent, or the target
//! of a rename, walks to it from the root by path: an opened fid can't be walked from, and a path
//! is all WinFsp gives.
//!
//! Every callback is run by `guard`, which turns a panic into an I/O error. An error from the 9P
//! client that isn't the server saying no means the stream is dead or out of step, and marks the
//! share dead for the main thread to unmount; it never stops the dispatcher from here, which would
//! wait for the very thread that asks.
#![allow(clippy::too_many_arguments)]

use crate::ffi::{
    CLEANUP_DELETE, DirInfoBuf, FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FileInfo, FileSystem,
    Interface, MAX_NAME_UNITS, NtStatus, Pvoid, Pwstr, VolumeInfo,
};
use crate::hv::HvStream;
use crate::logic::{self, FileInfoData, status};
use crate::winfsp::Api;
use guesthouse_p9::{
    self as p9, AT_REMOVEDIR, Attr, Client, Errno, MAX_WALK, NOFID, SetAttr, getattr, setattr,
};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::slice;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock, PoisonError};

/// Each status here is the one `windows-sys` has.
mod checked {
    use super::status;
    use windows_sys::Win32::Foundation as f;
    const _: () = assert!(status::SUCCESS == f::STATUS_SUCCESS);
    const _: () = assert!(status::BUFFER_OVERFLOW == f::STATUS_BUFFER_OVERFLOW);
    const _: () = assert!(status::INVALID_PARAMETER == f::STATUS_INVALID_PARAMETER);
    const _: () = assert!(status::END_OF_FILE == f::STATUS_END_OF_FILE);
    const _: () = assert!(status::ACCESS_DENIED == f::STATUS_ACCESS_DENIED);
    const _: () = assert!(status::OBJECT_NAME_INVALID == f::STATUS_OBJECT_NAME_INVALID);
    const _: () = assert!(status::OBJECT_NAME_NOT_FOUND == f::STATUS_OBJECT_NAME_NOT_FOUND);
    const _: () = assert!(status::OBJECT_NAME_COLLISION == f::STATUS_OBJECT_NAME_COLLISION);
    const _: () = assert!(status::OBJECT_PATH_NOT_FOUND == f::STATUS_OBJECT_PATH_NOT_FOUND);
    const _: () = assert!(status::DISK_FULL == f::STATUS_DISK_FULL);
    const _: () = assert!(status::FILE_IS_A_DIRECTORY == f::STATUS_FILE_IS_A_DIRECTORY);
    const _: () = assert!(status::MEDIA_WRITE_PROTECTED == f::STATUS_MEDIA_WRITE_PROTECTED);
    const _: () = assert!(status::DIRECTORY_NOT_EMPTY == f::STATUS_DIRECTORY_NOT_EMPTY);
    const _: () = assert!(status::NOT_A_DIRECTORY == f::STATUS_NOT_A_DIRECTORY);
    const _: () = assert!(status::IO_DEVICE_ERROR == f::STATUS_IO_DEVICE_ERROR);
}

/// The group a new file is made in. The host's server answers for owners itself.
const GID: u32 = 0;
/// What a new file's and directory's mode asks for.
const FILE_MODE: u32 = 0o644;
const DIR_MODE: u32 = 0o755;
/// A share's size if the server won't say, so that Windows doesn't see a full disk.
const UNKNOWN_SIZE: u64 = 1 << 40;

type R<T> = Result<T, i32>;

/// Everything the callbacks share. There is one for the process, as there is one file system.
pub struct Shared {
    client: Mutex<Client<HvStream>>,
    /// The attach fid, never opened, so that every walk can start from it.
    root: u32,
    read_only: bool,
    label: Vec<u16>,
    security: Vec<u8>,
    api: Api,
    dead: Mutex<Option<String>>,
    woken: Condvar,
}

static SHARED: OnceLock<Shared> = OnceLock::new();

impl Shared {
    /// Installs the one set of shared state. It gives it back if there already is one.
    pub fn install(
        client: Client<HvStream>,
        root: u32,
        read_only: bool,
        label: &str,
        security: Vec<u8>,
        api: Api,
    ) -> &'static Shared {
        let label: Vec<u16> = label.encode_utf16().take(32).collect();
        let shared = Shared {
            client: Mutex::new(client),
            root,
            read_only,
            label,
            security,
            api,
            dead: Mutex::new(None),
            woken: Condvar::new(),
        };
        SHARED.get_or_init(|| shared)
    }

    pub fn api(&self) -> &Api {
        &self.api
    }

    /// Marks the connection as lost, for `wait_dead` to say, and keeps the first reason.
    fn kill(&self, why: String) {
        let mut dead = self.dead.lock().unwrap_or_else(PoisonError::into_inner);
        dead.get_or_insert(why);
        self.woken.notify_all();
    }

    fn is_dead(&self) -> bool {
        self.dead.lock().unwrap_or_else(PoisonError::into_inner).is_some()
    }

    /// Waits until the connection is lost, and says why.
    pub fn wait_dead(&self) -> String {
        let mut dead = self.dead.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(why) = dead.as_ref() {
                return why.clone();
            }
            dead = self.woken.wait(dead).unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// An error from the 9P client as a status. One that isn't the server's no kills the share.
    fn status(&self, error: &p9::Error) -> i32 {
        if logic::is_fatal(error) {
            self.kill(error.to_string());
        }
        logic::error_status(error)
    }
}

/// What an open handle holds. WinFsp keeps the pointer from `Create` or `Open` until `Close`.
struct FileCtx {
    /// The open fid, or `NOFID` once a delete has let go of it.
    fid: AtomicU32,
    is_dir: bool,
    /// The names from the root, which a rename changes.
    path: Mutex<Vec<String>>,
}

impl FileCtx {
    fn new(fid: u32, is_dir: bool, path: Vec<String>) -> Box<FileCtx> {
        Box::new(FileCtx { fid: AtomicU32::new(fid), is_dir, path: Mutex::new(path) })
    }

    fn fid(&self) -> R<u32> {
        match self.fid.load(Ordering::Acquire) {
            NOFID => Err(status::IO_DEVICE_ERROR),
            fid => Ok(fid),
        }
    }

    fn path(&self) -> Vec<String> {
        self.path.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// The client, locked for one callback, and what a callback does with it.
struct Ops<'a> {
    sh: &'a Shared,
    c: MutexGuard<'a, Client<HvStream>>,
}

fn missing(index: usize, total: usize) -> i32 {
    // The last name missing is a name not found; any before it is a path.
    if index + 1 >= total { status::OBJECT_NAME_NOT_FOUND } else { status::OBJECT_PATH_NOT_FOUND }
}

impl<'a> Ops<'a> {
    fn new(sh: &'a Shared) -> R<Ops<'a>> {
        if sh.is_dead() {
            return Err(status::IO_DEVICE_ERROR);
        }
        Ok(Ops { sh, c: sh.client.lock().unwrap_or_else(PoisonError::into_inner) })
    }

    fn st(&self, error: p9::Error) -> i32 {
        self.sh.status(&error)
    }

    fn writable(&self) -> R<()> {
        if self.sh.read_only { Err(status::MEDIA_WRITE_PROTECTED) } else { Ok(()) }
    }

    /// Lets go of a fid. Nothing can be done if that fails, beyond the share noticing a dead stream.
    fn clunk(&mut self, fid: u32) {
        if let Err(error) = self.c.clunk(fid) {
            self.sh.status(&error);
        }
    }

    /// Lets go of `fid` if `result` is an error, and gives the result back.
    fn undo<T>(&mut self, fid: u32, result: R<T>) -> R<T> {
        if result.is_err() {
            self.clunk(fid);
        }
        result
    }

    /// One walk of at most `MAX_WALK` names from `from` to a new fid. A walk can fail by an error or
    /// by stopping short, and both are not found, but neither leaves a fid.
    fn walk_chunk(&mut self, from: u32, chunk: &[String], done: usize, total: usize) -> R<u32> {
        let new = self.c.new_fid();
        let names: Vec<&str> = chunk.iter().map(String::as_str).collect();
        match self.c.walk(from, new, &names) {
            Ok(qids) if qids.len() == chunk.len() => Ok(new),
            Ok(qids) => {
                self.c.release_fid(new);
                Err(missing(done + qids.len(), total))
            }
            Err(p9::Error::Remote(n)) if Errno::from_raw(n) == Errno::NoEntry => {
                self.c.release_fid(new);
                Err(missing(done, total))
            }
            Err(error) => {
                self.c.release_fid(new);
                Err(self.st(error))
            }
        }
    }

    /// A new fid for the file at `names` from the root, as many walks as it takes. The root itself
    /// is a copy of the attach fid. The fid is the caller's to clunk.
    fn walk_path(&mut self, names: &[String]) -> R<u32> {
        let root = self.sh.root;
        if names.is_empty() {
            return self.walk_chunk(root, &[], 0, 0);
        }
        let mut current = root;
        let mut done = 0;
        for chunk in names.chunks(MAX_WALK) {
            let next = self.walk_chunk(current, chunk, done, names.len());
            if current != root {
                self.clunk(current);
            }
            current = next?;
            done += chunk.len();
        }
        Ok(current)
    }

    fn attr(&mut self, fid: u32) -> R<Attr> {
        let result = self.c.getattr(fid, getattr::BASIC);
        result.map_err(|e| self.st(e))
    }

    fn info(&mut self, fid: u32) -> R<FileInfoData> {
        Ok(logic::file_info(&self.attr(fid)?))
    }

    /// The attributes of the file at `names`.
    fn stat_path(&mut self, names: &[String]) -> R<Attr> {
        let fid = self.walk_path(names)?;
        let attr = self.attr(fid);
        self.clunk(fid);
        attr
    }

    /// Every name in the open directory `fid`, `.` and `..` too if the server sends them.
    fn dir_names(&mut self, fid: u32) -> R<Vec<String>> {
        let mut names = Vec::new();
        let mut offset = 0;
        let count = self.c.msize();
        loop {
            let entries = self.c.readdir(fid, offset, count).map_err(|e| self.st(e))?;
            let Some(last) = entries.last().map(|e| e.offset) else { break };
            if last == offset {
                break;
            }
            offset = last;
            names.extend(entries.into_iter().map(|e| e.name));
        }
        Ok(names)
    }

    /// Opens what is at `names`, for the access that was granted, and gives the handle and what to
    /// tell WinFsp of it.
    fn open_path(&mut self, names: &[String], granted: u32, options: u32) -> R<(Box<FileCtx>, FileInfoData)> {
        let fid = self.walk_path(names)?;
        let attr = self.attr(fid);
        let attr = self.undo(fid, attr)?;
        let dir = logic::is_dir(&attr);
        let wrong = if dir && options & FILE_NON_DIRECTORY_FILE != 0 {
            Err(status::FILE_IS_A_DIRECTORY)
        } else if !dir && options & FILE_DIRECTORY_FILE != 0 {
            Err(status::NOT_A_DIRECTORY)
        } else {
            Ok(())
        };
        self.undo(fid, wrong)?;
        let flags = logic::open_flags(granted, dir, self.sh.read_only);
        let opened = self.c.lopen(fid, flags).map(drop).map_err(|e| self.st(e));
        self.undo(fid, opened)?;
        Ok((FileCtx::new(fid, dir, names.to_vec()), logic::file_info(&attr)))
    }

    /// Makes a file or directory at `names`, and opens it.
    fn create(&mut self, names: &[String], granted: u32, options: u32) -> R<(Box<FileCtx>, FileInfoData)> {
        self.writable()?;
        let (name, parent) = names.split_last().ok_or(status::ACCESS_DENIED)?;
        let pfid = self.walk_path(parent)?;
        if options & FILE_DIRECTORY_FILE != 0 {
            let made = self.c.mkdir(pfid, name, DIR_MODE, GID);
            self.clunk(pfid);
            made.map_err(|e| self.st(e))?;
            return self.open_path(names, granted, options);
        }
        // `lcreate` makes the file and turns `pfid` into it.
        let flags = logic::open_flags(granted, false, false);
        let made = self.c.lcreate(pfid, name, flags, FILE_MODE, GID).map(drop).map_err(|e| self.st(e));
        self.undo(pfid, made)?;
        let attr = self.attr(pfid);
        let attr = self.undo(pfid, attr)?;
        Ok((FileCtx::new(pfid, false, names.to_vec()), logic::file_info(&attr)))
    }

    fn truncate(&mut self, fid: u32, size: u64) -> R<()> {
        let attr = SetAttr { valid: setattr::SIZE, size, ..SetAttr::default() };
        self.c.setattr(fid, &attr).map_err(|e| self.st(e))
    }

    /// Removes the file or directory at `names` from its parent.
    fn unlink(&mut self, names: &[String], is_dir: bool) -> R<()> {
        let (name, parent) = names.split_last().ok_or(status::ACCESS_DENIED)?;
        let pfid = self.walk_path(parent)?;
        let flags = if is_dir { AT_REMOVEDIR } else { 0 };
        let result = self.c.unlinkat(pfid, name, flags).map_err(|e| self.st(e));
        self.clunk(pfid);
        result
    }

    fn rename(&mut self, old: &[String], new: &[String], replace: bool) -> R<()> {
        let (old_name, old_parent) = old.split_last().ok_or(status::ACCESS_DENIED)?;
        let (new_name, new_parent) = new.split_last().ok_or(status::ACCESS_DENIED)?;
        if old == new {
            return Ok(());
        }
        // `renameat` replaces a file without asking, so whether it may is decided here.
        match self.stat_path(new) {
            Ok(target) => {
                if !replace {
                    return Err(status::OBJECT_NAME_COLLISION);
                }
                if logic::is_dir(&target) {
                    return Err(status::ACCESS_DENIED);
                }
            }
            Err(s) if s == status::OBJECT_NAME_NOT_FOUND => {}
            Err(s) => return Err(s),
        }
        let from = self.walk_path(old_parent)?;
        let to = self.walk_path(new_parent);
        let to = self.undo(from, to)?;
        let result = self.c.renameat(from, old_name, to, new_name).map_err(|e| self.st(e));
        self.clunk(from);
        self.clunk(to);
        result
    }
}

/// Runs a callback's body, with the shared state, and turns what it gives into a status. A panic
/// can leave the stream out of step, so it kills the share.
fn guard(body: impl FnOnce(&'static Shared) -> R<()>) -> NtStatus {
    let Some(sh) = SHARED.get() else { return status::IO_DEVICE_ERROR };
    match catch_unwind(AssertUnwindSafe(|| body(sh))) {
        Ok(Ok(())) => status::SUCCESS,
        Ok(Err(s)) => s,
        Err(_) => {
            sh.kill("a file system callback panicked".to_owned());
            status::IO_DEVICE_ERROR
        }
    }
}

/// The handle behind a context pointer WinFsp gives back.
///
/// # Safety
/// `p` is a pointer `Create` or `Open` gave WinFsp, and `Close` hasn't yet taken back.
unsafe fn ctx<'a>(p: Pvoid) -> &'a FileCtx {
    // SAFETY: as said.
    unsafe { &*p.cast::<FileCtx>() }
}

/// The names of a path WinFsp gives as a wide string.
///
/// # Safety
/// `p` is null or a terminated wide string.
unsafe fn path_names(p: Pwstr) -> R<Vec<String>> {
    if p.is_null() {
        return Err(status::INVALID_PARAMETER);
    }
    let mut len = 0;
    // SAFETY: the string is terminated, and this stops at the terminator.
    while unsafe { *p.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: `len` units are before the terminator.
    let units = unsafe { slice::from_raw_parts(p, len) };
    logic::split_path(&logic::utf16_name(units)?)
}

fn to_ffi(d: &FileInfoData) -> FileInfo {
    FileInfo {
        file_attributes: d.attributes,
        reparse_tag: 0,
        allocation_size: d.allocation_size,
        file_size: d.file_size,
        creation_time: d.creation_time,
        last_access_time: d.access_time,
        last_write_time: d.write_time,
        change_time: d.change_time,
        index_number: d.index,
        hard_links: 0,
        ea_size: 0,
    }
}

/// Fills in the `FileInfo` WinFsp gave, if it gave one.
///
/// # Safety
/// `out` is null or valid for a write.
unsafe fn put_info(out: *mut FileInfo, d: &FileInfoData) {
    if !out.is_null() {
        // SAFETY: as said.
        unsafe { out.write(to_ffi(d)) };
    }
}

/// Copies the security descriptor out as `GetSecurityByName` and `GetSecurity` want: it says the
/// size it needs if the buffer is too small, and any pointer may be null.
///
/// # Safety
/// `descriptor` is null or `*size` bytes, and `size` is null or valid.
unsafe fn put_security(sh: &Shared, descriptor: Pvoid, size: *mut usize) -> R<()> {
    if size.is_null() {
        return Ok(());
    }
    let need = sh.security.len();
    // SAFETY: as said.
    unsafe {
        if *size < need {
            *size = need;
            return Err(status::BUFFER_OVERFLOW);
        }
        *size = need;
        if !descriptor.is_null() {
            std::ptr::copy_nonoverlapping(sh.security.as_ptr(), descriptor.cast::<u8>(), need);
        }
    }
    Ok(())
}

unsafe extern "C" fn get_volume_info(_fs: *mut FileSystem, out: *mut VolumeInfo) -> NtStatus {
    guard(|sh| {
        let mut ops = Ops::new(sh)?;
        let (total, free) = match ops.c.statfs(sh.root) {
            Ok(s) => logic::volume_sizes(s.bsize, s.blocks, s.bavail),
            Err(p9::Error::Remote(_)) => (UNKNOWN_SIZE, UNKNOWN_SIZE),
            Err(e) => return Err(ops.st(e)),
        };
        // SAFETY: WinFsp gives a valid structure to fill in.
        let out = unsafe { &mut *out };
        out.total_size = total;
        out.free_size = free;
        out.volume_label = [0; 32];
        out.volume_label[..sh.label.len()].copy_from_slice(&sh.label);
        out.volume_label_length = (sh.label.len() * 2) as u16;
        Ok(())
    })
}

unsafe extern "C" fn get_security_by_name(
    _fs: *mut FileSystem,
    name: Pwstr,
    attributes: *mut u32,
    descriptor: Pvoid,
    size: *mut usize,
) -> NtStatus {
    guard(|sh| {
        // SAFETY: WinFsp gives a terminated name.
        let names = unsafe { path_names(name) }?;
        let attr = Ops::new(sh)?.stat_path(&names)?;
        if !attributes.is_null() {
            // SAFETY: not null, and WinFsp's to write.
            unsafe { *attributes = logic::file_info(&attr).attributes };
        }
        // SAFETY: WinFsp's buffer and its size.
        unsafe { put_security(sh, descriptor, size) }
    })
}

unsafe extern "C" fn create(
    _fs: *mut FileSystem,
    name: Pwstr,
    options: u32,
    granted: u32,
    _attributes: u32,
    _descriptor: Pvoid,
    _allocation: u64,
    out_ctx: *mut Pvoid,
    out_info: *mut FileInfo,
) -> NtStatus {
    guard(|sh| {
        // SAFETY: WinFsp gives a terminated name.
        let names = unsafe { path_names(name) }?;
        let (handle, info) = Ops::new(sh)?.create(&names, granted, options)?;
        // SAFETY: WinFsp's pointers; the handle is taken back in `close`.
        unsafe {
            *out_ctx = Box::into_raw(handle).cast();
            put_info(out_info, &info);
        }
        Ok(())
    })
}

unsafe extern "C" fn open(
    _fs: *mut FileSystem,
    name: Pwstr,
    options: u32,
    granted: u32,
    out_ctx: *mut Pvoid,
    out_info: *mut FileInfo,
) -> NtStatus {
    guard(|sh| {
        // SAFETY: WinFsp gives a terminated name.
        let names = unsafe { path_names(name) }?;
        let (handle, info) = Ops::new(sh)?.open_path(&names, granted, options)?;
        // SAFETY: WinFsp's pointers; the handle is taken back in `close`.
        unsafe {
            *out_ctx = Box::into_raw(handle).cast();
            put_info(out_info, &info);
        }
        Ok(())
    })
}

unsafe extern "C" fn overwrite(
    _fs: *mut FileSystem,
    handle: Pvoid,
    _attributes: u32,
    _replace_attributes: u8,
    _allocation: u64,
    out_info: *mut FileInfo,
) -> NtStatus {
    guard(|sh| {
        // SAFETY: WinFsp's handle.
        let fid = unsafe { ctx(handle) }.fid()?;
        let mut ops = Ops::new(sh)?;
        ops.writable()?;
        ops.truncate(fid, 0)?;
        let info = ops.info(fid)?;
        // SAFETY: WinFsp's pointer.
        unsafe { put_info(out_info, &info) };
        Ok(())
    })
}

unsafe extern "C" fn cleanup(_fs: *mut FileSystem, handle: Pvoid, _name: Pwstr, flags: u32) {
    if flags & CLEANUP_DELETE == 0 {
        return;
    }
    // A delete is the one thing a cleanup does. It can't fail to WinFsp, so a failure is dropped.
    guard(|sh| {
        // SAFETY: WinFsp's handle.
        let handle = unsafe { ctx(handle) };
        let mut ops = Ops::new(sh)?;
        ops.writable()?;
        // The host may not remove a file it still has open, so this handle's fid goes first.
        let fid = handle.fid.swap(NOFID, Ordering::AcqRel);
        if fid != NOFID {
            ops.clunk(fid);
        }
        ops.unlink(&handle.path(), handle.is_dir)
    });
}

unsafe extern "C" fn close(_fs: *mut FileSystem, handle: Pvoid) {
    // SAFETY: the pointer was made by `Box::into_raw` in `create` or `open`, and WinFsp calls this
    // once, last.
    let handle = unsafe { Box::from_raw(handle.cast::<FileCtx>()) };
    guard(|sh| {
        let fid = handle.fid.swap(NOFID, Ordering::AcqRel);
        if fid != NOFID {
            Ops::new(sh)?.clunk(fid);
        }
        Ok(())
    });
}

unsafe extern "C" fn read(
    _fs: *mut FileSystem,
    handle: Pvoid,
    buffer: Pvoid,
    offset: u64,
    length: u32,
    transferred: *mut u32,
) -> NtStatus {
    guard(|sh| {
        // SAFETY: WinFsp's pointers.
        unsafe { *transferred = 0 };
        // SAFETY: WinFsp's handle.
        let fid = unsafe { ctx(handle) }.fid()?;
        let mut ops = Ops::new(sh)?;
        let data = ops.c.read(fid, offset, length).map_err(|e| ops.st(e))?;
        if data.is_empty() && length > 0 {
            return Err(status::END_OF_FILE);
        }
        // SAFETY: `buffer` holds `length` bytes and `data` no more.
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), buffer.cast::<u8>(), data.len());
            *transferred = data.len() as u32;
        }
        Ok(())
    })
}

unsafe extern "C" fn write(
    _fs: *mut FileSystem,
    handle: Pvoid,
    buffer: Pvoid,
    offset: u64,
    length: u32,
    write_to_end: u8,
    constrained: u8,
    transferred: *mut u32,
    out_info: *mut FileInfo,
) -> NtStatus {
    guard(|sh| {
        // SAFETY: WinFsp's pointers.
        unsafe { *transferred = 0 };
        // SAFETY: WinFsp's handle.
        let fid = unsafe { ctx(handle) }.fid()?;
        let mut ops = Ops::new(sh)?;
        ops.writable()?;
        let (to_end, constrained) = (write_to_end != 0, constrained != 0);
        let size = if to_end || constrained { ops.attr(fid)?.size } else { 0 };
        if let Some((at, len)) = logic::plan_write(to_end, constrained, offset, length as usize, size) {
            // SAFETY: `buffer` holds `length` bytes, and the plan is no more of them.
            let data = unsafe { slice::from_raw_parts(buffer.cast::<u8>(), len) };
            let wrote = ops.c.write(fid, at, data).map_err(|e| ops.st(e))?;
            // SAFETY: WinFsp's pointer.
            unsafe { *transferred = wrote as u32 };
        }
        let info = ops.info(fid)?;
        // SAFETY: WinFsp's pointer.
        unsafe { put_info(out_info, &info) };
        Ok(())
    })
}

unsafe extern "C" fn flush(_fs: *mut FileSystem, handle: Pvoid, out_info: *mut FileInfo) -> NtStatus {
    guard(|sh| {
        // A flush of the whole volume has no handle, and there's nothing buffered here.
        if handle.is_null() {
            return Ok(());
        }
        // SAFETY: WinFsp's handle.
        let handle = unsafe { ctx(handle) };
        let fid = handle.fid()?;
        let mut ops = Ops::new(sh)?;
        if !handle.is_dir && !sh.read_only {
            ops.c.fsync(fid).map_err(|e| ops.st(e))?;
        }
        let info = ops.info(fid)?;
        // SAFETY: WinFsp's pointer.
        unsafe { put_info(out_info, &info) };
        Ok(())
    })
}

unsafe extern "C" fn get_file_info(_fs: *mut FileSystem, handle: Pvoid, out_info: *mut FileInfo) -> NtStatus {
    guard(|sh| {
        // SAFETY: WinFsp's handle.
        let fid = unsafe { ctx(handle) }.fid()?;
        let info = Ops::new(sh)?.info(fid)?;
        // SAFETY: WinFsp's pointer.
        unsafe { put_info(out_info, &info) };
        Ok(())
    })
}

unsafe extern "C" fn set_basic_info(
    _fs: *mut FileSystem,
    handle: Pvoid,
    _attributes: u32,
    _creation: u64,
    access: u64,
    write: u64,
    _change: u64,
    out_info: *mut FileInfo,
) -> NtStatus {
    guard(|sh| {
        // SAFETY: WinFsp's handle.
        let fid = unsafe { ctx(handle) }.fid()?;
        let mut ops = Ops::new(sh)?;
        // Zero, and all ones, are WinFsp's and NTFS's "leave it as it is".
        let set = |t: u64| t != 0 && t != u64::MAX;
        let mut attr = SetAttr::default();
        if set(access) {
            let (sec, nsec) = logic::filetime_to_unix(access);
            attr.valid |= setattr::ATIME | setattr::ATIME_SET;
            attr.atime = p9::Timespec { sec, nsec };
        }
        if set(write) {
            let (sec, nsec) = logic::filetime_to_unix(write);
            attr.valid |= setattr::MTIME | setattr::MTIME_SET;
            attr.mtime = p9::Timespec { sec, nsec };
        }
        if attr.valid != 0 {
            ops.writable()?;
            ops.c.setattr(fid, &attr).map_err(|e| ops.st(e))?;
        }
        let info = ops.info(fid)?;
        // SAFETY: WinFsp's pointer.
        unsafe { put_info(out_info, &info) };
        Ok(())
    })
}

unsafe extern "C" fn set_file_size(
    _fs: *mut FileSystem,
    handle: Pvoid,
    new_size: u64,
    set_allocation: u8,
    out_info: *mut FileInfo,
) -> NtStatus {
    guard(|sh| {
        // SAFETY: WinFsp's handle.
        let fid = unsafe { ctx(handle) }.fid()?;
        let mut ops = Ops::new(sh)?;
        ops.writable()?;
        // An allocation smaller than the file cuts it; a larger one leaves it as it is.
        if set_allocation == 0 || new_size < ops.attr(fid)?.size {
            ops.truncate(fid, new_size)?;
        }
        let info = ops.info(fid)?;
        // SAFETY: WinFsp's pointer.
        unsafe { put_info(out_info, &info) };
        Ok(())
    })
}

unsafe extern "C" fn can_delete(_fs: *mut FileSystem, handle: Pvoid, _name: Pwstr) -> NtStatus {
    guard(|sh| {
        // SAFETY: WinFsp's handle.
        let handle = unsafe { ctx(handle) };
        let mut ops = Ops::new(sh)?;
        ops.writable()?;
        if handle.is_dir {
            let names = ops.dir_names(handle.fid()?)?;
            if names.iter().any(|n| n != "." && n != "..") {
                return Err(status::DIRECTORY_NOT_EMPTY);
            }
        }
        Ok(())
    })
}

unsafe extern "C" fn rename(
    _fs: *mut FileSystem,
    handle: Pvoid,
    _name: Pwstr,
    new_name: Pwstr,
    replace: u8,
) -> NtStatus {
    guard(|sh| {
        // SAFETY: WinFsp's handle and terminated name.
        let (handle, new) = unsafe { (ctx(handle), path_names(new_name)?) };
        let mut ops = Ops::new(sh)?;
        ops.writable()?;
        ops.rename(&handle.path(), &new, replace != 0)?;
        *handle.path.lock().unwrap_or_else(PoisonError::into_inner) = new;
        Ok(())
    })
}

unsafe extern "C" fn get_security(
    _fs: *mut FileSystem,
    _handle: Pvoid,
    descriptor: Pvoid,
    size: *mut usize,
) -> NtStatus {
    // SAFETY: WinFsp's buffer and its size.
    guard(|sh| unsafe { put_security(sh, descriptor, size) })
}

unsafe extern "C" fn read_directory(
    _fs: *mut FileSystem,
    handle: Pvoid,
    _pattern: Pwstr,
    marker: Pwstr,
    buffer: Pvoid,
    length: u32,
    transferred: *mut u32,
) -> NtStatus {
    guard(|sh| {
        // SAFETY: WinFsp's pointer.
        unsafe { *transferred = 0 };
        // SAFETY: WinFsp's handle.
        let handle = unsafe { ctx(handle) };
        if !handle.is_dir {
            return Err(status::NOT_A_DIRECTORY);
        }
        let fid = handle.fid()?;
        let marker = if marker.is_null() {
            None
        } else {
            let mut len = 0;
            // SAFETY: a terminated name, read up to its terminator.
            while unsafe { *marker.add(len) } != 0 {
                len += 1;
            }
            // SAFETY: `len` units are before the terminator.
            Some(logic::utf16_name(unsafe { slice::from_raw_parts(marker, len) })?)
        };
        let path = handle.path();
        let mut ops = Ops::new(sh)?;
        let names = ops.dir_names(fid)?;
        let ordered = logic::order_directory(names, path.is_empty(), marker.as_deref());
        let mut entry = Box::new(DirInfoBuf::zeroed());
        for name in ordered {
            let units: Vec<u16> = name.encode_utf16().collect();
            if units.len() > MAX_NAME_UNITS {
                continue;
            }
            let info = if name == "." || name == ".." {
                ops.info(fid)
            } else {
                let mut child = path.clone();
                child.push(name);
                ops.stat_path(&child).map(|attr| logic::file_info(&attr))
            };
            let info = match info {
                Ok(info) => info,
                // An entry that went away, or can't be read, is left out of the listing.
                Err(_) if !sh.is_dead() => continue,
                Err(s) => return Err(s),
            };
            entry.info.size = (size_of::<crate::ffi::DirInfo>() + units.len() * 2) as u16;
            entry.info.file_info = to_ffi(&info);
            entry.name[..units.len()].copy_from_slice(&units);
            // SAFETY: the entry is a header with its name after it, and the buffer is WinFsp's.
            let added = unsafe { sh.api.add_dir_info(&mut entry.info, buffer, length, transferred) };
            if !added {
                // The buffer is full: WinFsp asks again from the last entry that went in.
                return Ok(());
            }
        }
        // SAFETY: a null entry ends the listing.
        unsafe { sh.api.add_dir_info(std::ptr::null_mut(), buffer, length, transferred) };
        Ok(())
    })
}

/// The callbacks, as WinFsp's table has them.
pub fn interface() -> &'static Interface {
    static TABLE: OnceLock<Interface> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = Interface::empty();
        table.get_volume_info = Some(get_volume_info);
        table.get_security_by_name = Some(get_security_by_name);
        table.create = Some(create);
        table.open = Some(open);
        table.overwrite = Some(overwrite);
        table.cleanup = Some(cleanup);
        table.close = Some(close);
        table.read = Some(read);
        table.write = Some(write);
        table.flush = Some(flush);
        table.get_file_info = Some(get_file_info);
        table.set_basic_info = Some(set_basic_info);
        table.set_file_size = Some(set_file_size);
        table.can_delete = Some(can_delete);
        table.rename = Some(rename);
        table.get_security = Some(get_security);
        table.read_directory = Some(read_directory);
        table
    })
}
