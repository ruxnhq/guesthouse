//! The rule for a guest whose system isn't known yet. The rules themselves are held at their edges,
//! word for word, in `linux_host`'s tests.

use super::*;

#[test]
fn a_name_either_system_takes_passes() {
    // Windows only: capitals, dots, spaces, and Ubuntu's own users.
    // Ubuntu only: longer than Windows takes, a leading underscore, and Windows' own accounts.
    for name in
        ["a", "guesthouse", "Ann B.Lee", "root", "syslog", &"x".repeat(32), "_a", "administrator", "krbtgt"]
    {
        assert_eq!(either_name_problem(name), None, "{name:?}");
    }
}

#[test]
fn a_name_neither_takes_is_refused_with_the_windows_reason() {
    // An empty name is refused by both: each host gives its default before it asks.
    for name in ["", "Administrator", "a@b", " a", "a/b", "é", ".", &"x".repeat(33), "Guest", "SYSTEM"] {
        let windows = windows_name_problem(name);
        assert!(windows.is_some() && linux_name_problem(name).is_some(), "{name:?}");
        assert_eq!(either_name_problem(name), windows, "{name:?}");
    }
    assert_eq!(
        either_name_problem("Administrator").as_deref(),
        Some("Windows has an account called Administrator already. Choose another name.")
    );
}
