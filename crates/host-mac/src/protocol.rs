// SPDX-License-Identifier: GPL-2.0-or-later
//! The words the app and the agent speak: one JSON object per line, in both directions, the
//! same over the Unix socket and over XPC (which carries each line as a string). Version 1.

use crate::state::Status;
use serde::{Deserialize, Serialize};

/// The protocol's version. A request that names another is refused.
pub const VERSION: u32 = 1;

/// A guest account's password on its way to the answer files. Its debug form hides it, so a log
/// line that prints a request never shows it.
#[derive(Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(transparent)]
pub struct Secret(pub String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(hidden)")
    }
}

/// What the app asks. `id` comes back in the answer, so answers and events can share a line.
#[derive(Deserialize, Serialize, Debug, PartialEq, Eq)]
pub struct Request {
    pub v: u32,
    #[serde(default)]
    pub id: u64,
    #[serde(flatten)]
    pub op: Op,
}

#[derive(Deserialize, Serialize, Debug, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// Every machine the agent knows and where it stands.
    List,
    /// Starts a machine. `qemu` is the QEMU binary from the app's settings; without it the
    /// agent's own `GUESTHOUSE_QEMU` is used.
    Start {
        machine: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        qemu: Option<String>,
        /// The rmhost program, for sharing the screen if the machine allows it once it runs.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rmhost: Option<String>,
        /// The graphics process, for a machine on the guarded path.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pvg_host: Option<String>,
        /// The scale a Linux guest is started with: 2 for Sharp, 1 for Standard. Left out, 2.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scale: Option<u32>,
    },
    /// Asks the guest to power off.
    Shutdown { machine: String },
    /// Quits QEMU, and ends it if it stays.
    ForceOff { machine: String },
    /// Installs a guest in a new machine called `name`: `guest` is `windows` or `ubuntu`. For
    /// Windows `file` is the installer the person chose.
    Install {
        name: String,
        guest: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        qemu: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pvg_host: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        driver_dir: Option<String>,
        /// Sizes the person changed with Customise; left out, the guest's own defaults.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cores: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        memory_mb: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        disk_gb: Option<u64>,
        /// Ubuntu's desktop: `gnome`, `plasma` or `weston`; left out, weston.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        desktop: Option<String>,
        /// The password of the guest's account, made by the app and kept in the Mac's Keychain.
        /// The agent holds it in memory and writes it to the answer files only.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        password: Option<Secret>,
    },
    /// What the person chose at a failure: `try_again`, `choose_installer` (with `path`) or
    /// `open_without_driver`.
    InstallChoose {
        machine: String,
        choice: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        /// The password again, for an agent that has been restarted since the install began.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        password: Option<Secret>,
    },
    /// Where an install stands (the answer has it in `installs`).
    InstallStatus { machine: String },
    /// Stops an install; what was done is kept.
    InstallCancel { machine: String },
    /// Closes an open pairing at once (rmhost's `pair close`); a device waiting is declined.
    PairClose { machine: String },
    /// Trusts the device waiting to be let in (rmhost's `pair accept`).
    PairAccept { machine: String },
    /// Turns the device waiting away (rmhost's `pair decline`).
    PairDecline { machine: String },
    /// Allows or forbids something for a trusted device: `what` is `input` (control; off is
    /// view-only), `clipboard`, `files`, `sound`, `mic` or `camera`.
    DeviceAllow { machine: String, fingerprint: String, what: String, on: bool },
    /// Stops trusting a device; it is disconnected and refused from now on.
    DeviceRevoke { machine: String, fingerprint: String },
    /// Disconnects the viewer now; it stays trusted.
    DeviceDrop { machine: String },
    /// Starts or stops sharing a running machine's screen with other devices.
    Share {
        machine: String,
        on: bool,
        /// Open pairing: let the next unknown device in, for a short time. The share is
        /// started first if it is not there; one that is there is not restarted.
        #[serde(default)]
        pairing: bool,
        /// How long pairing stays open, in seconds; the default is `remote::PAIRING_SECONDS`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pairing_seconds: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rmhost: Option<String>,
    },
}

// The state of a machine's screen sharing is the shared crate's, the same on every host: the
// same field names on the wire, the same states, the same reasons a pairing closed.
pub use guesthouse_remote_view_core::share::{
    CloseReason, DeviceInfo, Pending, Permissions, ShareInfo, ShareState,
};

