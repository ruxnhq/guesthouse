//! A machine's network side: open or closed, announced while it listens, and what it says about itself.

use crate::control::{RelayClient, RelayReply, RelayStatus, Request, ShareState};
use crate::relay::{Config, Relay};
use guesthouse_remote_view_core::announce::{Announcement, Announcer};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

/// Where the network side listens when a request names no address: every network of this PC.
const ANY_ADDRESS: IpAddr = IpAddr::V4(Ipv4Addr::UNSPECIFIED);

/// What a network side starts with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanSettings {
    /// The machine's id, which only names it in what the network side says.
    pub machine: String,
    /// The guest's port: QEMU's forward on loopback on the Linux host, or the guest's own address.
    pub upstream: SocketAddr,
    /// Where to listen on the network from the start. Without it the network side starts closed, and
    /// listens once asked to.
    pub listen: Option<SocketAddr>,
    /// The relay's own when left out.
    pub idle_timeout: Option<Duration>,
    pub max_clients: Option<usize>,
    /// The name the machine is announced by on the network, which viewers show. Left out: its id.
    pub name: Option<String>,
    /// The guest host's fingerprint. Nothing is announced until it is known.
    pub fingerprint: Option<String>,
}

impl LanSettings {
    /// Starting closed, with the relay's own limits and nothing to announce yet.
    pub fn new(machine: impl Into<String>, upstream: SocketAddr) -> Self {
        Self {
            machine: machine.into(),
            upstream,
            listen: None,
            idle_timeout: None,
            max_clients: None,
            name: None,
            fingerprint: None,
        }
    }
}

pub struct Lan {
    machine: String,
    upstream: SocketAddr,
    listen: Option<SocketAddr>,
    config: LanSettings,
    relay: Option<Relay>,
    /// Why the last attempt to open did not work. Cleared by the next open or by a close.
    failure: Option<String>,
    /// The name and fingerprint the machine is announced with, once both are known.
    name: String,
    fingerprint: Option<String>,
    announcer: Option<Box<dyn Announcer + Send>>,
    /// Why the announcement could not be made. The relay works without it.
    announce_failure: Option<String>,
}

/// An announcer that says nothing, for a trial run or a test.
struct Quiet;

impl Announcer for Quiet {
    fn announce(&mut self, _: &Announcement) -> Result<(), String> {
        Ok(())
    }

    fn withdraw(&mut self) {}
}

impl Lan {
    /// A closed network side, or one that opens at once if the settings have an address to listen on.
    pub fn new(settings: &LanSettings) -> Self {
        Self::with_announcer(settings, Box::new(Quiet))
    }

    /// As [`Lan::new`], announcing the machine with `announcer` while it listens. The announcement
    /// goes out each time the network side opens or moves, and is taken back when it closes, so it
    /// never outlives the port.
    pub fn with_announcer(settings: &LanSettings, announcer: Box<dyn Announcer + Send>) -> Self {
        let mut lan = Self {
            machine: settings.machine.clone(),
            upstream: settings.upstream,
            listen: settings.listen,
            config: settings.clone(),
            relay: None,
            failure: None,
            name: settings.name.clone().unwrap_or_else(|| settings.machine.clone()),
            fingerprint: settings.fingerprint.clone(),
            announcer: Some(announcer),
            announce_failure: None,
        };
        if let Some(listen) = settings.listen {
            let _ = lan.open(Some(listen.ip()), Some(listen.port()));
        }
        lan
    }

