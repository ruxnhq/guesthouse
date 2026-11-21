//! The words between Guesthouse's app and the Guesthouse service on a Windows host. The service runs
//! as the system, as HCS needs, and the app runs as its user, so the app never touches a machine
//! itself: it asks the service, on the service's pipe, and shows what the service says.
//!
//! Each connection carries one request, a line of JSON, and its answer. Most answers are one line.
//! `Create` and `Watch` answer with a line for each event, as it happens, until the work ends or the
//! app closes the pipe. The service keeps every rule; the app only asks and shows.
//!
//! Nothing here calls into Windows but the pipe, which std opens as a file, so it builds and is
//! tested anywhere.

pub mod client;
pub mod events;
pub mod screen;

pub use events::{Event, State};

use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Read, Write};
use std::path::PathBuf;

/// The service's pipe, which only the system, administrators and the signed-in user may open.
pub const PIPE: &str = r"\\.\pipe\guesthouse";
/// The longest request the service reads. Requests are short, and a longer one isn't from the app.
pub const REQUEST_LIMIT: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "ask", rename_all = "snake_case")]
pub enum Request {
    /// The machines, the cards that can be shared, and the service's version.
    Status,
    /// Makes a new machine's disks from an ISO. Its events come back as it works, then `Done`.
    Create { machine: NewMachine },
    /// Boots a machine. The answer comes once it's begun; `Watch` follows it.
    Start { name: String },
    /// Asks a running machine's guest to shut down. Asked again, the machine is stopped at once.
    Stop { name: String },
    /// Stops a running machine at once, as pulling its power would, with no shutdown in the guest.
    End { name: String },
    /// What the remoteman viewer needs to show a Windows guest's screen.
    Screen { name: String },
    /// Deletes a machine that isn't running, with its disks.
    Delete { name: String },
    /// The guest's administrator and password, for Remote Desktop or SSH.
    Account { name: String },
    /// A machine's recent events, then each new one, until the app closes the pipe.
    Watch { name: String },
}

/// What a new machine is made from and given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewMachine {
    /// Letters, digits, - and _. It names the machine's folder and its VM.
    pub name: String,
    /// A Windows ISO, or its install.wim or install.esd, or an Ubuntu Server ISO.
    pub image: PathBuf,
    pub memory_mb: u64,
    pub processors: u32,
    /// The disk's largest size; it only takes what the guest writes.
    pub size_gb: u64,
    pub card: Card,
}

/// Which card a machine is given.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "card", content = "name", rename_all = "snake_case")]
pub enum Card {
    /// The card with the most memory of its own, which is the discrete one whoever makes it.
    #[default]
    Default,
    /// The card of this name, as `Status` lists it.
    Named(String),
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "answer", rename_all = "snake_case")]
pub enum Answer {
    Status {
        version: String,
        machines: Vec<Machine>,
        cards: Vec<CardInfo>,
    },
    /// Done as asked.
    Ok,
    /// Not done, and why, in words for the person.
    Refused {
        reason: String,
    },
    /// One of a machine's events, for `Create` and `Watch`.
    Event {
        event: Event,
    },
    /// The end of `Create`'s work: `reason` is `None` when it worked.
    Done {
        reason: Option<String>,
    },
    Account {
        user: String,
        password: String,
    },
    /// Where a guest's screen is and what the viewer proves itself and expects with, for
    /// [`screen::viewer_state`]. The certificate and key are hex.
    Screen {
        address: String,
        port: u16,
        host_fingerprint: String,
        viewer_certificate: String,
        viewer_key: String,
    },
}

/// A machine as the service sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Machine {
    pub name: String,
    pub system: System,
    pub state: State,
    /// Whether its first boot has set it up, after which it boots with its card.
    pub set_up: bool,
    pub memory_mb: u64,
    pub processors: u32,
    /// The card it's given, by name, if any.
    pub card: Option<String>,
    /// The address the guest last reported, for Remote Desktop or SSH.
    pub address: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum System {
    Windows,
    Linux,
}

/// A card that can be shared with a guest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardInfo {
    pub name: String,
    /// The memory it has of its own, in MB, if its driver says.
    pub memory_mb: Option<u64>,
    /// Whether it's the card a machine gets when none is named.
    pub default: bool,
}

