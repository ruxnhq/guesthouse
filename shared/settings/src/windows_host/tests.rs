//! The Windows host's name and edition rules, as they were held in `control` before they moved here.

use super::*;

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
