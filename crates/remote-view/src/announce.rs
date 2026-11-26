//! Being found: what a machine's remote view says about itself on the network, and the narrow trait
//! whatever says it must meet.
//!
//! Remoteman's viewers browse DNS-SD for `_remoteman._udp` and connect to the port in the answer. A guest
//! behind QEMU's user network or passt never reaches the network with its own announcement, so this
//! program announces for it: the machine's name, the relay's port, and the first 16 hex digits of the
//! guest's certificate fingerprint, so a device that paired with it recognises it. The handshake still
//! checks the whole key.
//!
//! The relay helper on a Linux host implements [`Announcer`] with an mDNS responder of its own and
//! calls it as its network side opens and closes, so that an announcement never outlives its port.
//! The records are shaped as remoteman's own host announces itself: `v=0`, `fp`, `os`.

use std::fmt;
use std::net::IpAddr;

/// The DNS-SD type remoteman's viewers browse for.
pub const SERVICE_TYPE: &str = "_remoteman._udp";
/// How many hex digits of the fingerprint go on the network.
pub const FINGERPRINT_DIGITS: usize = 16;
/// A DNS label, which the instance name is, holds at most this many bytes.
const LONGEST_NAME: usize = 63;
/// The guests this serves are Windows ones.
const OS: &str = "windows";
/// The form of the records, which a viewer does not read yet.
const VERSION: &str = "0";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BadAnnouncement {
    EmptyName,
    NameTooLong,
    /// The fingerprint has to be hex and at least [`FINGERPRINT_DIGITS`] digits.
    Fingerprint,
}

impl fmt::Display for BadAnnouncement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::EmptyName => "the machine has no name to announce",
            Self::NameTooLong => "the machine's name is too long for a DNS-SD instance",
            Self::Fingerprint => "the fingerprint is not hex, or has too few digits",
        })
    }
}

/// What one machine announces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announcement {
    /// The instance's name, which viewers show: the machine's name.
    pub instance: String,
    /// The port on the network, which is the relay's.
    pub port: u16,
    /// The first [`FINGERPRINT_DIGITS`] hex digits of the guest's fingerprint, lower case.
    pub fingerprint: String,
    /// The one address to announce, when the relay listens on one. Otherwise the announcer picks the
    /// addresses of this PC's private networks.
    pub address: Option<IpAddr>,
}

impl Announcement {
    /// Checks the name and takes the first digits of the whole fingerprint.
    pub fn new(
        instance: &str,
        port: u16,
        fingerprint: &str,
        address: Option<IpAddr>,
    ) -> Result<Self, BadAnnouncement> {
        if instance.is_empty() {
            return Err(BadAnnouncement::EmptyName);
        }
        if instance.len() > LONGEST_NAME {
            return Err(BadAnnouncement::NameTooLong);
        }
        let digits = fingerprint
            .get(..FINGERPRINT_DIGITS)
            .filter(|digits| digits.bytes().all(|b| b.is_ascii_hexdigit()));
        let fingerprint = digits.ok_or(BadAnnouncement::Fingerprint)?.to_ascii_lowercase();
        Ok(Self { instance: instance.to_owned(), port, fingerprint, address })
    }

    /// The TXT records. There is no `mac`: it only wakes a computer on the viewer's own network, and a
    /// guest behind NAT is not on it.
    pub fn txt(&self) -> Vec<(&'static str, String)> {
        vec![("v", VERSION.to_owned()), ("fp", self.fingerprint.clone()), ("os", OS.to_owned())]
    }

    /// The host name the records point at: `guesthouse-NAME.local.`, with NAME made a DNS label.
    pub fn host_name(&self) -> String {
        host_name(&self.instance)
    }
}

/// `guesthouse-NAME.local.` for a machine, with every character a DNS label cannot hold made a `-`,
/// in lower case, and no longer than a label may be.
pub fn host_name(machine: &str) -> String {
    let label: String = machine
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect();
    let label = format!("guesthouse-{}", label.trim_matches('-'));
    let label = label[..label.len().min(LONGEST_NAME)].trim_end_matches('-');
    format!("{label}.local.")
}

