//! The pipe the app asks on. Each request has a pipe instance of its own, and the next instance is
//! made before a request is served, so the pipe's name is always the service's: no other program
//! can take it between two requests and pose as the service.
//!
//! Who may ask is checked twice. The pipe's own security lets in the system, administrators and
//! interactive users, and lets none of them but the service make an instance of it. Then each
//! request is checked against who is asking: the system, an elevated administrator, or the user
//! signed in at the console. The console's user changes as people sign in and out, which a pipe's
//! fixed security can't follow, so that's checked for each request.
//!
//! Each connection is served on a thread of its own, and only so many are served at once: so many
//! for each user, by the account of the process that connected, and so many in all. One past either
//! is closed at once, and the pipe goes on being served. So does it when no thread can be made.

use super::log;
use super::security::{Descriptor, SYSTEM, is_administrator, last_error, own_user, token_user, wide};
use crate::requests::{Asker, Connection, Held, Service, Work};
use guesthouse_control::protocol::{NOT_A_REQUEST, request_from};
use guesthouse_control::{Answer, REQUEST_LIMIT, read_line, write_line};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader};
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::ptr::null_mut;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_PIPE_CONNECTED, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::{RevertToSelf, SECURITY_ATTRIBUTES, TOKEN_QUERY};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ADD_FILE, FILE_ADD_SUBDIRECTORY, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_LIST_DIRECTORY, FILE_NAME_NORMALIZED, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TRAVERSE,
    FlushFileBuffers, GetDriveTypeW, GetFinalPathNameByHandleW, PIPE_ACCESS_DUPLEX, READ_CONTROL,
    VOLUME_NAME_DOS, WRITE_DAC,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, ImpersonateNamedPipeClient,
    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    PeekNamedPipe,
};
use windows_sys::Win32::System::RemoteDesktop::{WTSGetActiveConsoleSessionId, WTSQueryUserToken};
use windows_sys::Win32::System::Threading::{
    GetCurrentThread, OpenProcess, OpenProcessToken, OpenThreadToken, PROCESS_QUERY_LIMITED_INFORMATION,
};

/// The system, administrators and the pipe's owner, who is whoever made it, may do anything with
/// the pipe, making more instances of it too. Interactive users may read it and write to it
/// (`FILE_GENERIC_READ | FILE_WRITE_DATA`), and not append to it, which for a pipe is making an
/// instance of it. The protection stops a parent folder's rules being added.
const PIPE_SECURITY: &str = "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;OW)(A;;0x12008b;;;IU)";
/// What `GetDriveTypeW` says of a drive that is a disk the PC keeps.
const DRIVE_FIXED: u32 = 3;
/// Room in the pipe each way, enough for any request and most answers.
const BUFFER: u32 = REQUEST_LIMIT as u32;
/// How long to wait before trying again when an instance couldn't be made.
const AGAIN: Duration = Duration::from_secs(1);
/// How many connections may be open at once, and how long each has to send its request. The app has
/// a few open at a time (its requests, one after another, a watch, a machine being made, the remote
/// view's), far fewer than this for a user, and sends each request as soon as it connects.
pub const LIMITS: Limits = Limits { per_user: 32, in_all: 256, first_line: Duration::from_secs(10) };
/// How often a connection is looked at while its request is awaited.
const LINE_POLL: Duration = Duration::from_millis(20);
/// Whom a connection is counted against when the account of the process that made it can't be read.
const UNKNOWN_USER: &str = "an unknown user";

/// How many connections may be open at once, for one user and in all, and how long one may take to
/// send its whole request line before it's closed unanswered.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub per_user: usize,
    pub in_all: usize,
    pub first_line: Duration,
}

/// A connection's work, handed to a new thread.
type ConnectionWork = Box<dyn FnOnce() + Send>;

/// Starts `work` on a thread of its own. The work is dropped, and with it its connection, if no
/// thread can be made.
fn on_new_thread(work: ConnectionWork) -> io::Result<()> {
    thread::Builder::new().name("request".into()).spawn(work).map(drop)
}

