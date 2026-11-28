//! A machine's remote view on the network, announced through Windows' own mDNS responder: the
//! shared [`Announcer`] over `hcs`'s DNS-SD calls, for the service to hand to a machine's network
//! side.
//!
//! The instance is the machine's name, on the relay's port, with the records the shared
//! announcement carries, so a viewer lists it beside this PC's own remoteman and tells them apart by
//! name and fingerprint. Its records point at this PC's own host name, the only one Windows'
//! responder answers for, and each network gets this PC's address on it. So the announcement's
//! one address, and the per-machine host name the Linux host gives, have no place here.
//!
//! Windows answers on every network this PC is on: it was seen answering on a Hyper-V virtual
//! switch, with this PC's address there. Nothing narrows that, as the Linux host's interface names
//! do, so guests on this PC's virtual networks may list the machine too.

use guesthouse_hcs::dns_sd::Service;
use guesthouse_remote_view_core::announce::{Announcement, SERVICE_TYPE};

/// What Windows is asked to announce for `announcement`.
pub fn service(announcement: &Announcement) -> Service {
    Service {
        instance: announcement.instance.clone(),
        kind: SERVICE_TYPE.to_owned(),
        port: announcement.port,
        txt: announcement.txt().into_iter().map(|(key, value)| (key.to_owned(), value)).collect(),
    }
}

#[cfg(windows)]
pub use self::windows::DnsSd;

#[cfg(windows)]
mod windows {
    use guesthouse_hcs::dns_sd::Registration;
    use guesthouse_remote_view_core::announce::{Announcement, Announcer};

    /// One machine's announcement, through Windows' responder. It is taken back when withdrawn or
    /// dropped, and each machine has its own.
    #[derive(Default)]
    pub struct DnsSd {
        now: Option<Registration>,
    }

    impl DnsSd {
        /// Announcing nothing until asked. Give it to `Lan::with_announcer`.
        pub fn new() -> Self {
            Self::default()
        }
    }

    impl Announcer for DnsSd {
        /// Waits for Windows to say how it went, which took 0.8 s when tried, so that a name taken
        /// already is an error here and not a silent `NAME(1)`.
        fn announce(&mut self, announcement: &Announcement) -> Result<(), String> {
            // The old one goes first, so the same name can be registered again.
            self.withdraw();
            let registration = Registration::register(&super::service(announcement))
                .map_err(|failure| format!("The machine isn't announced on the network: {failure}."))?;
            self.now = Some(registration);
            Ok(())
        }

        fn withdraw(&mut self) {
            // Dropping it takes it back, and waits a little for Windows to say so.
            self.now = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_machine_is_announced_by_its_name_on_the_relays_port_with_the_shared_records() {
        let fingerprint = "AB12CD34EF56AB78901234567890ABCDEF1234567890ABCDEF1234567890ABCD";
        let announcement = Announcement::new("Windows 11", 7792, fingerprint, None).unwrap();
        let service = service(&announcement);
        assert_eq!(service.full_name(), "Windows 11._remoteman._udp.local");
        assert_eq!(service.port, 7792);
        let txt: Vec<(&str, &str)> = service.txt.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(txt, [("v", "0"), ("fp", "ab12cd34ef56ab78"), ("os", "windows")]);
    }
}