/// Whether an announcement goes out on a network interface of this name. Only real networks: not
/// loopback, and not the PC's own bridges, containers and tunnels, where nobody looks for a screen.
pub fn lan_interface(name: &str) -> bool {
    const NOT: [&str; 9] = ["lo", "docker", "virbr", "veth", "tailscale", "wg", "tun", "br-", "vnet"];
    !name.is_empty() && !NOT.iter().any(|start| name.starts_with(start))
}

/// Puts an announcement on the network and takes it back. One announcer holds one announcement.
pub trait Announcer {
    /// Announces, replacing what it announced before. An error means nothing is announced, and the
    /// relay still works: a device can be given the address by hand.
    fn announce(&mut self, announcement: &Announcement) -> Result<(), String>;

    /// Takes the announcement back, so the instance goes from viewers' lists at once and not when its
    /// records run out. Does nothing when nothing is announced.
    fn withdraw(&mut self);
}

#[cfg(test)]
mod tests {
    use super::*;

    const FINGERPRINT: &str = "AB12CD34EF56AB78901234567890ABCDEF1234567890ABCDEF1234567890ABCD";

    #[test]
    fn the_records_are_the_first_sixteen_digits_in_lower_case_the_os_and_the_form() {
        let announcement = Announcement::new("Windows 11", 7800, FINGERPRINT, None).unwrap();
        assert_eq!(
            announcement.txt(),
            [("v", "0".to_owned()), ("fp", "ab12cd34ef56ab78".to_owned()), ("os", "windows".to_owned())]
        );
        assert!(announcement.txt().iter().all(|(key, _)| *key != "mac"));
    }

    #[test]
    fn a_name_or_fingerprint_that_could_not_be_announced_is_refused() {
        let new = |name: &str, fingerprint: &str| Announcement::new(name, 7800, fingerprint, None);
        assert_eq!(new("", FINGERPRINT), Err(BadAnnouncement::EmptyName));
        assert_eq!(new(&"x".repeat(64), FINGERPRINT), Err(BadAnnouncement::NameTooLong));
        assert!(new(&"x".repeat(63), FINGERPRINT).is_ok());
        assert_eq!(new("m", "ab12"), Err(BadAnnouncement::Fingerprint));
        assert_eq!(new("m", "zz12cd34ef56ab78"), Err(BadAnnouncement::Fingerprint));
        assert_eq!(
            new("m", "ab12cd34ef56ab7é"),
            Err(BadAnnouncement::Fingerprint),
            "no split inside a character"
        );
    }

    #[test]
    fn the_host_name_is_a_dns_label_under_local() {
        assert_eq!(host_name("Windows 11"), "guesthouse-windows-11.local.");
        assert_eq!(host_name("win11"), "guesthouse-win11.local.");
        assert!(host_name(&"x".repeat(80)).len() <= 63 + ".local.".len());
    }

    #[test]
    fn only_real_networks_are_announced_on() {
        for name in ["eno2", "eth0", "wlp3s0", "enp5s0"] {
            assert!(lan_interface(name), "{name}");
        }
        for name in ["lo", "docker0", "virbr0", "veth12ab", "tailscale0", "wg0", "tun0", "br-1234", ""] {
            assert!(!lan_interface(name), "{name}");
        }
    }

    /// What a real announcer is held to: a second announcement replaces the first, and withdrawing
    /// twice is harmless.
    #[derive(Default)]
    struct Recorder {
        now: Option<Announcement>,
    }

    impl Announcer for Recorder {
        fn announce(&mut self, announcement: &Announcement) -> Result<(), String> {
            self.now = Some(announcement.clone());
            Ok(())
        }

        fn withdraw(&mut self) {
            self.now = None;
        }
    }

    #[test]
    fn an_announcer_holds_one_announcement_and_gives_it_back() {
        let mut announcer = Recorder::default();
        let first = Announcement::new("a", 7800, FINGERPRINT, None).unwrap();
        let second = Announcement::new("a", 7801, FINGERPRINT, None).unwrap();
        announcer.announce(&first).unwrap();
        announcer.announce(&second).unwrap();
        assert_eq!(announcer.now, Some(second));
        announcer.withdraw();
        announcer.withdraw();
        assert_eq!(announcer.now, None);
    }
}