/// Makes the pipe's first instance, which fails if any other program has its name, and then serves
/// it on a thread of its own for as long as the process runs.
pub fn listen<W: Work>(name: &'static str, service: Arc<Service<W>>) -> Result<(), String> {
    let security = Descriptor::new(PIPE_SECURITY)?;
    let first = instance(name, &security, true).map_err(|error| {
        format!("{error}. Another program may have the pipe's name, or the service is running already.")
    })?;
    let slots = Slots::new(LIMITS);
    thread::spawn(move || serve(name, service, security, first, &slots, on_new_thread));
    Ok(())
}

fn instance(name: &str, security: &Descriptor, first: bool) -> Result<OwnedHandle, String> {
    let text = wide(name);
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: security.as_ptr(),
        bInheritHandle: 0,
    };
    let open_mode = PIPE_ACCESS_DUPLEX | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
    let pipe_mode = PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS;
    // SAFETY: the name ends in a zero, and it and the security outlive the call.
    let handle = unsafe {
        CreateNamedPipeW(
            text.as_ptr(),
            open_mode,
            pipe_mode,
            PIPE_UNLIMITED_INSTANCES,
            BUFFER,
            BUFFER,
            0,
            &attributes,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(last_error(&format!("Making the pipe {name}")));
    }
    // SAFETY: a handle Windows just gave us, owned by nothing else.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

fn serve<W: Work>(
    name: &str,
    service: Arc<Service<W>>,
    security: Descriptor,
    first: OwnedHandle,
    slots: &Arc<Slots>,
    spawn: impl Fn(ConnectionWork) -> io::Result<()>,
) {
    let mut waiting = first;
    loop {
        // SAFETY: an instance of ours, waited on with no overlapped I/O.
        let connected = unsafe { ConnectNamedPipe(waiting.as_raw_handle(), null_mut()) } != 0
            // SAFETY: plain call, right after the one whose error it reads.
            || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
        // The next instance is made while this one is still held, so the name is never free.
        let next = loop {
            match instance(name, &security, false) {
                Ok(next) => break next,
                Err(error) => {
                    log(&error);
                    thread::sleep(AGAIN);
                }
            }
        };
        let served = std::mem::replace(&mut waiting, next);
        if !connected {
            continue;
        }
        let user = client_user(&served).unwrap_or_else(|| UNKNOWN_USER.to_owned());
        // One past the limit is closed here, unanswered, as `served` goes.
        let Some(slot) = slots.take(&user) else { continue };
        let service = Arc::clone(&service);
        let first_line = slots.limits.first_line;
        let work = Box::new(move || {
            let _slot = slot;
            let pipe = File::from(served);
            // A connection that hasn't sent its request in time is closed unanswered.
            if line_within(&pipe, first_line) {
                answer(&service, pipe);
            }
        });
        if let Err(error) = spawn(work) {
            log(&format!("A request was closed unanswered, as no thread could be made for it: {error}"));
        }
    }
}

/// The connections open now, by whom.
pub struct Slots {
    limits: Limits,
    open: Mutex<Open>,
}

#[derive(Default)]
struct Open {
    by_user: HashMap<String, usize>,
    in_all: usize,
    /// The users told of in the log as having reached their limit, until one of theirs closes.
    told: HashSet<String>,
}

/// One connection's place, given back as it goes.
pub struct Slot {
    slots: Arc<Slots>,
    user: String,
}

impl Slots {
    pub fn new(limits: Limits) -> Arc<Self> {
        Arc::new(Slots { limits, open: Mutex::new(Open::default()) })
    }

    /// A place for one more of `user`'s connections, if there's room for it.
    fn take(self: &Arc<Self>, user: &str) -> Option<Slot> {
        let mut open = self.lock();
        let theirs = open.by_user.get(user).copied().unwrap_or(0);
        if theirs >= self.limits.per_user || open.in_all >= self.limits.in_all {
            if open.told.insert(user.to_owned()) {
                log(&format!(
                    "refused: {user} has {theirs} connections open, of {} in all, which is as many as are served",
                    open.in_all
                ));
            }
            return None;
        }
        *open.by_user.entry(user.to_owned()).or_default() += 1;
        open.in_all += 1;
        Some(Slot { slots: Arc::clone(self), user: user.to_owned() })
    }

    fn lock(&self) -> MutexGuard<'_, Open> {
        self.open.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut open = self.slots.lock();
        open.in_all -= 1;
        open.told.remove(&self.user);
        if let Some(theirs) = open.by_user.get_mut(&self.user) {
            *theirs -= 1;
            if *theirs == 0 {
                open.by_user.remove(&self.user);
            }
        }
    }
}

