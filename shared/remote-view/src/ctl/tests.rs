use super::*;

const FP: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn line(words: &Words) -> String {
    words.join(" ")
}

#[test]
fn the_commands_with_no_device_in_them() {
    for (words, want) in [
        (status(), "status"),
        (pair_open(), "pair on"),
        (pair_close(), "pair off"),
        (device_drop(), "drop"),
        (share(true), "sharing on"),
        (share(false), "sharing off"),
        (updates_off(), "set updates off"),
    ] {
        assert_eq!(line(&words), want);
    }
}

#[test]
fn updates_are_off_only_when_the_host_said_ok() {
    assert_eq!(updates_off_took("ok\r\n"), Ok(()));
    for answer in [
        "",
        "the host is stopping",
        "done, but it won't be remembered: access denied",
        "this host was started with --updates check, which decides this",
        "this host doesn't look for updates",
        // What 0.1.8 answers to words it doesn't know.
        "status | pair on|off|accept|decline | set lock|speakers|notify on|off",
    ] {
        assert!(updates_off_took(answer).is_err(), "{answer:?}");
    }
    assert_eq!(updates_off_took(""), Err("the host said nothing".into()));
}

#[test]
fn the_host_wide_switches_take_clipboard_and_files_only() {
    assert_eq!(line(&host_allow("clipboard", true).unwrap()), "allow clipboard on");
    assert_eq!(line(&host_allow("files", false).unwrap()), "allow files off");
    for what in ["sound", "input", "mic", "camera", "", "clipboard on"] {
        assert!(host_allow(what, true).is_err(), "{what}");
    }
}

#[test]
fn a_device_is_allowed_by_what_and_control_is_the_opposite_of_view_only() {
    let table = [
        ("input", true, "view-only off"),
        ("input", false, "view-only on"),
        ("clipboard", true, "allow clipboard on"),
        ("clipboard", false, "allow clipboard off"),
        ("files", true, "allow files on"),
        ("files", false, "allow files off"),
        ("sound", true, "allow sound on"),
        ("mic", false, "allow mic off"),
        ("camera", true, "allow camera on"),
    ];
    for (what, on, tail) in table {
        let words = device_allow(FP, what, on).unwrap();
        assert_eq!(line(&words), format!("device {FP} {tail}"));
    }
    assert!(device_allow(FP, "root", true).unwrap_err().contains("not something a device can be allowed"));
    assert!(device_allow(FP, "", true).is_err());
}

#[test]
fn a_fingerprint_is_sixty_four_hex_digits_and_goes_in_in_lower_case() {
    assert_eq!(line(&device_revoke(FP).unwrap()), format!("revoke {FP}"));
    assert_eq!(check_fingerprint(&FP.to_uppercase()).unwrap(), FP);
    let bad = [
        "",
        "ab12",
        &FP[..63],
        &format!("{FP}0"),
        &FP.replace('0', "g"),
        &format!("{FP}\npair on")[..],
        &format!(" {}", &FP[1..]),
        &format!("{}\u{ff10}", &FP[1..]),
        &format!("{} ", &FP[..63]),
        &format!("{};drop", &FP[..58]),
        &format!("{}\u{e9}", &FP[..62]),
    ];
    for fingerprint in bad {
        assert_eq!(
            check_fingerprint(fingerprint).unwrap_err(),
            "That is not a device's fingerprint.",
            "{fingerprint:?}"
        );
        assert!(device_revoke(fingerprint).is_err());
        assert!(device_allow(fingerprint, "files", true).is_err());
    }
}

#[test]
fn every_word_is_its_own_string() {
    // A name with spaces or quotes cannot be one: nothing but the fingerprint is supplied, and it
    // is hex. The words stay apart, so a program is run with them as they are.
    assert_eq!(device_allow(FP, "files", false).unwrap().len(), 5);
    assert_eq!(device_revoke(FP).unwrap().len(), 2);
}

#[test]
fn the_hosts_answers_are_told_apart() {
    let device = "Pat's iPad (0123456789abcdef): can control this PC; clipboard not allowed, files not allowed, sound allowed, microphone allowed, camera allowed";
    let view_only = "Den (abababababababab): view only; clipboard allowed, files allowed, sound allowed, microphone allowed, camera allowed";
    let table: [(&str, Ack); 12] = [
        ("ok\n", Ack::Done("ok".into())),
        ("ok\r\n", Ack::Done("ok".into())),
        (
            "ok: every device is asked for a code again\n",
            Ack::Done("ok: every device is asked for a code again".into()),
        ),
        (device, Ack::Done(device.into())),
        (view_only, Ack::Done(view_only.into())),
        (
            "done, but it won't be remembered: Access is denied.\n",
            Ack::NotKept("done, but it won't be remembered: Access is denied.".into()),
        ),
        ("no device is waiting\n", Ack::Refused("no device is waiting".into())),
        ("no device is connected\n", Ack::Refused("no device is connected".into())),
        ("no such device\n", Ack::Refused("no such device".into())),
        ("the host is stopping\n", Ack::Stopping),
        (
            "status | second-factor on|confirm CODE|off|status|forget | pair on|off\n",
            Ack::Refused("status | second-factor on|confirm CODE|off|status|forget | pair on|off".into()),
        ),
        ("", Ack::Refused(String::new())),
    ];
    for (answer, want) in table {
        assert_eq!(parse_ack(answer), want, "{answer:?}");
    }
}

#[test]
fn accept_and_decline_name_the_device_by_its_fingerprint() {
    assert_eq!(line(&pair_accept(FP).unwrap()), format!("pair accept {FP}"));
    assert_eq!(line(&pair_decline(&FP.to_ascii_uppercase()).unwrap()), format!("pair decline {FP}"));
    for bad in ["", "0123", "Tablet", &format!("{FP}0")] {
        assert_eq!(pair_accept(bad), Err(NOT_A_FINGERPRINT.to_string()), "{bad:?}");
        assert_eq!(pair_decline(bad), Err(NOT_A_FINGERPRINT.to_string()), "{bad:?}");
    }
}