    /// Starts listening, or moves to the address and port asked for. Asking for what it already does
    /// changes nothing, and port 0 means whatever port it has, or any free one.
    pub fn open(&mut self, address: Option<IpAddr>, port: Option<u16>) -> Result<(), String> {
        let wanted = SocketAddr::new(
            address.or(self.listen.map(|listen| listen.ip())).unwrap_or(ANY_ADDRESS),
            port.or(self.listen.map(|listen| listen.port())).ok_or("a port is needed to open")?,
        );
        if let Some(relay) = &self.relay {
            let have = relay.local_addr();
            if have.ip() == wanted.ip() && (wanted.port() == 0 || have.port() == wanted.port()) {
                return Ok(());
            }
        }
        // The old one goes first: the new address may be the same port on another interface.
        self.relay = None;
        let mut config = Config::new(wanted, self.upstream);
        if let Some(idle) = self.config.idle_timeout {
            config.idle_timeout = idle;
        }
        if let Some(most) = self.config.max_clients {
            config.max_clients = most;
        }
        match Relay::start(config) {
            Ok(relay) => {
                self.relay = Some(relay);
                self.failure = None;
                self.announce();
                Ok(())
            }
            Err(error) => {
                let reason = format!("{wanted} could not be opened: {error}");
                self.failure = Some(reason.clone());
                Err(reason)
            }
        }
    }

    pub fn close(&mut self) {
        self.withdraw();
        self.relay = None;
        self.failure = None;
    }

    /// Announces what listens now, under the name and fingerprint known now.
    fn announce(&mut self) {
        let (Some(relay), Some(fingerprint), Some(announcer)) =
            (&self.relay, &self.fingerprint, &mut self.announcer)
        else {
            return;
        };
        let local = relay.local_addr();
        let address = (!local.ip().is_unspecified()).then_some(local.ip());
        self.announce_failure = Announcement::new(&self.name, local.port(), fingerprint, address)
            .map_err(|error| error.to_string())
            .and_then(|announcement| announcer.announce(&announcement))
            .err();
    }

    fn withdraw(&mut self) {
        if let Some(announcer) = &mut self.announcer {
            announcer.withdraw();
        }
        self.announce_failure = None;
    }

    /// Takes a new name or fingerprint to announce by, and says whether either changed.
    fn identity(&mut self, name: Option<String>, fingerprint: Option<String>) -> bool {
        let mut changed = false;
        if let Some(name) = name.filter(|name| *name != self.name) {
            self.name = name;
            changed = true;
        }
        if let Some(fingerprint) =
            fingerprint.filter(|fingerprint| Some(fingerprint) != self.fingerprint.as_ref())
        {
            self.fingerprint = Some(fingerprint);
            changed = true;
        }
        changed
    }

    pub fn status(&self) -> RelayStatus {
        let local = self.relay.as_ref().map(Relay::local_addr);
        let (state, reason) = match (&self.relay, &self.failure) {
            (Some(_), _) => (ShareState::Listening, self.announce_failure.clone()),
            (None, Some(reason)) => (ShareState::Failed, Some(reason.clone())),
            (None, None) => (ShareState::Off, None),
        };
        RelayStatus {
            machine: self.machine.clone(),
            state,
            address: local.map(|address| address.ip()),
            port: local.map(|address| address.port()),
            loopback_port: self.upstream.port(),
            reason,
            counters: self.relay.as_ref().map(Relay::counters).unwrap_or_default(),
            clients: self
                .relay
                .as_ref()
                .map(|relay| relay.clients().iter().map(RelayClient::from).collect())
                .unwrap_or_default(),
        }
    }

    pub fn handle(&mut self, request: Request) -> RelayReply {
        match request {
            Request::Status => {}
            Request::Open { address, port, name, fingerprint } => {
                let before = self.relay.as_ref().map(Relay::local_addr);
                let changed = self.identity(name, fingerprint);
                if let Err(reason) = self.open(address, port) {
                    return RelayReply::Error(reason);
                }
                // An open that moved nothing announced nothing: a new name or fingerprint still does.
                if changed && before == self.relay.as_ref().map(Relay::local_addr) {
                    self.announce();
                }
            }
            Request::Close => self.close(),
            // The card's clock is the Linux host's helper's, which takes this before it comes here.
            Request::StreamClocks { .. } => {}
        }
        RelayReply::Status(self.status())
    }

