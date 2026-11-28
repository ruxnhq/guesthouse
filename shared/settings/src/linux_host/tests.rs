//! The Linux host's network value checks word for word, as they were held in `app-core` before
//! they moved here.

use super::*;

#[test]
fn network_values_word_for_word() {
    let bridge = Some("A bridge's name is up to 15 letters, digits, dots, dashes or underscores.".to_owned());
    for name in ["br0", "a", "a.b-c_d", &"x".repeat(15)] {
        assert_eq!(bridge_name_problem(name), None, "{name}");
    }
    for name in ["", &"x".repeat(16), "a,b", "a b", "a/b", "é"] {
        assert_eq!(bridge_name_problem(name), bridge, "{name:?}");
    }
    let interface =
        Some("An interface's name is up to 15 characters, with no spaces, commas or slashes.".to_owned());
    for name in ["", "wg0", "eth0:1", "a+b", &"x".repeat(15)] {
        assert_eq!(interface_name_problem(name), None, "{name}");
    }
    for name in [&"x".repeat(16), "a b", "a,b", "a/b", "é", "a\tb"] {
        assert_eq!(interface_name_problem(name), interface, "{name:?}");
    }
    let dns = Some("A DNS server is an IPv4 address, like 10.64.0.1.".to_owned());
    for address in ["", "10.64.0.1", "0.0.0.0", "255.255.255.255"] {
        assert_eq!(dns_problem(address), None, "{address}");
    }
    for address in [" 10.64.0.1", "10.64.0", "::1", "dns.example", "256.0.0.1"] {
        assert_eq!(dns_problem(address), dns, "{address:?}");
    }
    let shape = Some("A MAC address is six pairs of hex digits, like 52:54:00:ab:cd:ef.".to_owned());
    for mac in ["52:54:00:AB:CD:EF", "52-54-00-ab-cd-ef", "52:54:00:ab:cd", "", " 52:54:00:ab:cd:ef"] {
        assert_eq!(mac_problem(mac), shape, "{mac:?}");
    }
    assert_eq!(mac_problem("52:54:00:ab:cd:ef"), None);
    assert_eq!(mac_problem("02:00:00:00:00:01"), None);
    assert_eq!(
        mac_problem("01:00:5e:00:00:01").as_deref(),
        Some("This is a multicast address, so a network card cannot have it. The first pair must be even.")
    );
    assert_eq!(
        mac_problem("00:00:00:00:00:00").as_deref(),
        Some("A network card cannot have an address of all zeros.")
    );
    assert_eq!(
        mac_problem("52:54:00:67:68:01").as_deref(),
        Some("Guesthouse's own link to the guest uses this address.")
    );
}
