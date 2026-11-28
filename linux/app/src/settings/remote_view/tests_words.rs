//! A waiting device's line, word for word, as the Linux app shows it.

use super::*;
use guesthouse_remote_view_core::share::Pending;

#[test]
fn a_waiting_devices_line_is_word_for_word() {
    let pending =
        Pending { name: "Den".into(), fingerprint: "a".repeat(64), code: "482 917".into(), seconds: 60 };
    let info = ShareInfo { pending: Some(pending), ..ShareInfo::default() };
    assert_eq!(
        pending_text(&info, 42),
        "Code 482 917, 42 s left to answer\nCheck that this code matches the one shown on the device before you press Accept."
    );
    assert_eq!(
        pending_text(&ShareInfo::default(), 0),
        "Code , 0 s left to answer\nCheck that this code matches the one shown on the device before you press Accept."
    );
}