/// The event line for a share's state: the state as JSON, with the protocol version and
/// `"event": "share"`.
pub fn share_event_line(info: &ShareInfo) -> String {
    let mut value = serde_json::to_value(info).expect("an event always serialises");
    value["v"] = VERSION.into();
    value["event"] = "share".into();
    value.to_string()
}

/// The event line for an install's state, as for a share.
pub fn install_event_line(info: &crate::install::state::InstallInfo) -> String {
    let mut value = serde_json::to_value(info).expect("an event always serialises");
    value["v"] = VERSION.into();
    value["event"] = "install".into();
    value.to_string()
}

/// A machine's state, as in a listing and in a state change.
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
pub struct MachineState {
    /// The machine's folder name.
    pub machine: String,
    pub state: String,
    pub reason: Option<String>,
}

impl MachineState {
    pub fn new(machine: &str, status: &Status) -> MachineState {
        MachineState {
            machine: machine.to_string(),
            state: status.name(),
            reason: status.reason().map(String::from),
        }
    }
}

/// The agent's answer to one request.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Response {
    pub v: u32,
    pub id: u64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// For `list`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machines: Option<Vec<MachineState>>,
    /// For `list`: every machine whose screen is being shared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shares: Option<Vec<ShareInfo>>,
    /// For `list` and `install_status`: the installs, running or kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installs: Option<Vec<crate::install::state::InstallInfo>>,
    /// For `install`: the new machine's folder name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    /// For `list`: which build this agent is (`agent_build::Identity`), left out by an agent
    /// that was not told its build, and by every older one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<crate::agent_build::Identity>,
}

impl Response {
    pub fn ok(id: u64) -> Response {
        Response {
            v: VERSION,
            id,
            ok: true,
            error: None,
            machines: None,
            shares: None,
            installs: None,
            machine: None,
            agent: None,
        }
    }

    pub fn error(id: u64, message: impl Into<String>) -> Response {
        Response {
            v: VERSION,
            id,
            ok: false,
            error: Some(message.into()),
            machines: None,
            shares: None,
            installs: None,
            machine: None,
            agent: None,
        }
    }

    pub fn to_line(&self) -> String {
        serde_json::to_string(self).expect("a response always serialises")
    }
}

/// Sent to everyone connected whenever a machine's state changes.
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct StateChange {
    pub v: u32,
    /// Always "state"; tells an event from an answer, which has `ok` instead.
    pub event: String,
    #[serde(flatten)]
    pub state: MachineState,
}

impl StateChange {
    pub fn line(state: MachineState) -> String {
        let change = StateChange { v: VERSION, event: "state".into(), state };
        serde_json::to_string(&change).expect("an event always serialises")
    }
}

