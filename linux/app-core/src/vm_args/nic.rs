//! The guest's network, as QEMU's words.

use super::arguments::words;
use super::passt_age::{PROBE_DNS_HOST, PasstAge};
use super::{PASST_FD, SHARE_ADDRESS, TAP_FD, VmPlan};
use crate::network::{
    self, CONTROL_MAC, GUEST_AGENT_PORT, GUEST_APP_WINDOWS_PORT, GUEST_FILES_PORT, Mode, bridge_helper,
};
use crate::text::{qemu_value, shell_quote};
use guesthouse_remote_view_core::PORT as REMOTEMAN_PORT;

/// Passt copies the host's address by default, and these can't clash with the private link. Its
/// default MTU is more than an e1000e takes. QEMU's passt network and passt itself take the same names.
const PASST_NETWORK: [(&str, &str); 4] =
    [("mtu", "1500"), ("address", "10.0.3.15"), ("netmask", "255.255.255.0"), ("gateway", "10.0.3.2")];
/// The guest has no way to the host but DNS, through this address.
const PASST_DNS: &str = "10.0.3.3";
/// The same as `--map-host-loopback none` for a passt that doesn't take `none`. QEMU's passt network
/// has no name for it, only its list of passt's own words.
const NO_MAP_GW: &str = "no-map-gw";
const QEMU_PASST_WORD: &str = "param";
/// passt tells nothing but what goes wrong.
const PASST_QUIET: &str = "--quiet";
/// passt's words for sending out through one interface. IPv4 is bound whatever the interface has.
const PASST_OUT_IF4: &str = "--outbound-if4";
const PASST_OUT_IF6: &str = "--outbound-if6";
/// For an interface with no IPv6 address, where binding IPv6 to it makes passt refuse to start and
/// leaving it unbound would let IPv6 out the normal link.
const PASST_IPV4_ONLY: &str = "--ipv4-only";
/// Where the jail's launcher notes passt's pid and start time, in the run folder, so it can be found
/// like QEMU.
pub const PASST_PID_FILE: &str = "passt.pid";
const CARD: &str = "e1000e";
const MACOS_CARD: &str = "virtio-net-pci";
const MACOS_CARD_SLOT: &str = "0x3";
const QEMU_NAT: &str = "user";
/// QEMU's own NAT with the host's loopback out of the guest's reach: 10.0.2.2 and the IPv6 prefix
/// lead to no service on the host's loopback, and forwards from the host still arrive from 10.0.2.2.
/// The word is in `qemu/qemu-host-loopback.patch`. A restricted network sends nothing the guest
/// starts out of QEMU, so it goes without.
const NO_HOST_LOOPBACK: &str = "host-loopback=off";

