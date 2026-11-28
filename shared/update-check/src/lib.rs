//! Whether a newer Guesthouse is out. Each host fetches the file its releases publish, and this decides
//! what it says, with no OS or network calls, so all of them share it and it's tested anywhere.

use serde::Deserialize;

pub mod driver_manifest;
pub mod openpgp;
pub mod signed;

/// Where a Linux release says it's the latest, last of all once its packages are up, so a package
/// install and install.sh's --check-update both read it. `.github/workflows/packages.yml` writes it.
pub const LINUX_LATEST_URL: &str = "https://pkg.ruxn.com/guesthouse/latest.json";
/// The longest file taken. A real one is under a hundred bytes.
pub const LONGEST: usize = 4096;
/// Where a release's notes are. A link anywhere else in the file is never shown.
const NOTES: &str = "https://github.com/ruxnlabs/guesthouse/releases/";

/// The newest Linux release, as its file says.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Latest {
    pub version: String,
    /// The release's page, if it has one.
    #[serde(default)]
    pub notes: String,
}

/// The newest Linux release, from the file's text, if the file is one a release writes.
pub fn parse(text: &str) -> Option<Latest> {
    if text.len() > LONGEST {
        return None;
    }
    let latest: Latest = serde_json::from_str(text).ok()?;
    let plain = |link: &str| !link.chars().any(|c| c.is_whitespace() || c.is_control());
    let notes_ok = latest.notes.is_empty() || (latest.notes.starts_with(NOTES) && plain(&latest.notes));
    (is_version(&latest.version) && notes_ok).then_some(latest)
}

/// How a copy of Guesthouse was installed, which is how it updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// install.sh, or the AppImage, which runs it. It updates itself.
    Script,
    Apt,
    Dnf,
    Pacman,
}

impl Method {
    /// From the word a package writes in its `installed-by` file.
    pub fn from_installed_by(text: &str) -> Option<Self> {
        match text.trim() {
            "apt" => Some(Self::Apt),
            "dnf" => Some(Self::Dnf),
            "pacman" => Some(Self::Pacman),
            _ => None,
        }
    }

    /// Whether the app's own Update can do it. Arch takes no partial upgrades, so there Guesthouse
    /// comes with the rest of the system.
    pub fn updates_in_app(self) -> bool {
        self != Self::Pacman
    }
}

/// What a check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    Newer(String),
    Latest,
    /// No network, or the file couldn't say. It isn't worth bothering anyone about by itself.
    CantTell,
    /// A build of the repo, which updates through git, or a copy from a package with no way of its own
    /// to say, such as NixOS's flake, which updates the way it came.
    NotInstalled,
}

/// What the latest file says to a copy of version `current`. `latest` is the file's text, or `None`
/// if it couldn't be fetched.
pub fn found(current: &str, latest: Option<&str>) -> Found {
    match latest.and_then(parse) {
        Some(latest) if newer(&latest.version, current) => Found::Newer(latest.version),
        Some(_) => Found::Latest,
        None => Found::CantTell,
    }
}

/// Whether `latest` is a newer version than `current`, each three numbers.
pub fn newer(latest: &str, current: &str) -> bool {
    match (numbers(latest), numbers(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}

/// Whether `version` is three numbers, the only versions a release has.
pub fn is_version(version: &str) -> bool {
    numbers(version).is_some()
}

fn numbers(version: &str) -> Option<[u64; 3]> {
    let mut parts = version.trim().split('.').map(|part| {
        (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())).then(|| part.parse().ok()).flatten()
    });
    let found = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_higher_version_is_newer() {
        assert!(newer("0.10.0", "0.9.0"), "by number, not by text");
        assert!(newer("1.0.0", "0.99.99"));
        assert!(!newer("0.9.0", "0.9.0"));
        assert!(!newer("0.8.5", "0.9.0"));
        assert!(!newer("0.10", "0.9.0"));
        assert!(!newer("0.10.0", "garbled"));
    }

    #[test]
    fn a_release_file_is_read() {
        let text = r#"{"version": "0.11.0", "notes": "https://github.com/ruxnlabs/guesthouse/releases/tag/v0.11.0"}"#;
        assert_eq!(parse(text).unwrap().version, "0.11.0");
        assert_eq!(parse(r#"{"version": "0.11.0"}"#).unwrap().notes, "");
        assert_eq!(parse(r#"{"version": "0.11"}"#), None);
        assert_eq!(parse(r#"{"version": "0.11.0", "notes": "https://example.com/"}"#), None);
        assert_eq!(parse("not json"), None);
        assert_eq!(parse(&" ".repeat(LONGEST + 1)), None);
    }

    #[test]
    fn the_file_is_compared_with_the_running_version() {
        let file = r#"{"version": "0.11.0"}"#;
        assert_eq!(found("0.10.0", Some(file)), Found::Newer("0.11.0".to_owned()));
        assert_eq!(found("0.11.0", Some(file)), Found::Latest);
        assert_eq!(found("0.12.0", Some(file)), Found::Latest, "a newer build is never offered an older one");
        assert_eq!(found("0.10.0", Some("garbled")), Found::CantTell);
        assert_eq!(found("0.10.0", None), Found::CantTell);
    }

    #[test]
    fn a_package_says_which_manager_it_came_from() {
        assert_eq!(Method::from_installed_by("apt\n"), Some(Method::Apt));
        assert_eq!(Method::from_installed_by("dnf"), Some(Method::Dnf));
        assert_eq!(Method::from_installed_by("pacman"), Some(Method::Pacman));
        assert_eq!(Method::from_installed_by("zypper"), None);
        assert!(
            Method::Apt.updates_in_app() && Method::Dnf.updates_in_app() && Method::Script.updates_in_app()
        );
        assert!(!Method::Pacman.updates_in_app(), "no partial upgrades on Arch");
    }

    #[test]
    fn a_version_is_three_numbers() {
        assert!(is_version("0.10.0"));
        assert!(!is_version("0.10"));
        assert!(!is_version("0.10.0-rc1"));
        assert!(!is_version(""));
    }
}
