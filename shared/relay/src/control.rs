//! What a host asks a machine's network side, and what it answers, as JSON. The Linux host's app sends
//! these to its helper as lines on a Unix socket, which is the app's own, so this wording is what an
//! older helper still running after an update understands.

pub use guesthouse_remote_view_core::share::ShareState;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

use crate::{ClientInfo, Counters};

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
    /// Holds this NVIDIA card's memory clock up while a viewer streams, by its PCI address, or with
    /// none holds nothing from now on: the machine's setting, changed while it runs.
    StreamClocks {
        #[serde(default)]
        card: Option<String>,
    },
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
/// The network side's own state. Not the screen's [`ShareInfo`]: this is only the network side, which
/// is `off`, `listening` or `failed` (`starting` is the app's word for a machine that is not up yet,
/// which the network side never says).
///
/// [`ShareInfo`]: guesthouse_remote_view_core::share::ShareInfo
pub struct RelayStatus {
    pub machine: String,
    pub state: ShareState,
    /// The address and port devices on the network send to, when listening.
    pub address: Option<IpAddr>,
    pub port: Option<u16>,
    /// The guest's port the datagrams go to: QEMU's loopback forward on the Linux host, whose name it
    /// keeps on the wire.
    pub loopback_port: u16,
    pub reason: Option<String>,
    /// Totals since the network side was last opened.
    pub counters: Counters,
    pub clients: Vec<RelayClient>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayClient {
    pub address: String,
    pub idle_ms: u64,
    pub up_packets: u64,
    pub down_packets: u64,
}

impl From<&ClientInfo> for RelayClient {
    fn from(from: &ClientInfo) -> Self {
        Self {
            address: from.address.to_string(),
            idle_ms: from.idle.as_millis() as u64,
            up_packets: from.up_packets,
            down_packets: from.down_packets,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_short_lines_with_an_op() {
        let text = |request: &Request| serde_json::to_string(request).unwrap();
        assert_eq!(text(&Request::Status), r#"{"op":"status"}"#);
        assert_eq!(text(&Request::Close), r#"{"op":"close"}"#);
        assert_eq!(
            text(&Request::StreamClocks { card: Some("0000:01:00.0".into()) }),
            r#"{"op":"stream_clocks","card":"0000:01:00.0"}"#
        );
        assert_eq!(
            serde_json::from_str::<Request>(r#"{"op":"stream_clocks"}"#).unwrap(),
            Request::StreamClocks { card: None }
        );
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
    fn counters_keep_their_names_on_the_wire() {
        assert_eq!(
            serde_json::to_string(&Counters::default()).unwrap(),
            r#"{"up_packets":0,"up_bytes":0,"down_packets":0,"down_bytes":0,"clients_started":0,"clients_expired":0,"refused_full":0,"refused_stranger":0,"errors":0}"#
        );
    }

    #[test]
    fn counters_from_a_relay_that_did_not_count_strangers_read_as_none() {
        let older = r#"{"up_packets":1,"up_bytes":2,"down_packets":3,"down_bytes":4,"clients_started":5,"clients_expired":6,"refused_full":7,"errors":8}"#;
        let counters: Counters = serde_json::from_str(older).unwrap();
        assert_eq!((counters.refused_full, counters.refused_stranger, counters.errors), (7, 0, 8));
    }
}