/// The account of the process that connected to `pipe`, if it can be read.
fn client_user(pipe: &OwnedHandle) -> Option<String> {
    let mut process = 0u32;
    // SAFETY: an instance of ours with a client; the ID comes back in a place for it.
    if unsafe { GetNamedPipeClientProcessId(pipe.as_raw_handle(), &mut process) } == 0 {
        return None;
    }
    // SAFETY: plain call; the handle is closed below.
    let opened = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process) };
    if opened.is_null() {
        return None;
    }
    let mut token: HANDLE = null_mut();
    // SAFETY: the process's handle is open, and the token comes back in a place for it.
    let got = unsafe { OpenProcessToken(opened, TOKEN_QUERY, &mut token) } != 0;
    // SAFETY: ours, and not used again.
    unsafe { CloseHandle(opened) };
    if !got {
        return None;
    }
    let user = token_user(token);
    // SAFETY: ours, and not used again.
    unsafe { CloseHandle(token) };
    user
}

/// Whether a whole request line, or as much as a request may be, is waiting on `pipe` within
/// `within`, or the other end has gone, which reading then finds. The whole line fits in the pipe's
/// room, so it's all there before it's read.
fn line_within(pipe: &File, within: Duration) -> bool {
    let until = std::time::Instant::now() + within;
    let mut seen = vec![0u8; BUFFER as usize];
    loop {
        let mut copied = 0u32;
        // SAFETY: an instance of ours; the buffer is as long as said, and only copied into.
        let peeked = unsafe {
            PeekNamedPipe(
                pipe.as_raw_handle(),
                seen.as_mut_ptr().cast(),
                BUFFER,
                &mut copied,
                null_mut(),
                null_mut(),
            )
        } != 0;
        let copied = copied as usize;
        if !peeked || copied == seen.len() || seen[..copied].contains(&b'\n') {
            return true;
        }
        if std::time::Instant::now() >= until {
            return false;
        }
        thread::sleep(LINE_POLL);
    }
}

/// Reads one request and answers it, if whoever asked may ask.
fn answer<W: Work>(service: &Arc<Service<W>>, pipe: File) {
    let line = read_line::<Value>(&mut BufReader::new(&pipe));
    let refuse = |reason: String| {
        let _ = write_line(&mut &pipe, &Answer::Refused { reason });
    };
    match line.map(|line| line.map(request_from)) {
        Ok(Some(Ok(request))) => match allowed(&pipe) {
            Ok(asker) => service.answer(request, &mut Pipe { file: &pipe, asker }),
            Err(reason) => {
                log(&format!("refused: {reason}"));
                refuse(reason);
            }
        },
        // An app of another version, or a line that isn't a request: told why, in words any app shows.
        Ok(Some(Err(reason))) => refuse(reason),
        Ok(None) => {}
        Err(_) => refuse(NOT_A_REQUEST.into()),
    }
    // The answer is waited on until the app has read it, and then the instance is closed, which
    // the app reads as the end. Disconnecting it instead would throw away what the app hadn't read,
    // and the app would read an error, not the end.
    // SAFETY: the pipe's handle is ours and open.
    unsafe { FlushFileBuffers(pipe.as_raw_handle()) };
}

