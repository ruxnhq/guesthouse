//! The rules every host gives the account an unattended install makes in a guest: the names a
//! Windows and an Ubuntu guest take, and the password. They're the Linux host's rules, which the
//! Windows host uses too, as should any host to come. Each refusal is a sentence for the person.
//!
//! A host that knows the guest's system asks [`windows_name_problem`] or [`linux_name_problem`]; one
//! that doesn't yet, as the Windows host before it has opened the image, asks
//! [`either_name_problem`] and the right one once it knows. An empty name or password means the
//! host's default or a random one, which each host decides before it asks these, and so do the
//! default names. The rules are for a new name only: an account a machine has already is never
//! checked again.

/// Both installers take the password as one line, and Windows stops at 127 characters.
pub const MOST_PASSWORD_CHARS: usize = 127;
pub const MOST_WINDOWS_NAME_CHARS: usize = 20;
pub const MOST_LINUX_NAME_CHARS: usize = 32;
/// Names Windows has, or makes during setup, so an account of that name can't be made.
pub const WINDOWS_RESERVED: [&str; 8] = [
    "administrator",
    "guest",
    "defaultaccount",
    "defaultuser0",
    "defaultuser1",
    "wdagutilityaccount",
    "system",
    "krbtgt",
];
/// Ubuntu's own users, which the installer refuses to make again.
pub const LINUX_RESERVED: [&str; 32] = [
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
];

/// Why this name won't do for a Windows guest's account, if it won't.
pub fn windows_name_problem(name: &str) -> Option<String> {
    let allowed = |letter: char| letter.is_ascii_alphanumeric() || matches!(letter, '-' | '_' | '.' | ' ');
    if name.chars().count() > MOST_WINDOWS_NAME_CHARS {
        return Some(format!("A Windows user name has at most {MOST_WINDOWS_NAME_CHARS} characters."));
    }
    if !name.chars().all(allowed) || name != name.trim() {
        return Some(
            "A Windows user name can hold letters, digits, dots, dashes, underscores and spaces between words."
                .to_owned(),
        );
    }
    if name.chars().all(|letter| letter == '.') {
        return Some("A Windows user name needs a letter or a digit.".to_owned());
    }
    if WINDOWS_RESERVED.contains(&name.to_ascii_lowercase().as_str()) {
        return Some(format!("Windows has an account called {name} already. Choose another name."));
    }
    None
}

/// Why this name won't do for an Ubuntu guest's account, if it won't.
pub fn linux_name_problem(name: &str) -> Option<String> {
    let mut letters = name.chars();
    let first_fits = letters.next().is_some_and(|first| first.is_ascii_lowercase() || first == '_');
    let rest_fit = letters
        .all(|letter| letter.is_ascii_lowercase() || letter.is_ascii_digit() || matches!(letter, '-' | '_'));
    if name.chars().count() > MOST_LINUX_NAME_CHARS {
        return Some(format!("A Linux user name has at most {MOST_LINUX_NAME_CHARS} characters."));
    }
    if !first_fits || !rest_fit {
        return Some(
            "A Linux user name starts with a small letter or an underscore, and holds small letters, digits, dashes and underscores."
                .to_owned(),
        );
    }
    if LINUX_RESERVED.contains(&name) || name.starts_with("systemd-") {
        return Some(format!("Ubuntu has a user called {name} already. Choose another name."));
    }
    None
}

/// Why this password won't do, if it won't. Empty means a random one, which is fine.
pub fn password_problem(password: &str) -> Option<String> {
    if password.chars().count() > MOST_PASSWORD_CHARS {
        return Some(format!("The password has at most {MOST_PASSWORD_CHARS} characters."));
    }
    if password.chars().any(char::is_control) {
        return Some("The password can't hold line breaks or tabs.".to_owned());
    }
    None
}

/// Why this name won't do for a guest whose system isn't known yet: why it won't do for a Windows
/// guest, if an Ubuntu guest won't take it either. A name either one takes passes, and the guest's
/// own rule is asked again once the system is known.
pub fn either_name_problem(name: &str) -> Option<String> {
    windows_name_problem(name).filter(|_| linux_name_problem(name).is_some())
}

#[cfg(test)]
mod tests;
