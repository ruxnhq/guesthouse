//! The Windows host's checks of a machine's settings, held word for word as they are: the name, the
//! edition, the forwards and the shared folders. The app and the service both refuse with them.

use guesthouse_control::{
    Forward, MOST_FORWARDS, MOST_SHARES, Share, forwards_refusal, shares_refusal, valid_edition, valid_name,
};
use std::path::PathBuf;

#[test]
fn names_taken_and_refused() {
    for name in ["a", "Win11", "a-b_c", "0", &"x".repeat(40)] {
        assert!(valid_name(name), "{name}");
    }
    for name in ["", &"x".repeat(41), "a b", "a.b", "a/b", "é", "a\\b", "con."] {
        assert!(!valid_name(name), "{name:?}");
    }
}

#[test]
fn editions_taken_and_refused() {
    for edition in ["", "Windows 11 Pro", "Pro", " ", &"x".repeat(64)] {
        assert!(valid_edition(edition), "{edition:?}");
    }
    for edition in [&"x".repeat(65), "Pro-N", "Pro\t", "é", "a/b"] {
        assert!(!valid_edition(edition), "{edition:?}");
    }
}

fn forward(host_port: u16, guest_port: u16) -> Forward {
    Forward { host_port, guest_port }
}

#[test]
fn forwards_refused_word_for_word() {
    assert_eq!(MOST_FORWARDS, 16);
    assert_eq!(forwards_refusal(&[]), None);
    let sixteen: Vec<Forward> = (1..=16).map(|port| forward(port, 22)).collect();
    assert_eq!(forwards_refusal(&sixteen), None);
    let seventeen: Vec<Forward> = (1..=17).map(|port| forward(port, 22)).collect();
    assert_eq!(forwards_refusal(&seventeen).as_deref(), Some("A machine has 16 port forwards at most."));
    assert_eq!(forwards_refusal(&[forward(0, 22)]).as_deref(), Some("A port is 1 to 65535."));
    assert_eq!(forwards_refusal(&[forward(22, 0)]).as_deref(), Some("A port is 1 to 65535."));
    assert_eq!(
        forwards_refusal(&[forward(2222, 22), forward(2222, 23)]).as_deref(),
        Some("This PC's port 2222 is forwarded twice.")
    );
    assert_eq!(forwards_refusal(&[forward(2222, 22), forward(2223, 22)]), None, "a guest port may repeat");
    assert_eq!(
        forwards_refusal(&[forward(2222, 22), forward(2222, 0)]).as_deref(),
        Some("A port is 1 to 65535."),
        "the first problem found is said"
    );
}

fn share(folder: &str) -> Share {
    Share { folder: PathBuf::from(folder), read_only: false }
}

#[test]
fn shares_refused_word_for_word() {
    assert_eq!(MOST_SHARES, 8);
    assert_eq!(shares_refusal(&[]), None);
    let eight: Vec<Share> = (0..8).map(|n| share(&format!("C:\\S{n}"))).collect();
    assert_eq!(shares_refusal(&eight), None);
    let nine: Vec<Share> = (0..9).map(|n| share(&format!("C:\\S{n}"))).collect();
    assert_eq!(shares_refusal(&nine).as_deref(), Some("A machine shares 8 folders at most."));
    assert_eq!(shares_refusal(&[share("")]).as_deref(), Some("A shared folder needs a path."));
    assert_eq!(
        shares_refusal(&[share("C:\\Games"), share("c:\\games\\")]).as_deref(),
        Some("c:\\games\\ is shared twice.")
    );
    assert_eq!(shares_refusal(&[share("C:\\Games"), share("C:\\Games2")]), None);
}
