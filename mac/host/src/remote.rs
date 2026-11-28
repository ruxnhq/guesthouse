// SPDX-License-Identifier: GPL-2.0-or-later
//! Showing a machine's screen to another device, with remoteman's viewer. The work is done by
//! `rmhost serve`, a program of its own that the agent runs for a machine while it is running
//! and the person has allowed it (`allow_remote_view` in `machine.json`). It stays a separate
//! process on purpose: it links remoteman's core, which is GPL-3.0-or-later, and nothing of it
//! may be linked into the app or the agent.
//!
//! This file is the plain part: the machine's setting and port, the command line, and reading
//! what rmhost prints. Running the process is `agent/share.rs`.

use crate::launch::Launch;
use crate::protocol::{CloseReason, DeviceInfo, Pending};
use crate::store::{self, MACHINE_FILE};
use guesthouse_basics::json_file::write_json;
use guesthouse_remote_view_core::ctl;
use guesthouse_remote_view_core::events::PairingEvent;
use std::fs;
use std::net::UdpSocket;
use std::path::{Path, PathBuf};

// The ports a machine's screen is given, how long a pairing stays open and for what times it may be
// opened are the same on every host, so they are the shared crate's. rmhost accepts the same
// times for `pair open` and prints an error line outside them, so the agent checks first.
pub use guesthouse_remote_view_core::share::{
    FIRST_PORT, LAST_PORT, MAX_PAIRING_SECONDS, MIN_PAIRING_SECONDS, PAIRING_SECONDS,
};

// rmhost takes the words remoteman's own host takes for these (`pair accept`, `device FP allow KIND
// on|off`, `revoke FP`, `drop`), so the shared crate's words are the lines, one to a line: the
// pairing's driver builds them (a fingerprint is checked there to be the 64 hex digits of one, so a
// request cannot put a second command on the line) and `rmhost_line` turns them into lines.
// `pair close` and `pair open SECONDS` are rmhost's own (remoteman's are `pair off` and `pair on`,
// with no time), so those lines are made here.

/// The line that closes pairing.
pub const PAIR_CLOSE_LINE: &str = "pair close\n";

/// The file in the run folder that is rmhost's standard input. A named pipe, so the agent and
/// a later agent can both send it lines, and rmhost never sees the end of its input.
pub const INPUT_FILE: &str = "rmhost.in";

/// The line that opens pairing for `seconds`.
pub fn pair_open_line(seconds: u64) -> String {
    format!("pair open {seconds}\n")
}

/// The folder inside the machine's folder where rmhost keeps its key and the devices it
/// trusts, so a machine is paired once and keeps its identity from run to run.
pub const STATE_FOLDER: &str = "remote-view";

/// The log in the run folder, where rmhost's output goes and the agent reads it back.
pub const LOG_FILE: &str = "rmhost.log";
pub const PID_FILE: &str = "rmhost.pid";

/// Turns the setting on or off for machine `id`, choosing a port the first time it is turned
/// on. Returns the machine's port. Every other setting in the file stays as it is.
pub fn set_enabled(machines: &Path, id: &str, on: bool) -> Result<Option<u16>, String> {
    let machine = store::find(machines, id)?;
    let port = match (on, machine.remote_port) {
        (false, port) => port,
        (true, Some(port)) => Some(port),
        (true, None) => {
            Some(choose_port(machines, port_is_free).ok_or(guesthouse_remote_view_core::switch::NO_PORT)?)
        }
    };
    let path = Path::new(&machine.folder).join(MACHINE_FILE);
    let text = fs::read_to_string(&path).map_err(|e| format!("Cannot read {MACHINE_FILE}: {e}"))?;
    let mut settings: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("{MACHINE_FILE} is not valid: {e}"))?;
    settings["allow_remote_view"] = on.into();
    if let Some(port) = port {
        settings["remote_port"] = port.into();
    }
    write_json(&path, &settings).map_err(|e| format!("Cannot write {MACHINE_FILE}: {e}"))?;
    Ok(port)
}

/// The first port in range that no machine has taken and `is_free` accepts.
pub fn choose_port(machines: &Path, is_free: impl Fn(u16) -> bool) -> Option<u16> {
    let taken: Vec<u16> = store::list(machines)
        .map(|listing| listing.machines.iter().filter_map(|m| m.remote_port).collect())
        .unwrap_or_default();
    guesthouse_remote_view_core::switch::choose_port(&taken, is_free)
}

/// remoteman's transport is QUIC, which is UDP, so that is the kind of port to try.
fn port_is_free(port: u16) -> bool {
    UdpSocket::bind(("0.0.0.0", port)).is_ok()
}

/// The arguments for `rmhost serve`: the machine's frames, its own input socket, its own port
/// and state folder, and the name viewers are shown. Never `-p`: pairing is opened by a line on
/// its standard input, for a time.
pub fn serve_args(launch: &Launch, machine_folder: &Path, name: &str, port: u16) -> Vec<String> {
    vec![
        "serve".to_string(),
        "-q".into(),
        launch.remote_socket.display().to_string(),
        "-n".into(),
        launch.frame_name.clone(),
        "-l".into(),
        format!("0.0.0.0:{port}"),
        "-s".into(),
        machine_folder.join(STATE_FOLDER).display().to_string(),
        "-N".into(),
        name.to_string(),
    ]
}

