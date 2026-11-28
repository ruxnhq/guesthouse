// SPDX-License-Identifier: GPL-2.0-or-later
//! The agent and its socket as C functions, declared in `include/guesthouse_core.h`. The Swift
//! agent calls the first group; the app calls the connection functions. Lines cross as C
//! strings that the caller frees with `guesthouse_string_free`.

use crate::agent::{Agent, Config, Subscription};
use crate::server::{self, Connection, ServeError};
use crate::store;
use std::ffi::{CStr, CString, c_char};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

/// The agent's socket, `agent.sock` in the support folder (short, so it fits a socket address).
fn socket_path() -> Option<PathBuf> {
    Some(store::support_folder()?.join("agent.sock"))
}

fn to_c(text: String) -> *mut c_char {
    // The lines are JSON, which has no nul in it; paths from the environment have none either.
    CString::new(text).map_or(std::ptr::null_mut(), CString::into_raw)
}

/// # Safety
/// `text` is null or a nul-terminated string.
unsafe fn from_c<'a>(text: *const c_char) -> Option<&'a str> {
    if text.is_null() {
        return None;
    }
    // SAFETY: as the caller promises.
    unsafe { CStr::from_ptr(text) }.to_str().ok()
}

/// The agent's socket path, or null without a home folder. Free it with
/// `guesthouse_string_free`.
#[unsafe(no_mangle)]
pub extern "C" fn guesthouse_agent_socket_path() -> *mut c_char {
    match socket_path().and_then(|path| path.to_str().map(String::from)) {
        Some(path) => to_c(path),
        None => std::ptr::null_mut(),
    }
}

/// Makes the agent for this user and finds the machines still running. Null without a home
/// folder. Never freed: an agent lasts as long as its process.
#[unsafe(no_mangle)]
pub extern "C" fn guesthouse_agent_new() -> *mut Agent {
    match Config::from_env() {
        Some(config) => Box::into_raw(Box::new(Agent::new(config))),
        None => std::ptr::null_mut(),
    }
}

/// Answers one request line with one answer line.
///
/// # Safety
/// `agent` came from `guesthouse_agent_new`; `line` is a nul-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guesthouse_agent_handle(agent: *const Agent, line: *const c_char) -> *mut c_char {
    // SAFETY: as the caller promises.
    let (agent, line) = unsafe { (agent.as_ref(), from_c(line)) };
    match (agent, line) {
        (Some(agent), Some(line)) => to_c(agent.handle(line)),
        _ => std::ptr::null_mut(),
    }
}

/// Tells the agent which build it is: the bundle's version and build number, from which it makes
/// its identity (`agent_build`) and says it in every listing.
///
/// # Safety
/// `agent` came from `guesthouse_agent_new`; `version` and `build` are nul-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guesthouse_agent_set_build(
    agent: *const Agent,
    version: *const c_char,
    build: *const c_char,
) {
    // SAFETY: as the caller promises.
    let (agent, version, build) = unsafe { (agent.as_ref(), from_c(version), from_c(build)) };
    if let (Some(agent), Some(version), Some(build)) = (agent, version, build) {
        agent.set_build(version, build);
    }
}

/// Whether the agent has nothing running and nobody listening.
///
/// # Safety
/// `agent` came from `guesthouse_agent_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guesthouse_agent_is_idle(agent: *const Agent) -> bool {
    // SAFETY: as the caller promises.
    unsafe { agent.as_ref() }.is_some_and(Agent::is_idle)
}

/// Answers on the socket at `path` until the agent is idle. Returns 0 when it ended idle, 1
/// when another agent already has the socket, -1 on an error (printed to standard error).
///
/// # Safety
/// `agent` came from `guesthouse_agent_new`; `path` is a nul-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guesthouse_agent_serve(agent: *const Agent, path: *const c_char) -> i32 {
    // SAFETY: as the caller promises.
    let (agent, path) = unsafe { (agent.as_ref(), from_c(path)) };
    let (Some(agent), Some(path)) = (agent, path) else {
        return -1;
    };
    match server::serve(agent, Path::new(path)) {
        Ok(()) => 0,
        Err(ServeError::AlreadyRunning) => 1,
        Err(ServeError::Io(error)) => {
            eprintln!("guesthouse-agent: {path}: {error}");
            -1
        }
    }
}

