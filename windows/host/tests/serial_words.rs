//! What a guest of the Windows host says on its serial port, and how the host reads it, held as it
//! is today: the words, the scripts that write them, and each guest line made printable. Installed
//! guests keep saying these words, so neither side may drift from the other.

use guesthouse_host_windows::guest::linux::{FILES, INSTALL_FINISHED, install_finished};
use guesthouse_host_windows::guest::{REPORT, REPORT_DONE, SHUTDOWN_LINE, report_done};
use guesthouse_host_windows::printable::{LINE_LIMIT, printable};

fn linux_file(name: &str) -> &'static str {
    FILES.iter().find(|(path, _)| path.ends_with(name)).unwrap().1
}

#[test]
fn the_words_are_these() {
    assert_eq!(REPORT_DONE, "guesthouse: report done");
    assert_eq!(SHUTDOWN_LINE, "guesthouse: shutdown");
    assert_eq!(INSTALL_FINISHED, "guesthouse-install-finished");
    assert_eq!(LINE_LIMIT, 4096);
}

#[test]
fn a_report_is_done_only_on_a_line_that_starts_with_its_words() {
    let cases = [
        ("guesthouse: report done", true),
        ("guesthouse: report done and more", true),
        ("guesthouse: report done\t", true),
        (" guesthouse: report done", false),
        ("guesthouse: report don", false),
        ("Guesthouse: report done", false),
        ("guesthouse:  report done", false),
        ("report done", false),
        ("", false),
    ];
    for (line, done) in cases {
        assert_eq!(report_done(line), done, "{line:?}");
    }
}

#[test]
fn an_ubuntu_install_is_finished_only_on_a_line_that_starts_with_its_word() {
    let cases = [
        ("guesthouse-install-finished", true),
        ("guesthouse-install-finished ", true),
        ("guesthouse-install-finished\r", true),
        (" guesthouse-install-finished", false),
        ("guesthouse: guesthouse-install-finished", false),
        ("nvfwd-install-finished", false),
        ("GUESTHOUSE-INSTALL-FINISHED", false),
        ("", false),
    ];
    for (line, finished) in cases {
        assert_eq!(install_finished(line), finished, "{line:?}");
    }
}

#[test]
fn the_windows_guests_report_writes_each_line_after_the_word_the_host_reads() {
    assert!(REPORT.contains("try { $port.WriteLine(\"guesthouse: $Text\") }"));
    assert!(REPORT.contains(
        "if ($address.IPAddress -ne '127.0.0.1') { Send-Line \"address: $($address.IPAddress)\" }"
    ));
    assert!(REPORT.contains("Send-Line 'report done'"));
    assert!(REPORT.contains("if ($line.Trim() -eq 'guesthouse: shutdown') {"));
}

#[test]
fn the_ubuntu_guests_scripts_write_each_line_after_the_word_the_host_reads() {
    let gpu = linux_file("gpu.sh");
    assert!(gpu.contains("say() {\n    printf 'guesthouse: %s\\n' \"$*\" > /dev/ttyS0\n}"));
    assert!(gpu.contains("say_each() {\n    while IFS= read -r line; do say \"$1: $line\"; done"));
    assert!(gpu.contains("ip -o address show | say_each address\n"));
    assert!(gpu.contains("\nsay \"report done\"\n"));
    assert!(linux_file("shutdown.sh").contains("ask=\"guesthouse: shutdown\"\n"));
    assert!(linux_file("user-data").contains("\n    - echo guesthouse-install-finished > /dev/ttyS0\n"));
}

#[test]
fn each_line_is_made_printable_as_today() {
    let cases: [(&[u8], &str); 16] = [
        (b"guesthouse: card: ok", "guesthouse: card: ok"),
        (b"guesthouse: card: ok\r\n", "guesthouse: card: ok"),
        (b"line\n\r\n\r", "line"),
        (b"a\rb\r\n", "a<0d>b"),
        (b"\x1b]0;owned\x07card: ok\r\n", "<1b>]0;owned<07>card: ok"),
        (b"\x1b[31mred\x1b[0m", "<1b>[31mred<1b>[0m"),
        (b"\x00x\x7f", "<00>x<7f>"),
        (b"tab\there", "tab\there"),
        (b"\x0b\x0c", "<0b><0c>"),
        ("caf\u{e9}".as_bytes(), "caf\u{e9}"),
        ("\u{85}next".as_bytes(), "<85>next"),
        ("\u{9b}31m".as_bytes(), "<9b>31m"),
        ("zero\u{200b}width\u{2028}".as_bytes(), "zero\u{200b}width\u{2028}"),
        (b"\x9bA", "\u{fffd}A"),
        (b"\xff\xfe", "\u{fffd}\u{fffd}"),
        (b"", ""),
    ];
    for (bytes, shown) in cases {
        assert_eq!(printable(bytes), shown, "{}", String::from_utf8_lossy(bytes));
    }
}

#[test]
fn a_line_is_cut_at_the_limit_in_bytes_before_it_is_read() {
    let mut split = vec![b'a'; LINE_LIMIT - 1];
    split.extend("\u{e9}".as_bytes());
    assert_eq!(printable(&split), format!("{}\u{fffd}", "a".repeat(LINE_LIMIT - 1)));
    let mut ended = vec![b'a'; LINE_LIMIT];
    ended.extend(b"\r\n");
    assert_eq!(printable(&ended), "a".repeat(LINE_LIMIT));
    let mut long = vec![b'b'; LINE_LIMIT - 1];
    long.extend(b"\x1b");
    long.extend(b"cut off");
    assert_eq!(printable(&long), format!("{}<1b>", "b".repeat(LINE_LIMIT - 1)));
}
