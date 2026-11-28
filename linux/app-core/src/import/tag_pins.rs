//! The tag a snapshot of the Linux host's is kept under, as an import reads it today, held before it
//! moves.

use super::is_snapshot_tag;

#[test]
fn a_tag_is_the_prefix_and_eight_lower_case_hex_digits() {
    for tag in ["guesthouse-00000000", "guesthouse-0123abcd", "guesthouse-ffffffff", "guesthouse-9a8b7c6d"] {
        assert!(is_snapshot_tag(tag), "{tag}");
    }
    for name in [
        "",
        "guesthouse-",
        "guesthouse-0123abc",
        "guesthouse-0123abcde",
        "guesthouse-0123ABCD",
        "guesthouse-0123abcg",
        "guesthouse- 123abcd",
        "Guesthouse-0123abcd",
        "guesthouse_0123abcd",
        "x-guesthouse-0123abcd",
        "0123abcd",
        "guesthouse-0123abcd ",
        "guesthouse-０１２３abcd",
        "guesthouse-0123abcd/",
        "by-hand",
        "1",
    ] {
        assert!(!is_snapshot_tag(name), "{name:?}");
    }
}
