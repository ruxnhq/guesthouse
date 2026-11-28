//! The Linux host's rules at their edges, as they were held in `app-core` before they moved here.

use super::*;

const WINDOWS_CHARACTERS: &str =
    "A Windows user name can hold letters, digits, dots, dashes, underscores and spaces between words.";
const LINUX_CHARACTERS: &str = "A Linux user name starts with a small letter or an underscore, and holds small letters, digits, dashes and underscores.";

fn said(kind: Kind, name: &str) -> Option<String> {
    // An empty name is the default, which the host decides before it asks.
    if name.is_empty() {
        return None;
    }
    match kind {
        Kind::Windows => windows_name_problem(name),
        Kind::Linux => linux_name_problem(name),
    }
}

enum Kind {
    Windows,
    Linux,
}

#[test]
fn windows_names_taken_and_refused_word_for_word() {
    for name in ["", "a", "A", "0", "Ana B", "a.b", "a-b_c", ".a", "a.", &"x".repeat(20)] {
        assert_eq!(said(Kind::Windows, name), None, "{name:?}");
    }
    for (name, words) in [
        ("x".repeat(21), "A Windows user name has at most 20 characters.".to_owned()),
        ("é".repeat(21), "A Windows user name has at most 20 characters.".to_owned()),
        ("é".into(), WINDOWS_CHARACTERS.to_owned()),
        (" a".into(), WINDOWS_CHARACTERS.to_owned()),
        ("a ".into(), WINDOWS_CHARACTERS.to_owned()),
        ("a\tb".into(), WINDOWS_CHARACTERS.to_owned()),
        ("a@b".into(), WINDOWS_CHARACTERS.to_owned()),
        ("a/b".into(), WINDOWS_CHARACTERS.to_owned()),
        (".".into(), "A Windows user name needs a letter or a digit.".to_owned()),
        ("...".into(), "A Windows user name needs a letter or a digit.".to_owned()),
    ] {
        assert_eq!(said(Kind::Windows, &name), Some(words), "{name:?}");
    }
}

#[test]
fn windows_reserved_names_are_these_eight_in_any_case() {
    for name in [
        "administrator",
        "guest",
        "defaultaccount",
        "defaultuser0",
        "defaultuser1",
        "wdagutilityaccount",
        "system",
        "krbtgt",
        "Administrator",
        "GUEST",
    ] {
        let words = format!("Windows has an account called {name} already. Choose another name.");
        assert_eq!(said(Kind::Windows, name), Some(words), "{name}");
    }
    for name in ["admin", "root", "defaultuser2", "user", "guesthouse", "gpupv"] {
        assert_eq!(said(Kind::Windows, name), None, "{name}");
    }
}

#[test]
fn linux_names_taken_and_refused_word_for_word() {
    for name in ["", "a", "_", "_a", "a-b_2", "a0", &"x".repeat(32), "systemd", "rooty"] {
        assert_eq!(said(Kind::Linux, name), None, "{name:?}");
    }
    for (name, words) in [
        ("x".repeat(33), "A Linux user name has at most 32 characters.".to_owned()),
        ("A".into(), LINUX_CHARACTERS.to_owned()),
        ("aB".into(), LINUX_CHARACTERS.to_owned()),
        ("1a".into(), LINUX_CHARACTERS.to_owned()),
        ("-a".into(), LINUX_CHARACTERS.to_owned()),
        ("a.b".into(), LINUX_CHARACTERS.to_owned()),
        ("a b".into(), LINUX_CHARACTERS.to_owned()),
        ("é".into(), LINUX_CHARACTERS.to_owned()),
        ("systemd-".into(), "Ubuntu has a user called systemd- already. Choose another name.".to_owned()),
        ("systemd-x".into(), "Ubuntu has a user called systemd-x already. Choose another name.".to_owned()),
    ] {
        assert_eq!(said(Kind::Linux, &name), Some(words), "{name:?}");
    }
}

#[test]
fn linux_reserved_names_are_these_thirty_two() {
    for name in [
        "root",
        "daemon",
        "bin",
        "sys",
        "sync",
        "games",
        "man",
        "lp",
        "mail",
        "news",
        "uucp",
        "proxy",
        "www-data",
        "backup",
        "list",
        "irc",
        "nobody",
        "_apt",
        "sshd",
        "messagebus",
        "polkitd",
        "gdm",
        "gnome-initial-setup",
        "tss",
        "uuidd",
        "tcpdump",
        "landscape",
        "fwupd-refresh",
        "pollinate",
        "lxd",
        "dhcpcd",
        "syslog",
    ] {
        let words = format!("Ubuntu has a user called {name} already. Choose another name.");
        assert_eq!(said(Kind::Linux, name), Some(words), "{name}");
    }
    for name in ["admin", "ubuntu", "guest", "administrator", "desk"] {
        assert_eq!(said(Kind::Linux, name), None, "{name}");
    }
}

#[test]
fn passwords_taken_and_refused_word_for_word() {
    for password in ["", " ", "p", "é", &"x".repeat(127), &"é".repeat(127), "<&>\"'", "a b"] {
        assert_eq!(password_problem(password), None, "{password:?}");
    }
    let long = Some("The password has at most 127 characters.".to_owned());
    assert_eq!(password_problem(&"x".repeat(128)), long);
    assert_eq!(password_problem(&format!("{}\n", "x".repeat(127))), long, "length is said first");
    let control = Some("The password can't hold line breaks or tabs.".to_owned());
    for password in ["a\nb", "a\rb", "\t", "a\u{7f}", "a\u{0}", "a\u{85}"] {
        assert_eq!(password_problem(password), control, "{password:?}");
    }
}
