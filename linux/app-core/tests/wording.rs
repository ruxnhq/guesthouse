//! The Linux host's small wording, held as it is at the edges of each unit, so moving a formatter can't
//! change what a person reads by a character: a download's size, a host drive's size, and the shell
//! quoting and base64 the guest's agent is sent. Each table was written from what today's code says.

#![cfg(target_os = "linux")]

use guesthouse_app_core::downloads::size_text;
use guesthouse_app_core::host_drives::HostDrive;
use guesthouse_app_core::text::{base64_decode, base64_encode, shell_quote};

const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;

#[test]
fn a_download_is_said_in_gib_called_gb_or_mib_called_mb_rounded_up() {
    for (bytes, said) in [
        (0, "0 MB"),
        (1, "1 MB"),
        (MIB - 1, "1 MB"),
        (MIB, "1 MB"),
        (MIB + 1, "2 MB"),
        (1023 * MIB, "1023 MB"),
        (GIB - 1, "1024 MB"),
        (GIB, "1.0 GB"),
        (GIB + 1, "1.0 GB"),
        (GIB + 51 * MIB, "1.0 GB"),
        (GIB + 52 * MIB, "1.1 GB"),
        (GIB + GIB / 4, "1.2 GB"),
        (GIB + GIB * 3 / 4, "1.8 GB"),
        (2 * GIB - 1, "2.0 GB"),
        (10 * GIB - MIB, "10.0 GB"),
        (100 * GIB, "100.0 GB"),
        (u64::MAX, "17179869184.0 GB"),
    ] {
        assert_eq!(size_text(bytes), said, "{bytes} bytes");
    }
}

fn drive(bytes: u64) -> HostDrive {
    HostDrive {
        id: String::new(),
        kernel_name: String::new(),
        bytes,
        model: String::new(),
        removable: false,
        in_use: None,
        serial: String::new(),
    }
}

#[test]
fn a_host_drive_is_said_in_the_makers_decimal_units() {
    for (bytes, said) in [
        (0, "0 MB"),
        (499_999, "0 MB"),
        (500_000, "0 MB"),
        (999_999, "1 MB"),
        (1_000_000, "1 MB"),
        (1_500_000, "2 MB"),
        (2_500_000, "2 MB"),
        (999_499_999, "999 MB"),
        (999_500_000, "1000 MB"),
        (999_999_999, "1000 MB"),
        (1_000_000_000, "1 GB"),
        (1_500_000_000, "2 GB"),
        (2_500_000_000, "2 GB"),
        (999_499_999_999, "999 GB"),
        (999_999_999_999, "1000 GB"),
        (1_000_000_000_000, "1.0 TB"),
        (1_049_999_999_999, "1.0 TB"),
        (1_050_000_000_000, "1.1 TB"),
        (1_250_000_000_000, "1.2 TB"),
        (2_000_398_934_016, "2.0 TB"),
        (u64::MAX, "18446744.1 TB"),
    ] {
        assert_eq!(drive(bytes).size_text(), said, "{bytes} bytes");
    }
}

#[test]
fn words_are_quoted_for_a_shell_only_when_they_need_it() {
    for (word, quoted) in [
        ("", "''"),
        ("a", "a"),
        ("_@%+=:,./-", "_@%+=:,./-"),
        ("Z9", "Z9"),
        ("a b", "'a b'"),
        ("$HOME", "'$HOME'"),
        ("*", "'*'"),
        ("~", "'~'"),
        ("'", r"''\'''"),
        ("it's", r"'it'\''s'"),
        ("é", "'é'"),
        ("a\nb", "'a\nb'"),
        ("a\"b", "'a\"b'"),
    ] {
        assert_eq!(shell_quote(word), quoted, "{word:?}");
    }
}

#[test]
fn base64_is_the_standard_alphabet_with_padding() {
    for (data, text) in [
        (&b""[..], ""),
        (b"\0", "AA=="),
        (b"\0\0", "AAA="),
        (b"\0\0\0", "AAAA"),
        (b"\xff\xff\xff", "////"),
        (b"\xfb\xef\xbe", "++++"),
        (b"Man", "TWFu"),
        (b"hello, guest\n", "aGVsbG8sIGd1ZXN0Cg=="),
    ] {
        assert_eq!(base64_encode(data), text, "{data:?}");
        assert_eq!(base64_decode(text).as_deref(), Some(data), "{text}");
    }
    let every: Vec<u8> = (0..=255).collect();
    assert_eq!(base64_decode(&base64_encode(&every)), Some(every));
}

#[test]
fn base64_reading_takes_what_it_always_took() {
    assert_eq!(base64_decode("Zg").as_deref(), Some(&b"f"[..]), "no padding");
    assert_eq!(base64_decode("Zg=====").as_deref(), Some(&b"f"[..]), "any padding");
    assert_eq!(base64_decode("====").as_deref(), Some(&b""[..]));
    assert_eq!(base64_decode("Z").as_deref(), Some(&b""[..]), "a lone letter is no byte");
    assert_eq!(base64_decode("Zm9v\n"), None, "a line end is not base64");
    assert_eq!(base64_decode("Zm=9v"), None, "padding inside is not");
    assert_eq!(base64_decode("Zm9v-_"), None, "nor is the URL alphabet");
}