/// What rmhost says about itself, one line at a time.
#[derive(Debug, PartialEq, Eq)]
pub enum LogEvent {
    /// It is listening, with this host's fingerprint, and whether pairing is open.
    Listening {
        port: u16,
        fingerprint: String,
        pairing: bool,
    },
    /// A viewer connected, under this name.
    Viewer(String),
    ViewerLeft,
    /// Pairing opened, until this many seconds since 1970 (0: with no limit).
    PairingOpen {
        until_secs: u64,
    },
    /// The trusted devices, whenever the list changes.
    Devices(Vec<DeviceInfo>),
    /// A device waits to be accepted or declined.
    PairingPending(Pending),
    /// A device paired, by this fingerprint, with the name it gave.
    Paired {
        fingerprint: String,
        name: String,
    },
    /// Pairing closed, and why: `paired`, `declined`, `timeout`, `asked` or `other`.
    PairingClosed {
        reason: CloseReason,
    },
}

/// What rmhost said about pairing, as the shared pairing's event, or `None` for a line that is
/// not about pairing. Fingerprints are lower case in both lines, so a device that waits and the
/// same device that paired are one. A pairing with no time limit (`until 0`, rmhost's lab flag
/// only) is not fed: the shared pairing takes 0 as already ended.
pub fn pairing_event(event: &LogEvent) -> Option<PairingEvent> {
    match event {
        LogEvent::PairingOpen { until_secs } if *until_secs > 0 => {
            Some(PairingEvent::PairingOpen { until_unix_seconds: *until_secs })
        }
        LogEvent::PairingPending(device) => Some(PairingEvent::DeviceWaits {
            name: device.name.clone(),
            fingerprint: device.fingerprint.to_ascii_lowercase(),
            code: device.code.clone(),
            seconds: u32::try_from(device.seconds).unwrap_or(u32::MAX),
        }),
        LogEvent::Paired { fingerprint, name } => {
            Some(PairingEvent::Paired { fingerprint: fingerprint.to_ascii_lowercase(), name: name.clone() })
        }
        LogEvent::PairingClosed { reason } => Some(PairingEvent::PairingClosed { reason: *reason }),
        _ => None,
    }
}

/// The line rmhost reads for a word of the shared pairing, or why it has none. The pairing says
/// `pair on SECONDS` and `pair off`, which are remoteman's; rmhost's own are `pair open SECONDS`
/// and `pair close`. The device words are the same on both. What rmhost has no line for
/// (`sharing`, `allow clipboard`, `set updates off`) is not delivered.
pub fn rmhost_line(words: &[String]) -> Result<String, String> {
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["pair", "on", seconds] => {
            seconds.parse().map(pair_open_line).map_err(|_| format!("{seconds} is not a number of seconds."))
        }
        ["pair", "off"] => Ok(PAIR_CLOSE_LINE.to_string()),
        ["pair", "accept" | "decline", fingerprint] => {
            let fingerprint = ctl::check_fingerprint(fingerprint)?;
            Ok(format!("pair {} {fingerprint}\n", words[1]))
        }
        ["revoke", _] | ["drop"] | ["device", ..] => Ok(words.join(" ") + "\n"),
        _ => Err(format!("rmhost has no line for `{}`.", words.join(" "))),
    }
}

/// Reads one line of rmhost's output. Lines that say nothing the app shows give `None`.
///
/// The lines are those of `serve.m`:
/// `rmhost: remoteman host "NAME" listening: ADDRESS:PORT<tab>FINGERPRINT (address, fingerprint), pairing on|off`,
/// `rmhost: viewer "NAME" WxH at N Hz, decoders ...`, `rmhost: viewer left`,
/// `rmhost: pairing open until UNIX_SECONDS`, `rmhost: paired FINGERPRINT NAME` and
/// `rmhost: pairing pending JSON` and `rmhost: pairing closed: REASON`.
pub fn parse_line(line: &str) -> Option<LogEvent> {
    let line = line.trim();
    if let Some((_, rest)) = line.split_once(" listening: ") {
        let (info, tail) = rest.split_once(" (address, fingerprint), pairing ")?;
        let mut parts = info.split_whitespace();
        let (address, fingerprint) = (parts.next()?, parts.next()?);
        return Some(LogEvent::Listening {
            port: address.rsplit(':').next()?.parse().ok()?,
            fingerprint: fingerprint.to_string(),
            pairing: tail.trim() == "on",
        });
    }
    if line == "rmhost: viewer left" {
        return Some(LogEvent::ViewerLeft);
    }
    if let Some(until) = line.strip_prefix("rmhost: pairing open until ") {
        return Some(LogEvent::PairingOpen { until_secs: until.trim().parse().ok()? });
    }
    if let Some(json) = line.strip_prefix("rmhost: devices ") {
        return serde_json::from_str(json).ok().map(LogEvent::Devices);
    }
    if let Some(json) = line.strip_prefix("rmhost: pairing pending ") {
        return serde_json::from_str(json).ok().map(LogEvent::PairingPending);
    }
    if let Some(reason) = line.strip_prefix("rmhost: pairing closed: ") {
        // A reason this version does not know reads as `other`, as the shared type says.
        let reason = serde_json::from_value(reason.trim().into()).unwrap_or(CloseReason::Other);
        return Some(LogEvent::PairingClosed { reason });
    }
    if let Some(rest) = line.strip_prefix("rmhost: paired ") {
        // "FINGERPRINT NAME": the name is the rest of the line, which may hold spaces.
        let (fingerprint, name) = rest.split_once(' ').unwrap_or((rest, ""));
        let name = name.trim();
        return Some(LogEvent::Paired {
            fingerprint: fingerprint.to_string(),
            name: if name.is_empty() { fingerprint.to_string() } else { name.to_string() },
        });
    }
    let name = line.strip_prefix("rmhost: viewer \"")?;
    Some(LogEvent::Viewer(name.split('"').next()?.to_string()))
}

/// The folder for `machine`'s rmhost files, which `serve_args` names.
pub fn state_folder(machine_folder: &Path) -> PathBuf {
    machine_folder.join(STATE_FOLDER)
}

#[cfg(test)]
mod tests;
