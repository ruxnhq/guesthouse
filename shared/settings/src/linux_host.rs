//! The Linux host's checks of a machine's network values: a MAC address, the bridge's and the
//! interface's names, and the DNS server. Each takes a plain value and says what is wrong with it in
//! a sentence for a person. The Linux host runs them when it loads a machine's file.

use std::net::Ipv4Addr;

/// The second NIC's, which only ever sits on the VM's private link.
pub const CONTROL_MAC: &str = "52:54:00:67:68:01";
/// The kernel's limit on an interface's name.
pub const MOST_BRIDGE_NAME: usize = 15;

/// The MAC the user typed, in the form QEMU takes, or `None` if it isn't one that can be used.
pub fn read_mac(text: &str) -> Option<String> {
    let text = text.trim().to_lowercase().replace('-', ":");
    let pairs: Vec<&str> = text.split(':').collect();
    let hex = |pair: &&str| pair.len() == 2 && pair.bytes().all(|byte| byte.is_ascii_hexdigit());
    (pairs.len() == 6 && pairs.iter().all(hex)).then_some(text)
}

/// Why a NIC can't have this MAC. A low bit set in its first byte would make it a multicast address,
/// which guests and switches drop.
pub fn mac_problem(mac: &str) -> Option<String> {
    if read_mac(mac).as_deref() != Some(mac) {
        return Some("A MAC address is six pairs of hex digits, like 52:54:00:ab:cd:ef.".into());
    }
    if u8::from_str_radix(&mac[..2], 16).is_ok_and(|first| first & 1 == 1) {
        return Some(
            "This is a multicast address, so a network card cannot have it. The first pair must be even."
                .into(),
        );
    }
    if mac == "00:00:00:00:00:00" {
        return Some("A network card cannot have an address of all zeros.".into());
    }
    (mac == CONTROL_MAC).then(|| "Guesthouse's own link to the guest uses this address.".into())
}

/// The name goes into QEMU's options, where a comma would start another option.
pub fn bridge_name_problem(bridge: &str) -> Option<String> {
    let allowed = |byte: u8| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte);
    let good = (1..=MOST_BRIDGE_NAME).contains(&bridge.len()) && bridge.bytes().all(allowed);
    (!good).then(|| "A bridge's name is up to 15 letters, digits, dots, dashes or underscores.".into())
}

/// The interface's name goes to passt as a word of its own and to the launcher, so it must not hold
/// a space or a slash, and an empty one means the setting is off.
pub fn interface_name_problem(name: &str) -> Option<String> {
    let allowed = |byte: u8| byte.is_ascii_graphic() && !b",/".contains(&byte);
    let good = name.is_empty() || ((1..=MOST_BRIDGE_NAME).contains(&name.len()) && name.bytes().all(allowed));
    (!good).then(|| "An interface's name is up to 15 characters, with no spaces, commas or slashes.".into())
}

/// Why this can't be the DNS server of a machine that goes out through one interface. Empty finds one.
pub fn dns_problem(address: &str) -> Option<String> {
    let good = address.is_empty() || address.parse::<Ipv4Addr>().is_ok();
    (!good).then(|| "A DNS server is an IPv4 address, like 10.64.0.1.".into())
}

#[cfg(test)]
mod tests;