    /// Stops listening and waits for the relay's threads, so the port is free when this returns.
    pub fn stop(&mut self) {
        self.withdraw();
        self.relay = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::UdpSocket;
    use std::time::{Duration, Instant};

    fn settings(loopback_port: u16, listen: Option<SocketAddr>) -> LanSettings {
        LanSettings {
            listen,
            idle_timeout: Some(Duration::from_secs(5)),
            ..LanSettings::new("test", SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), loopback_port))
        }
    }

    fn forward() -> UdpSocket {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        socket
    }

    #[test]
    fn it_starts_closed_and_says_so() {
        let lan = Lan::new(&settings(19065, None));
        let status = lan.status();
        assert_eq!((status.state, status.port, status.loopback_port), (ShareState::Off, None, 19065));
        assert!(status.clients.is_empty());
    }

    #[test]
    fn opening_and_closing_move_the_network_side_while_the_program_runs() {
        let forward = forward();
        let mut lan = Lan::new(&settings(forward.local_addr().unwrap().port(), None));
        let RelayReply::Status(opened) = lan.handle(Request::Open {
            address: Some("127.0.0.1".parse().unwrap()),
            port: Some(0),
            name: None,
            fingerprint: None,
        }) else {
            panic!("opening failed")
        };
        assert_eq!(opened.state, ShareState::Listening);
        let port = opened.port.unwrap();
        let device = UdpSocket::bind("127.0.0.1:0").unwrap();
        device.send_to(b"input", ("127.0.0.1", port)).unwrap();
        let mut buffer = [0u8; 16];
        assert_eq!(forward.recv_from(&mut buffer).unwrap().0, 5);
        assert_eq!(lan.status().clients.len(), 1);
        // The relay counts a datagram once its send has returned, which can be just after it arrives.
        let deadline = Instant::now() + Duration::from_secs(3);
        while lan.status().counters.up_packets == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(lan.status().counters.up_packets, 1);

        let RelayReply::Status(closed) = lan.handle(Request::Close) else { panic!("closing failed") };
        assert_eq!((closed.state, closed.port), (ShareState::Off, None));
        // The relay lets go of its socket once its thread sees the close, which can be just after.
        let deadline = Instant::now() + Duration::from_secs(3);
        while UdpSocket::bind(("127.0.0.1", port)).is_err() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        UdpSocket::bind(("127.0.0.1", port)).expect("closing gives the port back");
    }

    #[test]
    fn opening_what_is_already_open_changes_nothing() {
        let mut lan = Lan::new(&settings(19065, Some("127.0.0.1:0".parse().unwrap())));
        let first = lan.status().port.unwrap();
        lan.open(None, Some(0)).unwrap();
        lan.open(Some("127.0.0.1".parse().unwrap()), Some(first)).unwrap();
        assert_eq!(lan.status().port, Some(first));
    }

    #[test]
    fn a_port_that_cannot_be_opened_is_a_failure_with_a_reason_and_closing_clears_it() {
        let taken = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = taken.local_addr().unwrap().port();
        let mut lan = Lan::new(&settings(19065, None));
        let RelayReply::Error(reason) = lan.handle(Request::Open {
            address: Some("127.0.0.1".parse().unwrap()),
            port: Some(port),
            name: None,
            fingerprint: None,
        }) else {
            panic!("a taken port opened")
        };
        assert!(reason.contains(&port.to_string()));
        let status = lan.status();
        assert_eq!(status.state, ShareState::Failed);
        assert_eq!(status.reason.as_deref(), Some(reason.as_str()));
        lan.close();
        assert_eq!(lan.status().state, ShareState::Off);
    }

    #[test]
    fn opening_with_no_port_anywhere_is_refused() {
        let mut lan = Lan::new(&settings(19065, None));
        assert!(matches!(
            lan.handle(Request::Open { address: None, port: None, name: None, fingerprint: None }),
            RelayReply::Error(_)
        ));
    }

    use std::sync::{Arc, Mutex};

    /// Keeps what was announced, where the test can see it.
    #[derive(Clone, Default)]
    struct Seen(Arc<Mutex<Vec<Option<Announcement>>>>);

    impl Announcer for Seen {
        fn announce(&mut self, announcement: &Announcement) -> Result<(), String> {
            self.0.lock().unwrap().push(Some(announcement.clone()));
            Ok(())
        }

        fn withdraw(&mut self) {
            self.0.lock().unwrap().push(None);
        }
    }

    #[test]
    fn the_announcement_comes_with_the_fingerprint_and_goes_with_the_port() {
        let seen = Seen::default();
        let mut lan = Lan::with_announcer(&settings(19065, None), Box::new(seen.clone()));
        let open = |fingerprint: Option<String>| Request::Open {
            address: Some("127.0.0.1".parse().unwrap()),
            port: Some(0),
            name: Some("Windows 11".into()),
            fingerprint,
        };
        lan.handle(open(None));
        assert!(seen.0.lock().unwrap().iter().all(Option::is_none), "no fingerprint, nothing announced");
        lan.handle(open(Some("ab".repeat(32))));
        let port = lan.status().port.unwrap();
        let last =
            seen.0.lock().unwrap().last().cloned().flatten().expect("announced once the fingerprint came");
        assert_eq!(
            (last.instance.as_str(), last.port, last.fingerprint.as_str()),
            ("Windows 11", port, "abababababababab")
        );
        assert_eq!(last.address, Some("127.0.0.1".parse().unwrap()));
        lan.handle(Request::Close);
        assert_eq!(seen.0.lock().unwrap().last(), Some(&None), "closing withdraws it");
        lan.handle(open(None));
        assert!(seen.0.lock().unwrap().last().unwrap().is_some(), "opening again announces again");
        lan.stop();
        assert_eq!(seen.0.lock().unwrap().last(), Some(&None), "stopping withdraws it");
    }

    #[test]
    fn a_listen_address_in_the_settings_opens_at_the_start() {
        let lan = Lan::new(&settings(19065, Some("127.0.0.1:0".parse().unwrap())));
        assert_eq!(lan.status().state, ShareState::Listening);
    }

    #[test]
    #[cfg_attr(windows, ignore = "listens on this PC's network, which Windows' firewall may ask about")]
    fn a_guest_with_an_address_of_its_own_is_reached_from_a_socket_on_any_network() {
        // This PC's own address stands in for a guest's: what matters is that it is not loopback.
        let Some(address) = crate::lan_address() else {
            eprintln!("skipped: this PC has no address on a network");
            return;
        };
        let guest = UdpSocket::bind((address, 0)).unwrap();
        guest.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let upstream = guest.local_addr().unwrap();
        let mut lan = Lan::new(&LanSettings {
            listen: Some("127.0.0.1:0".parse().unwrap()),
            ..LanSettings::new("test", upstream)
        });
        let status = lan.status();
        assert_eq!((status.state, status.loopback_port), (ShareState::Listening, upstream.port()));
        let device = forward();
        device.send_to(b"input", ("127.0.0.1", status.port.unwrap())).unwrap();
        let mut buffer = [0u8; 16];
        let (length, from) = guest.recv_from(&mut buffer).unwrap();
        assert_eq!(&buffer[..length], b"input");
        assert!(!from.ip().is_loopback(), "the device's socket is not on loopback: {from}");
        // Something else on the network finds the device's socket first.
        UdpSocket::bind((address, 0)).unwrap().send_to(b"stranger", from).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while lan.status().counters.refused_stranger == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(lan.status().counters.refused_stranger, 1);
        guest.send_to(b"video", from).unwrap();
        let (length, _) = device.recv_from(&mut buffer).unwrap();
        assert_eq!(&buffer[..length], b"video", "only the guest's answer reached the device");
        lan.stop();
    }
}
