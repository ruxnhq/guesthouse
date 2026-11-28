//! The account an unattended install makes in the guest. The user may name it and give it a password
//! before the install; otherwise it gets the name the guest scripts have always used, and a random
//! password that stays in the machine's folder. The rules themselves are the Linux host's in
//! `guesthouse_account_core::linux_host`, which every host can build on.

use crate::machine::Kind;

pub use guesthouse_account_core::linux_host::{LINUX_USER, WINDOWS_USER, password_problem};
use guesthouse_account_core::linux_host::{linux_name_problem, windows_name_problem};

/// The name the guest's account gets when the user chose none.
pub fn default_user(kind: Kind) -> &'static str {
    match kind {
        Kind::Linux => LINUX_USER,
        Kind::Windows | Kind::Plain => WINDOWS_USER,
    }
}

/// Why this name won't do for the account, if it won't. Empty means the default, which is fine.
pub fn name_problem(kind: Kind, name: &str) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    match kind {
        Kind::Windows => windows_name_problem(name),
        Kind::Linux => linux_name_problem(name),
        Kind::Plain => None,
    }
}

/// The text as it goes inside an XML element or attribute: the Linux host's escape, which takes the
/// apostrophe too.
pub use guesthouse_answers_core::linux_host::xml_text;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_names_the_scripts_have_always_used() {
        assert_eq!(default_user(Kind::Windows), "gpupv");
        assert_eq!(default_user(Kind::Linux), "desk");
        assert_eq!(name_problem(Kind::Windows, ""), None);
        assert_eq!(name_problem(Kind::Linux, ""), None);
    }

    #[test]
    fn a_windows_name_is_what_windows_takes() {
        for name in ["Ana", "ana.b", "Ana B", "user-1", "a_b", "x".repeat(20).as_str()] {
            assert_eq!(name_problem(Kind::Windows, name), None, "{name}");
        }
        for (name, said) in [
            ("x".repeat(21).as_str(), "at most 20"),
            ("ana/b", "can hold"),
            ("ana@b", "can hold"),
            (" ana", "can hold"),
            ("ana'b", "can hold"),
            ("...", "needs a letter"),
            ("Administrator", "already"),
            ("defaultuser0", "already"),
        ] {
            assert!(
                name_problem(Kind::Windows, name).is_some_and(|problem| problem.contains(said)),
                "{name}"
            );
        }
    }

    #[test]
    fn a_linux_name_is_what_the_installer_takes() {
        for name in ["ana", "_ana", "ana-b_2", "x".repeat(32).as_str()] {
            assert_eq!(name_problem(Kind::Linux, name), None, "{name}");
        }
        for (name, said) in [
            ("x".repeat(33).as_str(), "at most 32"),
            ("Ana", "small letter"),
            ("1ana", "small letter"),
            ("ana b", "small letter"),
            ("root", "already"),
            ("systemd-network", "already"),
        ] {
            assert!(name_problem(Kind::Linux, name).is_some_and(|problem| problem.contains(said)), "{name}");
        }
    }

    #[test]
    fn a_plain_machine_has_no_account_to_name() {
        assert_eq!(name_problem(Kind::Plain, "anything goes/"), None);
    }

    #[test]
    fn a_password_is_one_line_windows_can_take() {
        assert_eq!(password_problem(""), None);
        assert_eq!(password_problem("p4ss <&> \"quoted\" 'and' ünïcode"), None);
        assert!(password_problem(&"x".repeat(128)).is_some());
        assert!(password_problem("two\nlines").is_some());
        assert!(password_problem("a\tb").is_some());
    }

    #[test]
    fn xml_text_escapes_what_xml_reads() {
        assert_eq!(xml_text("a<b>&\"c'"), "a&lt;b&gt;&amp;&quot;c&apos;");
        assert_eq!(xml_text("plain"), "plain");
    }
}
