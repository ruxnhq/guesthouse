use super::linux::{INSTALL_FINISHED, install_finished};
use super::*;

/// The Windows host's guest scripts, which write the words read here and read the one it writes.
const REPORT: &str = include_str!("../../../../windows/host/guest/report.ps1");
const GPU: &str = include_str!("../../../../windows/host/guest/linux/gpu.sh");
const SHUTDOWN: &str = include_str!("../../../../windows/host/guest/linux/shutdown.sh");
const USER_DATA: &str = include_str!("../../../../windows/host/guest/linux/user-data");

#[test]
fn a_report_is_done_only_on_a_line_that_starts_with_its_words() {
    assert_eq!(REPORT_DONE, "guesthouse: report done");
    for (line, done) in [
        ("guesthouse: report done", true),
        ("guesthouse: report done and more", true),
        (" guesthouse: report done", false),
        ("say \"guesthouse: report done\"", false),
        ("Guesthouse: report done", false),
        ("report done", false),
    ] {
        assert_eq!(report_done(line), done, "{line:?}");
    }
}

#[test]
fn an_ubuntu_install_is_finished_only_on_a_line_that_starts_with_its_word() {
    assert_eq!(INSTALL_FINISHED, "guesthouse-install-finished");
    assert!(install_finished("guesthouse-install-finished"));
    assert!(install_finished("guesthouse-install-finished\r"));
    assert!(!install_finished("running 'echo guesthouse-install-finished > /dev/ttyS0'"));
    assert!(!install_finished("guesthouse: guesthouse-install-finished"));
    assert!(!install_finished("nvfwd-install-finished"));
}

#[test]
fn each_line_gives_the_address_it_gives_today() {
    for (line, address) in [
        ("guesthouse: address: 192.0.2.7", Some("192.0.2.7")),
        ("address: 192.0.2.7", Some("192.0.2.7")),
        ("guesthouse: address: 2: eth0    inet 192.0.2.7/20 metric 100 brd 192.0.2.255", Some("192.0.2.7")),
        ("guesthouse: address: 1: lo    inet 127.0.0.1/8 scope host lo", None),
        ("guesthouse: address: 2: eth0    inet6 fe80::1/64 scope link", None),
        ("guesthouse: address: 127.0.0.1 192.0.2.7", Some("192.0.2.7")),
        ("guesthouse: address: 0.0.0.0", Some("0.0.0.0")),
        ("guesthouse: address: 192.0.2.300", None),
        ("guesthouse: address: 192.0.2.7:22", None),
        ("guesthouse: address:192.0.2.7", None),
        (" guesthouse: address: 192.0.2.7", None),
        ("guesthouse: card: 192.0.2.7", None),
    ] {
        assert_eq!(address_in(line).as_deref(), address, "{line:?}");
    }
}

#[test]
fn the_guest_scripts_write_the_words_read_here_and_wait_for_the_one_written() {
    let done = REPORT_DONE.trim_start_matches("guesthouse: ");
    assert!(REPORT.contains("try { $port.WriteLine(\"guesthouse: $Text\") }"));
    assert!(REPORT.contains(&format!("Send-Line '{done}'")));
    assert!(REPORT.contains("Send-Line \"address: $($address.IPAddress)\""));
    assert!(REPORT.contains(&format!("if ($line.Trim() -eq '{SHUTDOWN_LINE}') {{")));
    assert!(GPU.contains("printf 'guesthouse: %s\\n' \"$*\" > /dev/ttyS0\n"));
    assert!(GPU.contains(&format!("\nsay \"{done}\"\n")));
    assert!(GPU.contains("ip -o address show | say_each address\n"));
    assert!(SHUTDOWN.contains(&format!("ask=\"{SHUTDOWN_LINE}\"\n")));
    assert!(USER_DATA.contains(&format!("\n    - echo {INSTALL_FINISHED} > /dev/ttyS0\n")));
}