/// One of passt's settings: a name and its value, or a switch.
enum PasstSetting {
    Value(&'static str, String),
    Switch(&'static str),
}

impl PasstSetting {
    fn passt_words(&self) -> Vec<String> {
        match self {
            Self::Value(name, value) => vec![format!("--{name}"), value.clone()],
            Self::Switch(name) => vec![format!("--{name}")],
        }
    }

    fn qemu_word(&self) -> String {
        match self {
            Self::Value(name, value) => format!("{name}={value}"),
            Self::Switch(name) => format!("{QEMU_PASST_WORD}=--{name}"),
        }
    }
}

/// passt's settings for a passt of this age, with the host's resolver if it has one. For
/// [`PasstAge::NoDnsHost`] that has to be one off the PC, which the guest is told of and asks itself.
fn passt_settings(age: PasstAge, resolver: Option<&str>) -> Vec<PasstSetting> {
    use PasstSetting::{Switch, Value};
    let mut settings: Vec<PasstSetting> =
        PASST_NETWORK.iter().map(|(name, value)| Value(name, (*value).to_owned())).collect();
    settings.push(if age.takes_loopback_none() {
        Value("map-host-loopback", "none".into())
    } else {
        Switch(NO_MAP_GW)
    });
    match resolver {
        Some(address) if !age.takes_dns_host() => settings.push(Value("dns", address.to_owned())),
        _ => {
            settings.extend([Value("dns", PASST_DNS.into()), Value("dns-forward", PASST_DNS.into())]);
            if let Some(address) = resolver {
                settings.push(Value("dns-host", address.to_owned()));
            }
        }
    }
    settings
}

fn passt_words_of(settings: &[PasstSetting]) -> Vec<String> {
    let mut words = vec![PASST_QUIET.to_owned()];
    words.extend(settings.iter().flat_map(PasstSetting::passt_words));
    words
}

/// The words to ask this PC's passt with whether it is of an age: that age's own, with a resolver
/// where it would be given one.
pub fn passt_probe_words(age: PasstAge) -> Vec<String> {
    let resolver = age.takes_dns_host().then_some(PROBE_DNS_HOST);
    passt_words_of(&passt_settings(age, resolver))
}

/// The words to ask this PC's passt with whether it can hold a machine to one interface: the newest
/// age's own and a bound interface.
pub fn only_through_probe_words() -> Vec<String> {
    let mut words = passt_probe_words(PasstAge::Current);
    words.extend([PASST_OUT_IF4.to_owned(), "lo".to_owned()]);
    words
}

impl VmPlan {
    fn passt_settings(&self) -> Vec<PasstSetting> {
        passt_settings(self.network.passt_age, self.network.resolver.as_deref())
    }

    /// passt's own words, for the jail's launcher to start it with. It's told of its socket there.
    pub(super) fn passt_words(&self) -> Vec<String> {
        let mut words = passt_words_of(&self.passt_settings());
        words.extend(self.only_through_words());
        words
    }

    /// What binds passt's connections to the one interface. Only the launcher's passt has these:
    /// QEMU's own passt network doesn't take them, and a machine held to an interface never gets it.
    fn only_through_words(&self) -> Vec<String> {
        let Some(through) = &self.network.only_through else { return Vec::new() };
        let interface = through.interface.clone();
        let mut words = vec![PASST_OUT_IF4.to_owned(), interface.clone()];
        if through.ipv6 {
            words.extend([PASST_OUT_IF6.to_owned(), interface]);
        } else {
            words.push(PASST_IPV4_ONLY.to_owned());
        }
        words
    }

    /// Whether the jail's launcher starts passt and QEMU only has its end of the connection. It does
    /// in a jail, and for a machine held to one interface also without one, where the launcher only
    /// opens passt.
    pub(super) fn launcher_starts_passt(&self) -> bool {
        self.jail.is_some() || self.network.only_through.is_some()
    }

    /// The agent, the files helper, the share and any forwards always go through QEMU's user network, as nothing else
    /// can run a program per connection. Restricted, that network carries them and nothing more.
    fn private_link(&self) -> String {
        let mut link = format!("hostfwd=tcp:127.0.0.1:{}-:{GUEST_AGENT_PORT}", self.agent_port);
        if let Some(port) = self.files_port {
            link += &format!(",hostfwd=tcp:127.0.0.1:{port}-:{GUEST_FILES_PORT}");
        }
        if let Some(port) = self.app_windows_port {
            link += &format!(",hostfwd=tcp:127.0.0.1:{port}-:{GUEST_APP_WINDOWS_PORT}");
        }
        if let Some(port) = self.remote_view_port {
            link += &format!(",hostfwd=udp:127.0.0.1:{port}-:{REMOTEMAN_PORT}");
        }
        if self.share.is_some() {
            let program = shell_quote(&self.share_program.to_string_lossy());
            link += &format!(",guestfwd=tcp:{SHARE_ADDRESS}:80-cmd:{}", qemu_value(&program));
        }
        // QEMU answers a guest's UDP from the first forward to its port, so a second one to
        // remoteman's port would take the remote view's replies: the remote view's is the one.
        let taken = |item: &&crate::network::Forward| {
            self.remote_view_port.is_some()
                && item.protocol == crate::network::Protocol::Udp
                && item.guest == u32::from(REMOTEMAN_PORT)
        };
        for item in self.network.forwards.iter().filter(|item| !taken(item)) {
            link += &format!(",hostfwd={}:127.0.0.1:{}-:{}", item.protocol.word(), item.host, item.guest);
        }
        link
    }

    /// The network card: an e1000e, or for macOS, which has no driver for one, a virtio card.
    fn nic_card(&self) -> &'static str {
        if self.macos_boot().is_some() { MACOS_CARD } else { CARD }
    }

    /// The machine's own address as the word for a card, or nothing for a machine that has none yet,
    /// which keeps QEMU's default.
    fn mac_word(&self) -> String {
        match self.network.mac.as_str() {
            "" => String::new(),
            mac => format!(",mac={mac}"),
        }
    }

    fn first_nic_model(&self) -> String {
        format!("model={}{}", self.nic_card(), self.mac_word())
    }

    /// The only NIC of an Ubuntu guest, through QEMU's own NAT whatever the machine's network says,
    /// with the machine's own address like any other machine's card. `settings` are the network's own.
    pub(super) fn ubuntu_nic(&self, settings: &str) -> String {
        let comma = if settings.is_empty() { "" } else { "," };
        format!("{QEMU_NAT},{NO_HOST_LOOPBACK},model=virtio-net-pci{}{comma}{settings}", self.mac_word())
    }

    /// What the first NIC is connected to: QEMU's name for it, and its settings if it has any.
    fn first_backend(&self) -> (&'static str, String) {
        let link = self.private_link();
        match (self.network.mode, self.network.passt) {
            // The jail's launcher started passt, and QEMU has its end of the connection.
            (Mode::Nat, true) if self.launcher_starts_passt() => ("socket", format!("fd={PASST_FD}")),
            (Mode::Nat, true) => {
                let settings: Vec<String> =
                    self.passt_settings().iter().map(PasstSetting::qemu_word).collect();
                ("passt", settings.join(","))
            }
            (Mode::Nat, false) => (QEMU_NAT, format!("{NO_HOST_LOOPBACK},{link}")),
            (Mode::None, _) => (QEMU_NAT, format!("restrict=on,{link}")),
            // The jail's launcher has the device from the helper already, as QEMU can't run it.
            (Mode::Bridged, _) if self.jail.is_some() => ("tap", format!("fd={TAP_FD}")),
            (Mode::Bridged, _) => {
                let bridge = if self.network.bridge.is_empty() {
                    network::DEFAULT_BRIDGE
                } else {
                    &self.network.bridge
                };
                ("bridge", format!("br={bridge},helper={}", bridge_helper()))
            }
        }
    }

    /// The first NIC stays `-nic` in every mode. `-device` would come after the others and move them.
    pub(super) fn first_nic(&self) -> String {
        let (kind, settings) = self.first_backend();
        format!("{kind},{},{settings}", self.first_nic_model())
    }

    /// The first NIC of a macOS guest, which was installed with its card at a place `-nic` can't name,
    /// so it's a network and a card on it.
    pub(super) fn macos_first_nic(&self) -> Vec<String> {
        let (kind, settings) = self.first_backend();
        // QEMU's own NAT gives macOS an IPv6 address that leads nowhere on most PCs.
        let settings = if kind == QEMU_NAT { format!("ipv6=off,{settings}") } else { settings };
        let card = match self.network.mac.as_str() {
            "" => format!("{MACOS_CARD},netdev=net0,id=net0,addr={MACOS_CARD_SLOT}"),
            mac => format!("{MACOS_CARD},netdev=net0,id=net0,mac={mac},addr={MACOS_CARD_SLOT}"),
        };
        words(&["-netdev", &format!("{kind},id=net0,{settings}"), "-device", &card])
    }

    /// The first NIC of an Android guest, which has a driver for virtio's card and not Intel's.
    pub(super) fn android_nic(&self) -> Vec<String> {
        let (kind, settings) = self.first_backend();
        let comma = if settings.is_empty() { "" } else { "," };
        words(&["-nic", &format!("{kind},model={MACOS_CARD}{}{comma}{settings}", self.mac_word())])
    }

    /// The second NIC goes last, in a slot of its own, so that nothing the guest knows about moves.
    pub(super) fn second_nic(&self) -> Vec<String> {
        let wanted = self.network.mode == Mode::Bridged || self.network.passt;
        if !wanted {
            return Vec::new();
        }
        words(&[
            "-netdev",
            &format!("user,id=link,restrict=on,{}", self.private_link()),
            "-device",
            &format!("{},netdev=link,mac={CONTROL_MAC},bus=pcie.0,addr=0x1e", self.nic_card()),
        ])
    }
}

/// Every QEMU user network in `arguments`, each with whether it keeps the guest off the host's
/// loopback: restricted, or with [`NO_HOST_LOOPBACK`].
#[cfg(test)]
pub(super) fn user_networks(arguments: &[String]) -> Vec<(String, bool)> {
    arguments
        .windows(2)
        .filter(|pair| pair[0] == "-nic" || pair[0] == "-netdev")
        .map(|pair| &pair[1])
        .filter(|value| value.split(',').next() == Some(QEMU_NAT))
        .map(|value| {
            let words: Vec<&str> = value.split(',').collect();
            (value.clone(), words.contains(&"restrict=on") || words.contains(&NO_HOST_LOOPBACK))
        })
        .collect()
}

/// Panics unless `arguments` have a QEMU user network and every one keeps the guest off the host's
/// loopback.
#[cfg(test)]
pub(super) fn assert_user_networks_keep_off_host_loopback(arguments: &[String]) {
    let networks = user_networks(arguments);
    assert!(!networks.is_empty(), "no user network in {arguments:?}");
    for (network, kept) in networks {
        assert!(kept, "a QEMU user network is restricted or has {NO_HOST_LOOPBACK}: {network}");
    }
}
