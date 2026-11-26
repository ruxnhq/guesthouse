//! What the app and the program say to each other: one request as a line of JSON on a Unix socket in
//! the run folder, and one line back. The monitor and the daemon are reached the same way, and a
//! socket in the run folder is only open to this user. A program that is not running has no answer,
//! which is how the app finds out it is not there.

use super::SOCKET;
pub use guesthouse_remote_view_core::share::ShareState;
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::IpAddr;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A request is a few dozen bytes. A longer line is not one of ours.
const LONGEST_REQUEST: u64 = 4096;
/// A status with the most devices the relay allows is a few kilobytes.
const LONGEST_REPLY: u64 = 256 * 1024;
/// How long either side waits for the other to say its line.
const LINE_TIMEOUT: Duration = Duration::from_secs(2);
const SOCKET_MODE: u32 = 0o600;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Only the state.
    Status,
    /// Listens on the network, or moves to this address and port. What is left out is what the program
    /// was started with, and the address defaults to every network of this PC.
    Open {
        #[serde(default)]
        address: Option<IpAddr>,
        #[serde(default)]
        port: Option<u16>,
        /// The name to announce the machine by. Left out: the one it has.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// The guest host's fingerprint, which the announcement carries the start of. Left out: the
        /// one it has; until there is one nothing is announced.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fingerprint: Option<String>,
    },
    /// Stops listening on the network, with the machine and the program still running.
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayReply {
    /// What it did, or only the state, and the state after.
    Status(RelayStatus),
    /// Why it did not.
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// The helper's own state, as it answers on its socket. Not the screen's [`ShareInfo`]: this is
/// only the network side, which is `off`, `listening` or `failed` (`starting` is the app's word for a
/// machine that is not up yet, which the helper never says).
///
/// [`ShareInfo`]: guesthouse_remote_view_core::share::ShareInfo
pub struct RelayStatus {
    pub machine: String,
    pub state: ShareState,
    /// The address and port devices on the network send to, when listening.
    pub address: Option<IpAddr>,
    pub port: Option<u16>,
    pub loopback_port: u16,
    pub reason: Option<String>,
    pub counters: Counters,
    pub clients: Vec<RelayClient>,
}

/// Totals since the network side was last opened.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counters {
    pub up_packets: u64,
    pub up_bytes: u64,
    pub down_packets: u64,
    pub down_bytes: u64,
    pub clients_started: u64,
    pub clients_expired: u64,
    pub refused_full: u64,
    pub errors: u64,
}

