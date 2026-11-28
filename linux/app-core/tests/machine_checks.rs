//! The Linux host checks a machine's file when it loads it, and a machine with a problem isn't
//! listed. So the checks are held here as they are on machine files of the shapes the host has
//! written over time (the contents are made up): each one loads, and says exactly the problems it
//! said, in the same order, with the same words. The value checks the file's network settings go
//! through are held word for word too.

#![cfg(target_os = "linux")]

use guesthouse_app_core::machine::Machine;
use guesthouse_app_core::network::{bridge_name_problem, dns_problem, interface_name_problem, mac_problem};

fn problems(file: &str, max_vram_gib: u32) -> Vec<String> {
    let path = format!("{}/tests/machine_files/{file}", env!("CARGO_MANIFEST_DIR"));
    let mut machine: Machine = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    machine.id = file.trim_end_matches(".json").to_owned();
    machine.problems(max_vram_gib)
}

#[test]
fn machines_that_load_today_still_load() {
    for file in [
        "oldest-windows.json",
        "windows-with-share.json",
        "ubuntu-desk.json",
        "plain-boot.json",
        "bridged-with-forwards.json",
    ] {
        assert_eq!(problems(file, 8), Vec::<String>::new(), "{file}");
    }
    assert_eq!(
        problems("windows-with-share.json", 0),
        ["The video memory cap must be a whole number up to 0 GiB."]
    );
}

#[test]
fn every_problem_is_said_as_before_and_in_the_same_order() {
    let said = [
        "The machine needs a name.",
        "Windows has an account called Administrator already. Choose another name.",
        "Cores must be a whole number, at least 2.",
        "RAM must be a whole number of GiB, at least 4.",
        "The screen size must be one of 1920x1080, 2560x1440, 3840x2160.",
        "The refresh rate must be a whole number from 30 to 360 Hz.",
        "relative/disk.qcow2 must be a full path on this PC.",
        "vars.fd must be a full path on this PC.",
        "shared must be a full path on this PC.",
        "Ports must be whole numbers from 1 to 65535.",
        "TCP port 2222 on the host is forwarded twice.",
        "A bridge's name is up to 15 letters, digits, dots, dashes or underscores.",
        "An interface's name is up to 15 characters, with no spaces, commas or slashes.",
        "A DNS server is an IPv4 address, like 10.64.0.1.",
        "A machine can have 3 drives at most.",
        "A drive must be one of the names under /dev/disk/by-id.",
        "ata-made-up is given twice.",
        "The video memory cap must be a whole number up to 8 GiB.",
    ];
    assert_eq!(problems("everything-wrong.json", 8), said);
    assert_eq!(
        problems("network-mistakes.json", 8),
        [
            "A Linux user name starts with a small letter or an underscore, and holds small letters, digits, dashes and underscores.",
            "This is a multicast address, so a network card cannot have it. The first pair must be even.",
            "Drives are for Windows and plain machines.",
        ]
    );
}

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