/// Reads one request line. On failure the error carries the id when one could be read, so the
/// answer can still say which request it is for.
pub fn parse_request(line: &str) -> Result<Request, (u64, String)> {
    let id = serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|value| value.get("id").and_then(|id| id.as_u64()))
        .unwrap_or(0);
    let request: Request =
        serde_json::from_str(line).map_err(|error| (id, format!("not a request: {error}")))?;
    if request.v != VERSION {
        return Err((id, format!("protocol version {} is not understood, this is {VERSION}", request.v)));
    }
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_each_request() {
        let list = parse_request(r#"{"v":1,"id":4,"op":"list"}"#).unwrap();
        assert_eq!(list, Request { v: 1, id: 4, op: Op::List });
        let start = parse_request(r#"{"v":1,"id":5,"op":"start","machine":"win","qemu":"/q"}"#);
        assert_eq!(
            start.unwrap().op,
            Op::Start {
                machine: "win".into(),
                qemu: Some("/q".into()),
                rmhost: None,
                pvg_host: None,
                scale: None,
            }
        );
        let start = parse_request(r#"{"v":1,"op":"start","machine":"win"}"#).unwrap();
        assert_eq!(
            start.op,
            Op::Start { machine: "win".into(), qemu: None, rmhost: None, pvg_host: None, scale: None }
        );
        assert_eq!(
            parse_request(r#"{"v":1,"id":1,"op":"shutdown","machine":"w"}"#).unwrap().op,
            Op::Shutdown { machine: "w".into() }
        );
        assert_eq!(
            parse_request(r#"{"v":1,"id":1,"op":"force_off","machine":"w"}"#).unwrap().op,
            Op::ForceOff { machine: "w".into() }
        );
    }

    #[test]
    fn a_start_may_carry_the_scale() {
        let start = parse_request(r#"{"v":1,"op":"start","machine":"deb","scale":1}"#).unwrap();
        assert!(matches!(start.op, Op::Start { scale: Some(1), .. }));
        let request = Request {
            v: VERSION,
            id: 8,
            op: Op::Start { machine: "deb".into(), qemu: None, rmhost: None, pvg_host: None, scale: Some(2) },
        };
        let text = serde_json::to_string(&request).unwrap();
        assert_eq!(text, r#"{"v":1,"id":8,"op":"start","machine":"deb","scale":2}"#);
    }

    #[test]
    fn refuses_other_versions_and_nonsense_but_keeps_the_id() {
        let (id, why) = parse_request(r#"{"v":2,"id":9,"op":"list"}"#).unwrap_err();
        assert_eq!(id, 9);
        assert!(why.contains("version 2"));
        let (id, _) = parse_request(r#"{"v":1,"id":3,"op":"explode"}"#).unwrap_err();
        assert_eq!(id, 3);
        let (id, _) = parse_request("not json").unwrap_err();
        assert_eq!(id, 0);
        assert!(parse_request(r#"{"v":1,"id":3,"op":"start"}"#).is_err());
    }

    #[test]
    fn a_request_round_trips() {
        let request = Request {
            v: VERSION,
            id: 7,
            op: Op::Start { machine: "w".into(), qemu: None, rmhost: None, pvg_host: None, scale: None },
        };
        let text = serde_json::to_string(&request).unwrap();
        assert_eq!(text, r#"{"v":1,"id":7,"op":"start","machine":"w"}"#);
        assert_eq!(parse_request(&text).unwrap(), request);
    }

    #[test]
    fn answers_and_events_look_as_documented() {
        assert_eq!(Response::ok(2).to_line(), r#"{"v":1,"id":2,"ok":true}"#);
        assert_eq!(
            Response::error(2, "no such machine").to_line(),
            r#"{"v":1,"id":2,"ok":false,"error":"no such machine"}"#
        );
        let state = MachineState::new("win", &Status::Failed { reason: "bad disk".into() });
        assert_eq!(
            StateChange::line(state),
            r#"{"v":1,"event":"state","machine":"win","state":"failed","reason":"bad disk"}"#
        );
        let state = MachineState::new("win", &Status::ShuttingDown { stalled: true });
        let line = StateChange::line(state);
        assert!(line.contains(r#""state":"stopping_stalled","reason":null"#));
    }

    #[test]
    fn a_share_request_and_its_events_read_and_write() {
        let request = parse_request(
            r#"{"v":1,"id":2,"op":"share","machine":"w","on":true,"pairing":true,"pairing_seconds":30,"rmhost":"/r"}"#,
        )
        .unwrap();
        assert_eq!(
            request.op,
            Op::Share {
                machine: "w".into(),
                on: true,
                pairing: true,
                pairing_seconds: Some(30),
                rmhost: Some("/r".into())
            }
        );
        // Pairing is off unless asked for.
        let plain = parse_request(r#"{"v":1,"op":"share","machine":"w","on":false}"#).unwrap();
        assert_eq!(
            plain.op,
            Op::Share { machine: "w".into(), on: false, pairing: false, pairing_seconds: None, rmhost: None }
        );
        for (op, want) in [("pair_close", "close"), ("pair_accept", "accept"), ("pair_decline", "decline")] {
            let line = format!(r#"{{"v":1,"id":3,"op":"{op}","machine":"w"}}"#);
            let got = parse_request(&line).unwrap().op;
            let name = format!("{got:?}").to_lowercase();
            assert!(name.contains(want), "{name}");
        }
        let line = share_event_line(&ShareInfo::off("w"));
        assert!(line.contains(r#""event":"share""#) && line.contains(r#""state":"off""#));
    }

    #[test]
    fn a_listing_carries_every_machine() {
        let mut response = Response::ok(1);
        response.machines = Some(vec![MachineState::new("a", &Status::Running)]);
        let back: Response = serde_json::from_str(&response.to_line()).unwrap();
        assert_eq!(back.machines.unwrap()[0].state, "running");
    }
}