impl From<guesthouse_relay::Counters> for Counters {
    fn from(from: guesthouse_relay::Counters) -> Self {
        Self {
            up_packets: from.up_packets,
            up_bytes: from.up_bytes,
            down_packets: from.down_packets,
            down_bytes: from.down_bytes,
            clients_started: from.clients_started,
            clients_expired: from.clients_expired,
            refused_full: from.refused_full,
            errors: from.errors,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayClient {
    pub address: String,
    pub idle_ms: u64,
    pub up_packets: u64,
    pub down_packets: u64,
}

impl From<&guesthouse_relay::ClientInfo> for RelayClient {
    fn from(from: &guesthouse_relay::ClientInfo) -> Self {
        Self {
            address: from.address.to_string(),
            idle_ms: from.idle.as_millis() as u64,
            up_packets: from.up_packets,
            down_packets: from.down_packets,
        }
    }
}

pub fn socket_path(run_folder: &Path) -> PathBuf {
    run_folder.join(SOCKET)
}

/// Asks the program of the machine in this run folder. An error means it did not answer: it is not
/// running, or not any more.
pub fn ask(run_folder: &Path, request: &Request) -> io::Result<RelayReply> {
    let mut stream = UnixStream::connect(socket_path(run_folder))?;
    stream.set_read_timeout(Some(LINE_TIMEOUT))?;
    stream.set_write_timeout(Some(LINE_TIMEOUT))?;
    let mut line = serde_json::to_string(request).map_err(io::Error::other)?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    let mut answer = String::new();
    BufReader::new(stream.take(LONGEST_REPLY)).read_line(&mut answer)?;
    serde_json::from_str(&answer).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Makes the socket, replacing one an earlier run left, for only this user.
pub fn listener(run_folder: &Path) -> io::Result<UnixListener> {
    let path = socket_path(run_folder);
    match std::fs::remove_file(&path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(SOCKET_MODE))?;
    Ok(listener)
}

/// Answers requests, one connection at a time, until the listener fails. A request that is not one
/// of ours gets an error and costs nothing else. Run it on a thread of its own.
pub fn serve(listener: &UnixListener, mut answer: impl FnMut(Request) -> RelayReply) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { return };
        let _ = serve_one(stream, &mut answer);
    }
}

fn serve_one(stream: UnixStream, answer: &mut impl FnMut(Request) -> RelayReply) -> io::Result<()> {
    stream.set_read_timeout(Some(LINE_TIMEOUT))?;
    stream.set_write_timeout(Some(LINE_TIMEOUT))?;
    let mut line = String::new();
    BufReader::new((&stream).take(LONGEST_REQUEST)).read_line(&mut line)?;
    let reply = match serde_json::from_str(&line) {
        Ok(request) => answer(request),
        Err(error) => RelayReply::Error(format!("not a request: {error}")),
    };
    let mut out = serde_json::to_string(&reply).map_err(io::Error::other)?;
    out.push('\n');
    (&stream).write_all(out.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;
    use std::thread;

    fn status(state: ShareState) -> RelayStatus {
        RelayStatus {
            machine: "m".into(),
            state,
            address: Some("192.168.1.5".parse().unwrap()),
            port: Some(7800),
            loopback_port: 19065,
            reason: None,
            counters: Counters::default(),
            clients: vec![RelayClient {
                address: "192.168.1.9:5000".into(),
                idle_ms: 20,
                up_packets: 3,
                down_packets: 4,
            }],
        }
    }

    #[test]
    fn requests_are_short_lines_with_an_op() {
        let text = |request: &Request| serde_json::to_string(request).unwrap();
        assert_eq!(text(&Request::Status), r#"{"op":"status"}"#);
        assert_eq!(text(&Request::Close), r#"{"op":"close"}"#);
        assert_eq!(
            text(&Request::Open {
                address: Some("0.0.0.0".parse().unwrap()),
                port: Some(7800),
                name: None,
                fingerprint: None
            }),
            r#"{"op":"open","address":"0.0.0.0","port":7800}"#
        );
        assert_eq!(
            serde_json::from_str::<Request>(r#"{"op":"open"}"#).unwrap(),
            Request::Open { address: None, port: None, name: None, fingerprint: None }
        );
    }

    #[test]
    fn a_request_gets_its_answer_over_the_socket() {
        let scratch = Scratch::new();
        let folder = scratch.path("run");
        std::fs::create_dir_all(&folder).unwrap();
        let listener = listener(&folder).unwrap();
        assert_eq!(
            std::fs::metadata(socket_path(&folder)).unwrap().permissions().mode() & 0o777,
            SOCKET_MODE
        );
        thread::spawn(move || {
            serve(&listener, |request| match request {
                Request::Status => RelayReply::Status(status(ShareState::Listening)),
                _ => RelayReply::Error("no".into()),
            })
        });
        assert_eq!(
            ask(&folder, &Request::Status).unwrap(),
            RelayReply::Status(status(ShareState::Listening))
        );
        assert_eq!(ask(&folder, &Request::Close).unwrap(), RelayReply::Error("no".into()));
    }

    #[test]
    fn nonsense_gets_an_error_and_the_next_request_still_works() {
        let scratch = Scratch::new();
        let folder = scratch.path("run");
        std::fs::create_dir_all(&folder).unwrap();
        let listener = listener(&folder).unwrap();
        thread::spawn(move || serve(&listener, |_| RelayReply::Status(status(ShareState::Off))));
        let mut stream = UnixStream::connect(socket_path(&folder)).unwrap();
        stream.write_all(b"{\"op\":\"explode\"}\n").unwrap();
        let mut answer = String::new();
        BufReader::new(stream).read_line(&mut answer).unwrap();
        assert!(matches!(serde_json::from_str::<RelayReply>(&answer).unwrap(), RelayReply::Error(_)));
        assert!(matches!(ask(&folder, &Request::Status).unwrap(), RelayReply::Status(_)));
    }

    #[test]
    fn nobody_listening_is_an_error() {
        let scratch = Scratch::new();
        assert!(ask(scratch.root(), &Request::Status).is_err());
    }

    #[test]
    fn an_old_socket_is_replaced() {
        let scratch = Scratch::new();
        let first = listener(scratch.root()).unwrap();
        drop(first);
        assert!(listener(scratch.root()).is_ok(), "the file the first left behind does not stop the second");
    }
}
