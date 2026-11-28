// SPDX-License-Identifier: GPL-2.0-or-later
use super::*;

#[test]
fn a_password_in_json_or_a_command_is_hidden_and_the_rest_of_the_line_stays() {
    assert_eq!(
        scrub(r#"{"user":"a","password":"k7mn-p3xq-ab9d-w4ht","x":1}"#),
        r#"{"user":"a","password":"<hidden>","x":1}"#
    );
    assert_eq!(scrub("sent password=hunter2 to the agent"), "sent password=<hidden> to the agent");
    assert_eq!(scrub("keychain: abc"), "keychain: <hidden>");
    assert_eq!(scrub("the Keychain item"), "the Keychain item");
}

#[test]
fn the_apps_own_password_shape_is_hidden_wherever_it_stands() {
    assert_eq!(scrub("it was k7mn-p3xq-ab9d-w4ht in the end"), "it was <hidden> in the end");
    assert_eq!(scrub("ubuntu-26-live-server"), "ubuntu-26-live-server");
}

#[test]
fn a_fingerprint_and_a_pairing_code_are_hidden() {
    let sha = "9f2c41ab".repeat(8);
    assert_eq!(scrub(&format!("rmhost: paired {sha} Phone")), "rmhost: paired <fingerprint> Phone");
    let colons = vec!["ab"; 32].join(":");
    assert_eq!(scrub(&format!("fingerprint {colons}")), "fingerprint <fingerprint>");
    assert_eq!(scrub(r#"pending {"code":"482913","name":"x"}"#), r#"pending {"code":"<code>","name":"x"}"#);
    // A MAC address is the Scrubber's, and is too short to be taken for a fingerprint.
    assert_eq!(scrub("52:54:00:aa:bb:cc"), "52:54:00:aa:bb:cc");
}

#[test]
fn serial_numbers_go_but_serial_ports_and_files_stay() {
    assert_eq!(scrub("serial: C02XYZ12345"), "serial: <serial>");
    assert_eq!(scrub("serial number = C02XYZ12345"), "serial number = <serial>");
    assert_eq!(scrub("serial.log is there"), "serial.log is there");
    assert_eq!(scrub("the serial port said hi"), "the serial port said hi");
}

#[test]
fn any_users_home_folder_is_a_tilde() {
    assert_eq!(scrub("/Users/pat/Library/x and /Users/lee/y"), "~/Library/x and ~/y");
    assert_eq!(scrub("/Users/Shared/x"), "/Users/Shared/x");
    assert_eq!(scrub("a\nb\n"), "a\nb\n");
}

#[test]
fn the_macs_pass_cannot_put_back_what_the_shared_list_takes_out() {
    use guesthouse_report_core::scrub::Scrubber;
    let shared = Scrubber::new("pat", "/Users/pat", "pat-mac", &["Quill".to_owned()]);
    let with_pass = Scrubber::new("pat", "/Users/pat", "pat-mac", &["Quill".to_owned()]).with_extra(scrub);
    let text = "password=hunter2 for pat on pat-mac at /Users/pat/Library/x, /Users/lee/y\n\
        fingerprint 9f2c41ab9f2c41ab9f2c41ab9f2c41ab9f2c41ab9f2c41ab9f2c41ab9f2c41ab Quill\n\
        code: 123 456, serial number: C02ABC, mail pat@example.org, 1920x1080\n";
    let done = with_pass.scrub(text);
    for secret in ["hunter2", "pat", "Quill", "lee", "9f2c41ab", "123 456", "C02ABC", "pat@example.org"] {
        assert!(!done.contains(secret), "{secret} is left in:\n{done}");
    }
    // Scrubbing what came out again with the shared list alone finds nothing more to take out.
    assert_eq!(shared.scrub(&done), done);
    // And the host's pass alone is unchanged by what the shared list did around it.
    assert_eq!(scrub(&done), done);
}
