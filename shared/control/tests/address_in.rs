//! The address in a guest's report line, held as it is read today: a Windows guest's line and an
//! Ubuntu guest's `ip -o address show` lines, each with and without the word the serial port adds.

use guesthouse_control::address_in;

#[test]
fn each_line_gives_the_address_it_gives_today() {
    let cases = [
        ("guesthouse: address: 192.0.2.7", Some("192.0.2.7")),
        ("address: 192.0.2.7", Some("192.0.2.7")),
        (
            "guesthouse: address: 2: eth0    inet 192.0.2.7/20 metric 100 brd 192.0.2.255 scope global dynamic eth0",
            Some("192.0.2.7"),
        ),
        (
            "address: 2: eth0    inet 198.51.100.20/24 brd 198.51.100.255 scope global eth0",
            Some("198.51.100.20"),
        ),
        ("guesthouse: address: 1: lo    inet 127.0.0.1/8 scope host lo\\       valid_lft forever", None),
        ("guesthouse: address: 2: eth0    inet6 fe80::1/64 scope link", None),
        ("guesthouse: address: 127.0.0.1", None),
        ("guesthouse: address: 127.0.0.1 192.0.2.7", Some("192.0.2.7")),
        ("guesthouse: address: 0.0.0.0", Some("0.0.0.0")),
        ("guesthouse: address: 192.0.2.7\t", Some("192.0.2.7")),
        ("guesthouse: address: 192.0.2.300", None),
        ("guesthouse: address: 192.0.2.7:22", None),
        ("guesthouse: address:192.0.2.7", None),
        (" guesthouse: address: 192.0.2.7", None),
        ("guesthouse: Address: 192.0.2.7", None),
        ("guesthouse: card: 192.0.2.7", None),
        ("guesthouse: address: ", None),
        ("", None),
    ];
    for (line, address) in cases {
        assert_eq!(address_in(line).as_deref(), address, "{line:?}");
    }
}