/// Reads one line of JSON, at most `REQUEST_LIMIT` long, as a `T`.
pub fn read_line<T: for<'de> Deserialize<'de>>(reader: &mut impl BufRead) -> io::Result<Option<T>> {
    let mut line = String::new();
    let read = reader.by_ref().take(REQUEST_LIMIT as u64).read_line(&mut line)?;
    if read == 0 {
        return Ok(None);
    }
    serde_json::from_str(line.trim_end())
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Writes `value` as one line of JSON, and flushes it so the other end has it at once.
pub fn write_line<T: Serialize>(writer: &mut impl Write, value: &T) -> io::Result<()> {
    let mut line = serde_json::to_string(value).map_err(io::Error::other)?;
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    writer.flush()
}

/// The IPv4 address in one of a guest's report lines, other than loopback. A Windows guest writes
/// `address: 172.29.1.2`; an Ubuntu guest writes what `ip -o address` says, such as
/// `address: 2: eth0    inet 172.29.1.2/20 ...`.
pub fn address_in(line: &str) -> Option<String> {
    let rest = line.strip_prefix("guesthouse: address: ").or_else(|| line.strip_prefix("address: "))?;
    rest.split(|c: char| c.is_whitespace() || c == '/')
        .find(|word| word.parse::<std::net::Ipv4Addr>().is_ok_and(|address| !address.is_loopback()))
        .map(str::to_owned)
}

/// HCS's ID for machine `name`, which also names its pipes. Every other program's machines are left
/// alone.
pub fn vm_id(name: &str) -> String {
    format!("guesthouse-{name}")
}

/// The pipe machine `name`'s console is served on: its own display, which HCS's worker serves for
/// Windows' Remote Desktop control to reach through RdpBase's named-pipe connector, for only the
/// user at the console and administrators.
pub fn console_pipe(name: &str) -> String {
    format!(r"\\.\pipe\{}-console", vm_id(name))
}

/// Whether `name` can name a machine: its folder and its VM take it as it is.
pub fn valid_name(name: &str) -> bool {
    const LONGEST: usize = 40;
    !name.is_empty()
        && name.len() <= LONGEST
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn requests_are_tagged_by_what_they_ask() {
        assert_eq!(serde_json::to_value(Request::Status).unwrap(), json!({"ask": "status"}));
        assert_eq!(
            serde_json::to_value(Request::Start { name: "win".into() }).unwrap(),
            json!({"ask": "start", "name": "win"})
        );
        let new = NewMachine {
            name: "u".into(),
            image: PathBuf::from("u.iso"),
            memory_mb: 8192,
            processors: 4,
            size_gb: 128,
            card: Card::Named("NVIDIA GeForce RTX 5090".into()),
        };
        let value = serde_json::to_value(Request::Create { machine: new }).unwrap();
        assert_eq!(value["machine"]["card"], json!({"card": "named", "name": "NVIDIA GeForce RTX 5090"}));
    }

    #[test]
    fn a_line_reads_back_as_it_was_written() {
        let mut buffer = Vec::new();
        write_line(&mut buffer, &Answer::Refused { reason: "busy".into() }).unwrap();
        write_line(&mut buffer, &Answer::Ok).unwrap();
        let mut reader = io::BufReader::new(&buffer[..]);
        assert_eq!(
            read_line::<Answer>(&mut reader).unwrap(),
            Some(Answer::Refused { reason: "busy".into() })
        );
        assert_eq!(read_line::<Answer>(&mut reader).unwrap(), Some(Answer::Ok));
        assert_eq!(read_line::<Answer>(&mut reader).unwrap(), None);
    }

    #[test]
    fn a_line_that_isnt_a_request_is_refused_as_bad_data() {
        let mut reader = io::BufReader::new(&b"{\"ask\":\"format the host\"}\n"[..]);
        assert_eq!(read_line::<Request>(&mut reader).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn an_address_is_found_in_either_systems_report() {
        assert_eq!(address_in("guesthouse: address: 172.29.167.163").as_deref(), Some("172.29.167.163"));
        assert_eq!(
            address_in("address: 2: eth0    inet 172.29.164.142/20 metric 100 brd 172.29.175.255").as_deref(),
            Some("172.29.164.142")
        );
        assert_eq!(address_in("address: 1: lo    inet 127.0.0.1/8 scope host lo"), None);
        assert_eq!(address_in("card: NVIDIA GeForce RTX 5090"), None);
    }

    #[test]
    fn a_name_is_one_a_folder_and_a_vm_take() {
        assert!(valid_name("ubuntu-24_2"));
        assert!(!valid_name(""));
        assert!(!valid_name("../x"));
        assert!(!valid_name("a b"));
    }
}