/// Whether the one asking on `pipe` may: the system, an elevated administrator, or the user signed
/// in at the console. With nobody signed in, as when the service runs at a prompt, the service's
/// own user stands in for the console's. The system counts as an administrator.
fn allowed(pipe: &File) -> Result<Asker, String> {
    let (user, administrator) = as_client(pipe, || {
        let mut token: HANDLE = null_mut();
        // SAFETY: the thread's token while it impersonates, opened as the service, and closed here.
        if unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut token) } == 0 {
            return (None, false);
        }
        let answer = (token_user(token), is_administrator(token));
        // SAFETY: ours, and not used after this.
        unsafe { CloseHandle(token) };
        answer
    })?;
    let user = user.ok_or_else(|| last_error("Reading who is asking"))?;
    let administrator = administrator || user == SYSTEM;
    if administrator || Some(&user) == console_user().or_else(own_user).as_ref() {
        Ok(Asker { user, administrator })
    } else {
        Err(format!("{user} asked, and only the user signed in at the console may use Guesthouse."))
    }
}

/// The user signed in at the console, if anyone is. Only the system may ask Windows this.
pub(super) fn console_user() -> Option<String> {
    let mut token: HANDLE = null_mut();
    // SAFETY: plain calls; the token comes back in a place for it, and is closed here.
    if unsafe { WTSQueryUserToken(WTSGetActiveConsoleSessionId(), &mut token) } == 0 {
        return None;
    }
    let user = token_user(token);
    // SAFETY: ours, and not used after this.
    unsafe { CloseHandle(token) };
    user
}

/// Runs `work` as the one asking on `pipe`, and is the service again after it.
fn as_client<T>(pipe: &File, work: impl FnOnce() -> T) -> Result<T, String> {
    // SAFETY: an instance of ours, from which a request has been read, as impersonating needs.
    if unsafe { ImpersonateNamedPipeClient(pipe.as_raw_handle()) } == 0 {
        return Err(last_error("Taking on who is asking"));
    }
    let done = work();
    // SAFETY: plain call. A thread that can't be the service again mustn't go on as someone else.
    if unsafe { RevertToSelf() } == 0 {
        log(&last_error("Ending the impersonation of an app"));
        std::process::abort();
    }
    Ok(done)
}

struct Pipe<'a> {
    file: &'a File,
    /// Who asked, as checked before the request is answered.
    asker: Asker,
}

impl Connection for Pipe<'_> {
    fn asker(&self) -> Asker {
        self.asker.clone()
    }

    fn send(&mut self, answer: &Answer) -> io::Result<()> {
        write_line(&mut self.file, answer)
    }

    fn gone(&mut self) -> bool {
        let mut waiting = 0u32;
        // SAFETY: an instance of ours; only how much is waiting is asked for, with no buffer.
        let peeked = unsafe {
            PeekNamedPipe(self.file.as_raw_handle(), null_mut(), 0, null_mut(), &mut waiting, null_mut())
        };
        peeked == 0
    }

    fn may_read(&mut self, path: &Path) -> Result<(), String> {
        match as_client(self.file, || File::open(path))? {
            Ok(_) => Ok(()),
            Err(error) => Err(format!(
                "{} can't be read by you, so the service won't read it either: {error}",
                path.display()
            )),
        }
    }

    fn may_share(&mut self, path: &Path, read_only: bool) -> Result<Held, String> {
        let opened = as_client(self.file, || open_shared_folder(path, read_only))?;
        let folder = opened.map_err(|error| {
            format!(
                "You can't {} {} and change who may open it, so the service won't share it: {error}",
                if read_only { "read" } else { "read and write" },
                path.display()
            )
        })?;
        the_folder_itself(&folder, path)?;
        Ok(Box::new(folder))
    }

    fn may_place(&mut self, path: &Path) -> Result<Held, String> {
        let opened = as_client(self.file, || {
            open_folder(path, FILE_LIST_DIRECTORY | FILE_TRAVERSE | FILE_ADD_SUBDIRECTORY)
        })?;
        let folder = opened.map_err(|error| {
            format!(
                "You can't make a folder in {}, so the service won't keep a machine there: {error}",
                path.display()
            )
        })?;
        the_folder_itself(&folder, path)?;
        on_a_fixed_drive(&folder, path)?;
        Ok(Box::new(folder))
    }
}

