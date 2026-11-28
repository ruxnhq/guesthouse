//! Whether a newer Windows host is out, from the small file a release publishes beside its installer,
//! `guesthouse-windows-latest.json`, as `scripts/windows-release.ps1` writes it. The app fetches it
//! and offers the installer; this decides what it says, so it's tested anywhere.

use guesthouse_update_check::is_version;
use serde::Deserialize;

/// Where a release publishes the file.
pub const LATEST_URL: &str = "https://files.ruxn.com/p/guesthouse/guesthouse-windows-latest.json";
/// Where an installer is published. A link anywhere else in the file is never offered.
const INSTALLERS: &str = "https://files.ruxn.com/p/guesthouse/";
/// Where a release's notes are.
const NOTES: &str = "https://github.com/ruxnlabs/guesthouse/releases/";
/// The longest file taken. A real one is a few hundred bytes.
pub const LONGEST: usize = 4096;

/// The newest Windows host, as its release says.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Latest {
    pub version: String,
    /// The installer to download.
    pub installer: String,
    pub sha256: String,
    /// The release's page, if it has one.
    #[serde(default)]
    pub notes: String,
}

/// The newest Windows host, from the file's text, if the file is one a release writes. Its links have
/// to be the project's own, as the app opens them.
pub fn parse(text: &str) -> Option<Latest> {
    if text.len() > LONGEST {
        return None;
    }
    let latest: Latest = serde_json::from_str(text).ok()?;
    let plain = |link: &str| !link.chars().any(|c| c.is_whitespace() || c.is_control());
    let installer_ok = latest.installer.starts_with(INSTALLERS)
        && latest.installer.ends_with(".exe")
        && !latest.installer[INSTALLERS.len()..].contains('/')
        && plain(&latest.installer);
    let notes_ok = latest.notes.is_empty() || (latest.notes.starts_with(NOTES) && plain(&latest.notes));
    let sha_ok = latest.sha256.len() == 64 && latest.sha256.bytes().all(|b| b.is_ascii_hexdigit());
    (is_version(&latest.version) && installer_ok && notes_ok && sha_ok).then_some(latest)
}

pub use guesthouse_update_check::newer;

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "41a84090e83b778e0314844eb8a403010d11d4d841b2fb0f31d63f1c1efbf08e";

    fn file(version: &str, installer: &str, notes: &str) -> String {
        serde_json::json!({"version": version, "installer": installer, "sha256": SHA, "notes": notes})
            .to_string()
    }

    #[test]
    fn a_release_file_is_read() {
        let text = file(
            "0.10.0",
            "https://files.ruxn.com/p/guesthouse/guesthouse-setup-0.10.0.exe",
            "https://github.com/ruxnlabs/guesthouse/releases/tag/windows-v0.10.0",
        );
        let latest = parse(&text).unwrap();
        assert_eq!(latest.version, "0.10.0");
        assert!(
            parse(&file("0.10.0", "https://files.ruxn.com/p/guesthouse/guesthouse-setup-0.10.0.exe", ""))
                .is_some()
        );
    }

    #[test]
    fn a_link_that_isnt_the_projects_own_is_never_offered() {
        for installer in [
            "https://example.com/guesthouse-setup-0.10.0.exe",
            "http://files.ruxn.com/p/guesthouse/guesthouse-setup-0.10.0.exe",
            "https://files.ruxn.com/p/guesthouse/../evil/x.exe",
            "https://files.ruxn.com/p/guesthouse/x.msi",
            "https://files.ruxn.com/p/guesthouse/a b.exe",
        ] {
            assert_eq!(parse(&file("0.10.0", installer, "")), None, "{installer}");
        }
        let notes = "https://example.com/notes";
        assert_eq!(parse(&file("0.10.0", "https://files.ruxn.com/p/guesthouse/x.exe", notes)), None);
        assert_eq!(parse(&file("0.10", "https://files.ruxn.com/p/guesthouse/x.exe", "")), None);
        assert_eq!(parse("not json"), None);
        assert_eq!(parse(&" ".repeat(LONGEST + 1)), None);
    }

    #[test]
    fn only_a_higher_version_is_newer() {
        assert!(newer("0.10.0", "0.9.0"), "by number, not by text");
        assert!(newer("1.0.0", "0.99.99"));
        assert!(!newer("0.9.0", "0.9.0"));
        assert!(!newer("0.8.5", "0.9.0"));
        assert!(!newer("0.10", "0.9.0"));
        assert!(!newer("0.10.0", "garbled"));
    }
}
