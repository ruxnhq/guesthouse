//! Every sentence of [`proposed`], word for word, as the app shows it.

use super::*;

#[test]
fn the_proposed_words_are_word_for_word() {
    assert_eq!(proposed::TITLE, "Remote View");
    assert_eq!(proposed::ADDRESS, "Address");
    assert_eq!(proposed::PORT, "Port");
    assert_eq!(proposed::FINGERPRINT, "Fingerprint");
    assert_eq!(proposed::PAIRING_OPEN, "Pairing is open");
    assert_eq!(proposed::CANCEL, "Cancel");
    assert_eq!(proposed::HELD, "Clipboard sharing with this PC pauses until pairing closes.");
    assert_eq!(
        proposed::BRIDGED,
        "This machine is bridged. Viewing from other devices is made for the shared network, and hasn't been \
         tried bridged yet."
    );
    assert_eq!(proposed::CLOSING, "Closing\u{2026}");
    assert_eq!(proposed::closes_in(105), "Closes in 1:45");
    assert_eq!(proposed::closes_in(0), "Closes in 0:00");
    assert_eq!(proposed::closes_in(3600), "Closes in 60:00");
    assert_eq!(proposed::left_to_answer(42), "42 s left to answer");
    assert_eq!(
        proposed::takes_screen("Phone"),
        "One device shows this machine at a time, so Phone has its screen and the machine's window here has \
         lost it. Open screen takes it back, and Phone's view ends. Neither connects again by itself."
    );
    assert_eq!(
        proposed::screen_line("Phone"),
        "Phone has this machine's screen now. Open screen takes it back."
    );
    assert_eq!(proposed::switch_name("Control", "Phone"), "Control for Phone");
    assert_eq!(proposed::clock(9), "0:09");
    assert_eq!(proposed::clock(61), "1:01");
}