/// Opens a folder to share, as whoever is asking. Changing who may open it is asked for too:
/// sharing it lets the machine's own account in, and the service, as the system, does that only to a
/// folder the user could do it to. It isn't opened for others to delete or rename, so while it's
/// held, it stays where it is, and so does every folder it's in.
fn open_shared_folder(path: &Path, read_only: bool) -> io::Result<File> {
    let mut access = FILE_LIST_DIRECTORY | FILE_TRAVERSE | READ_CONTROL | WRITE_DAC;
    if !read_only {
        access |= FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY;
    }
    open_folder(path, access)
}

/// Opens a folder as whoever is asking, for `access`, as `open_shared_folder` does, which is also how
/// a folder to keep a machine in is opened, and held while the machine's folder is made in it.
fn open_folder(path: &Path, access: u32) -> io::Result<File> {
    OpenOptions::new()
        .access_mode(access)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        // A folder opens only with backup semantics. They let a holder of the backup privilege past
        // the folder's rules, which an app's user doesn't have enabled, and the rules are what's
        // being asked. A link is opened as itself, not followed, so it can be refused.
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

/// Whether `folder`, opened from `path`, is a folder by that very path: not a link of any kind,
/// and not reached through one, as a junction anywhere along the path would hand the machine some
/// other folder than the one the user was checked for.
pub(super) fn the_folder_itself(folder: &File, path: &Path) -> Result<(), String> {
    let attributes =
        folder.metadata().map_err(|error| format!("{}: {error}", path.display()))?.file_attributes();
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(format!(
            "{} is a link to another folder, which isn't used. Name the folder itself.",
            path.display()
        ));
    }
    if attributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(format!("{} isn't a folder.", path.display()));
    }
    let real = final_path(folder).map_err(|error| format!("{}: {error}", path.display()))?;
    let asked = std::path::absolute(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let plain = |text: &str| text.trim_end_matches('\\').to_lowercase();
    if plain(&real) != plain(&asked.to_string_lossy()) {
        return Err(format!(
            "{} leads to {real}, as a link on the way there does. Name the folder by its own path.",
            path.display()
        ));
    }
    Ok(())
}

/// Whether the open folder is on one of this PC's fixed drives, not a removable or a network one, as
/// a machine's disks are on a disk the PC doesn't take away.
fn on_a_fixed_drive(folder: &File, path: &Path) -> Result<(), String> {
    let real = final_path(folder).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut drive = real.chars();
    let letter = drive.next().filter(char::is_ascii_alphabetic);
    if letter.is_none() || drive.next() != Some(':') {
        return Err(format!("{} isn't on a drive of this PC's own.", path.display()));
    }
    let root = wide(format!("{}:\\", letter.unwrap_or('C')));
    // SAFETY: the path ends in a zero.
    if unsafe { GetDriveTypeW(root.as_ptr()) } != DRIVE_FIXED {
        return Err(format!(
            "{} is on a removable, network or other drive. Machines are kept on this PC's fixed drives.",
            path.display()
        ));
    }
    Ok(())
}

