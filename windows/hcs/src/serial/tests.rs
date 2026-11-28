//! A pipe served by this test process itself stands in for a machine's worker: the port is taken
//! from the process or account it's told to expect, and from no other, and whoever serves it gets
//! only an identification token of the one who opened it.

use super::*;
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::Security::{
    RevertToSelf, SECURITY_IMPERSONATION_LEVEL, SecurityIdentification, TokenImpersonationLevel,
};
use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, ImpersonateNamedPipeClient, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentThread, OpenThreadToken};

/// A pipe name no other test, or run, has.
fn pipe_name() -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!(r"\\.\pipe\guesthouse-serial-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst))
}

/// One instance of pipe `name`, served by this process, waiting for its client on a thread of its
/// own; `then` runs on that thread once the client has connected.
fn serve<T: Send + 'static>(
    name: &str,
    then: impl FnOnce(&OwnedHandle) -> T + Send + 'static,
) -> thread::JoinHandle<T> {
    let text: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    // SAFETY: the name ends in a zero; the pipe gets the default security.
    let handle = unsafe {
        CreateNamedPipeW(
            text.as_ptr(),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            4096,
            4096,
            0,
            std::ptr::null(),
        )
    };
    assert_ne!(handle, INVALID_HANDLE_VALUE, "{}", io::Error::last_os_error());
    // SAFETY: a handle Windows just gave us, owned by nothing else.
    let pipe = unsafe { OwnedHandle::from_raw_handle(handle) };
    thread::spawn(move || {
        // SAFETY: the instance is ours, waited on with no overlapped I/O. A client that came first
        // reads as an error that is a connection all the same.
        unsafe { ConnectNamedPipe(pipe.as_raw_handle(), null_mut()) };
        then(&pipe)
    })
}

/// The account this test runs as.
fn own_account() -> String {
    let mut token: HANDLE = null_mut();
    // SAFETY: the pseudo-handle needs no closing; the token is closed below.
    assert_ne!(unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }, 0);
    let account = token_user(token).expect("the test's own account");
    // SAFETY: ours, and not used again.
    unsafe { CloseHandle(token) };
    account
}

#[test]
fn the_port_is_taken_from_the_process_expected() {
    let name = pipe_name();
    let server = serve(&name, |_| ());
    let port = SerialPort::open(&name, &Server::Process(std::process::id()));
    assert!(port.is_ok(), "{}", port.err().unwrap());
    server.join().unwrap();
}

#[test]
fn a_pipe_served_by_another_process_than_the_one_expected_is_let_go() {
    let name = pipe_name();
    let server = serve(&name, |_| ());
    let other = std::process::id().wrapping_add(4);
    match SerialPort::open(&name, &Server::Process(other)) {
        Err(OpenError::OtherServer { process, expected, .. }) => {
            assert_eq!(process, std::process::id());
            assert_eq!(expected, Server::Process(other));
        }
        Err(error) => panic!("refused for another reason: {error}"),
        Ok(_) => panic!("the port was taken from a process that wasn't the one expected"),
    }
    server.join().unwrap();
}

#[test]
fn the_port_is_taken_from_a_process_running_as_the_account_expected() {
    let name = pipe_name();
    let server = serve(&name, |_| ());
    let port = SerialPort::open(&name, &Server::Account(own_account()));
    assert!(port.is_ok(), "{}", port.err().unwrap());
    server.join().unwrap();
}

#[test]
fn a_pipe_served_by_another_account_than_the_one_expected_is_let_go() {
    let name = pipe_name();
    let server = serve(&name, |_| ());
    // A machine's worker's account, which this test doesn't run as.
    let worker =
        crate::worker::account(crate::hvsock::parse_guid("4d9e6102-3034-5803-82de-c685d6232367").unwrap());
    match SerialPort::open(&name, &Server::Account(worker.clone())) {
        Err(OpenError::OtherServer { process, account, .. }) => {
            assert_eq!(process, std::process::id());
            assert_eq!(account, Some(Ok(own_account())));
        }
        Err(error) => panic!("refused for another reason: {error}"),
        Ok(_) => panic!("the port was taken from a process not running as {worker}"),
    }
    server.join().unwrap();
}

/// The server reads what the client wrote, which it must before it can act as the client, and then
/// asks how far it may.
#[test]
fn whoever_serves_the_port_may_only_identify_who_opened_it() {
    let name = pipe_name();
    let server = serve(&name, |pipe| {
        let mut byte = 0u8;
        let mut read = 0u32;
        // SAFETY: a one-byte buffer for a one-byte read on our own instance.
        let got = unsafe { ReadFile(pipe.as_raw_handle(), &mut byte, 1, &mut read, null_mut()) };
        assert!(got != 0 && read == 1, "{}", io::Error::last_os_error());
        // SAFETY: our own instance, which a client has written to.
        assert_ne!(unsafe { ImpersonateNamedPipeClient(pipe.as_raw_handle()) }, 0);
        let mut token: HANDLE = null_mut();
        // SAFETY: the thread's token while it impersonates, opened as this process.
        let opened = unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut token) } != 0;
        let mut level: SECURITY_IMPERSONATION_LEVEL = 0;
        let mut size = 0u32;
        // SAFETY: the level comes back in a place its size.
        let asked = opened
            && unsafe {
                GetTokenInformation(
                    token,
                    TokenImpersonationLevel,
                    (&raw mut level).cast(),
                    size_of::<SECURITY_IMPERSONATION_LEVEL>() as u32,
                    &mut size,
                )
            } != 0;
        // SAFETY: plain calls; the token is ours.
        unsafe {
            RevertToSelf();
            if opened {
                CloseHandle(token);
            }
        }
        asked.then_some(level)
    });
    let port = SerialPort::open(&name, &Server::Process(std::process::id())).unwrap();
    port.write_all(b"x").unwrap();
    assert_eq!(server.join().unwrap(), Some(SecurityIdentification));
}

#[test]
fn each_boot_has_pipe_names_of_its_own() {
    let (one, two) = (pipe_tag().unwrap(), pipe_tag().unwrap());
    assert_eq!(one.len(), 2 * TAG_BYTES);
    assert!(one.chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(one, two);
}