/// Answers a question about agent builds (see `agent_build::answer`): the identity of the agent
/// program in the app's own bundle, and what to do about the agent that answered a listing.
///
/// # Safety
/// `request` is a nul-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guesthouse_agent_build(request: *const c_char) -> *mut c_char {
    // SAFETY: as the caller promises.
    match unsafe { from_c(request) } {
        Some(request) => to_c(crate::agent_build::answer(request)),
        None => std::ptr::null_mut(),
    }
}

/// State changes, as lines, for one listener.
pub struct Events {
    lines: Receiver<String>,
    _subscription: Subscription,
}

/// Starts listening for state changes. Close it with `guesthouse_events_close`.
///
/// # Safety
/// `agent` came from `guesthouse_agent_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guesthouse_agent_events(agent: *const Agent) -> *mut Events {
    // SAFETY: as the caller promises.
    let Some(agent) = (unsafe { agent.as_ref() }) else {
        return std::ptr::null_mut();
    };
    let (sender, lines) = mpsc::channel();
    let subscription = agent.subscribe(sender);
    Box::into_raw(Box::new(Events { lines, _subscription: subscription }))
}

/// The next state change, or null after `timeout_ms` with none.
///
/// # Safety
/// `events` came from `guesthouse_agent_events` and is used by one thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guesthouse_events_next(events: *mut Events, timeout_ms: u32) -> *mut c_char {
    // SAFETY: as the caller promises.
    let Some(events) = (unsafe { events.as_ref() }) else {
        return std::ptr::null_mut();
    };
    match events.lines.recv_timeout(Duration::from_millis(timeout_ms.into())) {
        Ok(line) => to_c(line),
        Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => std::ptr::null_mut(),
    }
}

/// Stops listening.
///
/// # Safety
/// `events` is null or came from `guesthouse_agent_events`, and is not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guesthouse_events_close(events: *mut Events) {
    if !events.is_null() {
        // SAFETY: from `Box::into_raw` above.
        drop(unsafe { Box::from_raw(events) });
    }
}

/// Connects to the agent's socket at `path`. Null when there is no agent there.
///
/// # Safety
/// `path` is a nul-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guesthouse_connection_open(path: *const c_char) -> *mut Connection {
    // SAFETY: as the caller promises.
    let Some(path) = (unsafe { from_c(path) }) else {
        return std::ptr::null_mut();
    };
    match Connection::open(Path::new(path)) {
        Ok(connection) => Box::into_raw(Box::new(connection)),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Sends one request line. False when the agent is gone.
///
/// # Safety
/// `connection` is open; `line` is a nul-terminated string; one thread at a time sends.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guesthouse_connection_send(
    connection: *mut Connection,
    line: *const c_char,
) -> bool {
    // SAFETY: as the caller promises.
    let (connection, line) = unsafe { (connection.as_ref(), from_c(line)) };
    match (connection, line) {
        (Some(connection), Some(line)) => connection.send(line).is_ok(),
        _ => false,
    }
}

/// The next line from the agent. Null after `timeout_ms` with none, and `*closed` set when the
/// agent hung up.
///
/// # Safety
/// `connection` is open; `closed` points to a bool.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guesthouse_connection_receive(
    connection: *mut Connection,
    timeout_ms: u32,
    closed: *mut bool,
) -> *mut c_char {
    // SAFETY: as the caller promises.
    let (connection, closed) = unsafe { (connection.as_ref(), closed.as_mut()) };
    let (Some(connection), Some(closed)) = (connection, closed) else {
        return std::ptr::null_mut();
    };
    *closed = false;
    match connection.receive(Duration::from_millis(timeout_ms.into())) {
        Ok(Some(line)) => to_c(line),
        Ok(None) => std::ptr::null_mut(),
        Err(_) => {
            *closed = true;
            std::ptr::null_mut()
        }
    }
}

/// The process id of the agent at the other end, or 0 when it cannot be told.
///
/// # Safety
/// `connection` is null or open.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guesthouse_connection_peer_pid(connection: *const Connection) -> i32 {
    // SAFETY: as the caller promises.
    unsafe { connection.as_ref() }.and_then(Connection::peer_pid).unwrap_or(0)
}

/// Closes the connection.
///
/// # Safety
/// `connection` is null or open, and not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn guesthouse_connection_close(connection: *mut Connection) {
    if !connection.is_null() {
        // SAFETY: from `Box::into_raw` above.
        drop(unsafe { Box::from_raw(connection) });
    }
}