/// The path Windows has for an open file, with every link resolved, as a drive letter's path.
fn final_path(file: &File) -> io::Result<String> {
    let mut buffer = vec![0u16; 1024];
    loop {
        // SAFETY: the handle is open, and the buffer is as many units long as said.
        let length = unsafe {
            GetFinalPathNameByHandleW(
                file.as_raw_handle(),
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
            )
        } as usize;
        if length == 0 {
            return Err(io::Error::last_os_error());
        }
        if length < buffer.len() {
            let text = std::ffi::OsString::from_wide(&buffer[..length]).to_string_lossy().into_owned();
            return Ok(text.strip_prefix(r"\\?\").map(str::to_owned).unwrap_or(text));
        }
        buffer.resize(length + 1, 0);
    }
}

#[cfg(test)]
mod folder_tests {
    use super::*;
    use std::process::Command;

    fn junction(link: &Path, target: &Path) {
        let made =
            Command::new("cmd").arg("/c").arg("mklink").arg("/J").arg(link).arg(target).output().unwrap();
        assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stdout));
    }

    fn check(path: &Path) -> Result<(), String> {
        let folder = open_shared_folder(path, false).map_err(|error| error.to_string())?;
        the_folder_itself(&folder, path)
    }

    #[test]
    fn only_a_folder_by_its_own_path_is_shared() {
        let root = std::env::temp_dir().join(format!("gh-share-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let real = root.join("real");
        std::fs::create_dir_all(real.join("inner")).unwrap();
        junction(&root.join("link"), &real);

        assert_eq!(check(&real), Ok(()));
        assert_eq!(check(&real.join("inner").join("")), Ok(()), "a trailing backslash is the same folder");
        let link = check(&root.join("link")).unwrap_err();
        assert!(link.contains("is a link"), "{link}");
        let through = check(&root.join("link").join("inner")).unwrap_err();
        assert!(through.contains("leads to"), "{through}");

        // A folder to keep a machine in is the same, and on one of this PC's fixed drives.
        let place = |path: &Path| {
            let folder = open_folder(path, FILE_LIST_DIRECTORY | FILE_TRAVERSE | FILE_ADD_SUBDIRECTORY)
                .map_err(|error| error.to_string())?;
            the_folder_itself(&folder, path)?;
            on_a_fixed_drive(&folder, path)
        };
        assert_eq!(place(&real), Ok(()));
        assert!(place(&root.join("link")).is_err());

        // Held, the folder can't be moved out of the way for a link to take its place.
        let held = open_shared_folder(&real, false).unwrap();
        assert!(std::fs::rename(&real, root.join("moved")).is_err());
        drop(held);
        // rmdir on a junction removes the link alone.
        Command::new("cmd").arg("/c").arg("rmdir").arg(root.join("link")).status().unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use guesthouse_control::protocol::{OTHER_VERSION, PROTOCOL, request_line};
    use guesthouse_control::{CardInfo, Request, read_line};
    use guesthouse_host_windows::events::Sink;
    use guesthouse_host_windows::settings::{BootSettings, PrepareSettings};
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::channel;

    struct Nothing;

    /// A network with nothing in it, as this test must change nothing of the real one's.
    struct NoNetwork;

    impl guesthouse_host_windows::network::Network for NoNetwork {
        fn proxies(&self) -> Result<Vec<guesthouse_host_windows::network::Proxy>, String> {
            Ok(Vec::new())
        }
        fn add_proxy(&self, _: u16, _: std::net::Ipv4Addr, _: u16) -> Result<(), String> {
            Err("not here".into())
        }
        fn delete_proxy(&self, _: u16) -> Result<(), String> {
            Err("not here".into())
        }
        fn rules(&self) -> Result<Vec<String>, String> {
            Ok(Vec::new())
        }
        fn add_rule(
            &self,
            _: &str,
            _: u16,
            _: guesthouse_host_windows::network::Protocol,
        ) -> Result<(), String> {
            Err("not here".into())
        }
        fn delete_rule(&self, _: &str) -> Result<(), String> {
            Err("not here".into())
        }
        fn port_free(&self, _: u16) -> bool {
            false
        }
        fn card_at(&self, _: std::net::Ipv4Addr) -> Result<String, String> {
            Err("no network".into())
        }
        fn ip_helper_running(&self) -> bool {
            false
        }
        fn bridging(&self) -> guesthouse_control::Bridging {
            guesthouse_control::Bridging::default()
        }
        fn enable_hyper_v(&self, _: &mut dyn FnMut(u8)) -> guesthouse_host_windows::hyper_v::Enabled {
            guesthouse_host_windows::hyper_v::Enabled::NotInEdition
        }
    }

    impl Work for Nothing {
        fn prepare(&self, _: &PrepareSettings, _: &dyn Sink) -> Result<(), String> {
            Err("not here".into())
        }
        fn boot(
            &self,
            _: &BootSettings,
            _: Arc<dyn Sink>,
            _: Arc<AtomicBool>,
            _: guesthouse_host_windows::guest_ctl::GuestCtl,
        ) -> Result<(), String> {
            Err("not here".into())
        }
        fn cards(&self) -> Vec<CardInfo> {
            Vec::new()
        }
        fn end(&self, _: &str) {}
        fn console_access(&self, _: Option<&str>) -> Vec<String> {
            Vec::new()
        }
        fn grow(&self, _: &Path, _: u64) -> Result<(), String> {
            Err("not here".into())
        }
        fn disk_size_gb(&self, _: &Path) -> Option<u64> {
            None
        }
        fn disks(&self) -> &dyn guesthouse_host_windows::snapshots::Disks {
            &guesthouse_host_windows::windows::RealDisks
        }
        fn report_parts(&self) -> Vec<(String, Result<String, String>)> {
            Vec::new()
        }
        fn network(&self) -> &dyn guesthouse_host_windows::network::Network {
            &NoNetwork
        }
        fn port_free(&self, _: u16) -> bool {
            false
        }
        fn udp_port_free(&self, _: u16) -> bool {
            false
        }
        fn open_view(&self, _: &crate::requests::ViewSettings) -> Result<crate::requests::Opened, String> {
            Err("not here".into())
        }
        fn free_space(&self, _: &Path) -> Result<u64, String> {
            Err("not here".into())
        }
    }

    /// The pipe as the service makes it, under a name of the test's own, asked as the app asks, by
    /// the user who made it. That other users may not make an instance of it needs another user to
    /// try, so it isn't tested here.
    #[test]
    fn the_pipe_is_taken_once_and_answers_each_request_on_an_instance_of_its_own() {
        let name = format!(r"\\.\pipe\guesthouse-test-{}", std::process::id());
        let name: &'static str = Box::leak(name.into_boxed_str());
        let root = std::env::temp_dir().join(format!("gh-pipe-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        listen(name, Service::new(Store::new(root.clone()), Nothing)).unwrap();
        assert!(
            listen(name, Service::new(Store::new(root.clone()), Nothing)).is_err(),
            "the name is taken once"
        );

        for _ in 0..3 {
            let pipe = OpenOptions::new().access_mode(0x0012_008b).open(name).unwrap();
            write_line(&mut &pipe, &request_line(&Request::Status)).unwrap();
            match read_line::<Answer>(&mut BufReader::new(&pipe)).unwrap() {
                Some(Answer::Status { machines, protocol, .. }) => {
                    assert!(machines.is_empty());
                    assert_eq!(protocol, PROTOCOL);
                }
                other => panic!("{other:?}"),
            }
        }
        // An app from before the pipe had a version is told to update, in words it shows.
        let pipe = OpenOptions::new().access_mode(0x0012_008b).open(name).unwrap();
        write_line(&mut &pipe, &Request::Status).unwrap();
        assert_eq!(
            read_line::<Answer>(&mut BufReader::new(&pipe)).unwrap(),
            Some(Answer::Refused { reason: OTHER_VERSION.into() })
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// The pipe served as `listen` serves it, under a name of the test's own, with these limits,
    /// its connections' threads made by `spawn`.
    fn serving(
        test: &str,
        limits: Limits,
        spawn: impl Fn(ConnectionWork) -> io::Result<()> + Send + 'static,
    ) -> &'static str {
        let name = format!(r"\\.\pipe\guesthouse-test-{test}-{}", std::process::id());
        let name: &'static str = Box::leak(name.into_boxed_str());
        let root = std::env::temp_dir().join(format!("gh-pipe-{test}-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let service = Service::new(Store::new(root), Nothing);
        let security = Descriptor::new(PIPE_SECURITY).unwrap();
        let first = instance(name, &security, true).unwrap();
        let slots = Slots::new(limits);
        thread::spawn(move || serve(name, service, security, first, &slots, spawn));
        name
    }

    /// A connection to the pipe, waiting a moment while the service makes its next instance.
    fn connect(name: &str) -> File {
        const ERROR_PIPE_BUSY: i32 = 231;
        for _ in 0..200 {
            match OpenOptions::new().access_mode(0x0012_008b).open(name) {
                Ok(pipe) => return pipe,
                Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    thread::sleep(Duration::from_millis(10))
                }
                Err(error) => panic!("{error}"),
            }
        }
        panic!("the pipe stayed busy");
    }

    /// What a connection that asks for the status gets within a few seconds: an answer, the end
    /// with none (`Some(None)`), or nothing yet (`None`).
    fn status_within(name: &'static str) -> Option<Option<Answer>> {
        let (said, heard) = channel();
        thread::spawn(move || {
            let pipe = connect(name);
            let _ = write_line(&mut &pipe, &request_line(&Request::Status));
            let _ = said.send(read_line::<Answer>(&mut BufReader::new(&pipe)).ok().flatten());
        });
        heard.recv_timeout(Duration::from_secs(5)).ok()
    }

    /// Whether the service closes `pipe`, which has asked nothing, within `seconds`.
    fn closed_within(pipe: &File, seconds: u64) -> bool {
        let until = std::time::Instant::now() + Duration::from_secs(seconds);
        while std::time::Instant::now() < until {
            // SAFETY: only asks whether the pipe is still there, reading nothing.
            let there = unsafe {
                PeekNamedPipe(pipe.as_raw_handle(), null_mut(), 0, null_mut(), null_mut(), null_mut())
            } != 0;
            if !there {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        false
    }

    /// Asks for the status until it's answered, as a connection that has just closed gives its
    /// place back a moment later. What the last try got, if none was answered.
    fn answered_soon(name: &'static str) -> Result<(), String> {
        let mut last = None;
        for _ in 0..50 {
            match status_within(name) {
                Some(Some(Answer::Status { .. })) => return Ok(()),
                other => last = Some(other),
            }
            thread::sleep(Duration::from_millis(50));
        }
        Err(format!("the last try got {last:?}"))
    }

    /// A user's connections that ask nothing hold no more than their share, the one past it is
    /// closed at once, and the pipe answers again as soon as one of theirs goes.
    #[test]
    fn connections_past_the_limit_are_closed_and_the_pipe_goes_on() {
        for (test, limits) in [
            ("per-user", Limits { per_user: 2, in_all: 100, ..LIMITS }),
            ("in-all", Limits { per_user: 100, in_all: 2, ..LIMITS }),
        ] {
            let name = serving(test, limits, on_new_thread);
            let held = [connect(name), connect(name)];
            assert!(!closed_within(&held[0], 1), "{test}: a connection within the limit is kept");
            let past = connect(name);
            assert!(closed_within(&past, 5), "{test}: the connection past the limit is closed");
            assert_eq!(status_within(name), Some(None), "{test}: and so is the next, while the limit holds");
            let [first, _second] = held;
            drop(first);
            answered_soon(name)
                .unwrap_or_else(|why| panic!("{test}: once one goes, the pipe answers again, but {why}"));
        }
    }

    #[test]
    fn a_connection_no_thread_could_be_made_for_is_closed_and_the_pipe_goes_on() {
        let failed = AtomicBool::new(false);
        let name = serving("no-thread", LIMITS, move |work| {
            if failed.swap(true, Ordering::SeqCst) {
                on_new_thread(work)
            } else {
                Err(io::Error::other("no thread this time"))
            }
        });
        assert_eq!(status_within(name), Some(None), "the first is closed unanswered");
        answered_soon(name).unwrap_or_else(|why| panic!("the next is answered, but {why}"));
    }

    /// A connection that sends nothing is closed once its time is up, and gives its place back;
    /// one that sends its request is answered.
    #[test]
    fn a_connection_that_sends_no_request_in_time_is_closed() {
        let limits = Limits { per_user: 1, in_all: 100, first_line: Duration::from_millis(300) };
        let name = serving("first-line", limits, on_new_thread);
        let silent = connect(name);
        assert!(closed_within(&silent, 5), "a connection that sends nothing is closed");
        answered_soon(name).unwrap_or_else(|why| panic!("its place is free for one that asks, but {why}"));
    }
}
