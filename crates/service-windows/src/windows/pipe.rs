//! The pipe the app asks on. Each request has a pipe instance of its own, and the next instance is
//! made before a request is served, so the pipe's name is always the service's: no other program
//! can take it between two requests and pose as the service.
//!
//! Who may ask is checked twice. The pipe's own security lets in the system, administrators and
//! interactive users, and lets none of them but the service make an instance of it. Then each
//! request is checked against who is asking: the system, an elevated administrator, or the user
//! signed in at the console. The console's user changes as people sign in and out, which a pipe's
//! fixed security can't follow, so that's checked for each request.

use super::log;
use super::security::{Descriptor, SYSTEM, is_administrator, last_error, own_user, token_user, wide};
use crate::requests::{Connection, Service, Work};
use guesthouse_control::{Answer, REQUEST_LIMIT, Request, read_line, write_line};
use std::fs::File;
use std::io::{self, BufReader};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::ptr::null_mut;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_PIPE_CONNECTED, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::{RevertToSelf, SECURITY_ATTRIBUTES, TOKEN_QUERY};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FlushFileBuffers, PIPE_ACCESS_DUPLEX,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, ImpersonateNamedPipeClient, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, PeekNamedPipe,
};
use windows_sys::Win32::System::RemoteDesktop::{WTSGetActiveConsoleSessionId, WTSQueryUserToken};
use windows_sys::Win32::System::Threading::{GetCurrentThread, OpenThreadToken};

/// The system, administrators and the pipe's owner, who is whoever made it, may do anything with
/// the pipe, making more instances of it too. Interactive users may read it and write to it
/// (`FILE_GENERIC_READ | FILE_WRITE_DATA`), and not append to it, which for a pipe is making an
/// instance of it. The protection stops a parent folder's rules being added.
const PIPE_SECURITY: &str = "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;OW)(A;;0x12008b;;;IU)";
/// Room in the pipe each way, enough for any request and most answers.
const BUFFER: u32 = REQUEST_LIMIT as u32;
/// How long to wait before trying again when an instance couldn't be made.
const AGAIN: Duration = Duration::from_secs(1);

/// Makes the pipe's first instance, which fails if any other program has its name, and then serves
/// it on a thread of its own for as long as the process runs.
pub fn listen<W: Work>(name: &'static str, service: Arc<Service<W>>) -> Result<(), String> {
    let security = Descriptor::new(PIPE_SECURITY)?;
    let first = instance(name, &security, true).map_err(|error| {
        format!("{error}. Another program may have the pipe's name, or the service is running already.")
    })?;
    thread::spawn(move || serve(name, service, security, first));
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

fn serve<W: Work>(name: &str, service: Arc<Service<W>>, security: Descriptor, first: OwnedHandle) {
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
        if connected {
            let service = Arc::clone(&service);
            thread::spawn(move || answer(&service, File::from(served)));
        }
    }
}

/// Reads one request and answers it, if whoever asked may ask.
fn answer<W: Work>(service: &Arc<Service<W>>, pipe: File) {
    let request = read_line::<Request>(&mut BufReader::new(&pipe));
    let mut connection = Pipe { file: &pipe };
    match request {
        Ok(Some(request)) => match allowed(&pipe) {
            Ok(()) => service.answer(request, &mut connection),
            Err(reason) => {
                log(&reason);
                let _ = connection.send(&Answer::Refused { reason });
            }
        },
        Ok(None) => {}
        Err(_) => {
            let reason =
                "That isn't a request the service knows. The app and the service may be different versions.";
            let _ = connection.send(&Answer::Refused { reason: reason.into() });
        }
    }
    // The answer is waited on until the app has read it, and then the instance is closed, which
    // the app reads as the end. Disconnecting it instead would throw away what the app hadn't read,
    // and the app would read an error, not the end.
    // SAFETY: the pipe's handle is ours and open.
    unsafe { FlushFileBuffers(pipe.as_raw_handle()) };
}

/// Whether the one asking on `pipe` may: the system, an elevated administrator, or the user signed
/// in at the console. With nobody signed in, as when the service runs at a prompt, the service's
/// own user stands in for the console's.
fn allowed(pipe: &File) -> Result<(), String> {
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
    if administrator || user == SYSTEM || Some(&user) == console_user().or_else(own_user).as_ref() {
        Ok(())
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
}

impl Connection for Pipe<'_> {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use guesthouse_control::{CardInfo, read_line};
    use guesthouse_host_windows::events::Sink;
    use guesthouse_host_windows::settings::{BootSettings, PrepareSettings};
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;
    use std::sync::atomic::AtomicBool;

    struct Nothing;

    impl Work for Nothing {
        fn prepare(&self, _: &PrepareSettings, _: &dyn Sink) -> Result<(), String> {
            Err("not here".into())
        }
        fn boot(&self, _: &BootSettings, _: Arc<dyn Sink>, _: Arc<AtomicBool>) -> Result<(), String> {
            Err("not here".into())
        }
        fn cards(&self) -> Vec<CardInfo> {
            Vec::new()
        }
        fn end(&self, _: &str) {}
        fn console_access(&self) -> Vec<String> {
            Vec::new()
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
            write_line(&mut &pipe, &Request::Status).unwrap();
            match read_line::<Answer>(&mut BufReader::new(&pipe)).unwrap() {
                Some(Answer::Status { machines, .. }) => assert!(machines.is_empty()),
                other => panic!("{other:?}"),
            }
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
