//! Whether a newer Windows host is out, from the small file a release publishes beside its installer,
//! `guesthouse-windows-latest.json`, as `scripts/windows-release.ps1` writes it. The app fetches it
//! and offers the installer; this decides what it says, so it's tested anywhere.
//!
//! The file is taken only when a release key signed it: its `signature` is an Ed25519 signature of
//! its version, installer, SHA-256 and notes (`guesthouse_update_check::signed`) by a key whose public
//! half is in [`RELEASE_KEYS`], the release keys every host shares. The installer is then checked against that SHA-256 before it runs, so
//! what runs is what the release key vouched for, wherever the file and the installer were served
//! from. Apps from before the signature read the same file and pass over the field.

use guesthouse_update_check::is_version;
pub use guesthouse_update_check::signed;
pub use guesthouse_update_check::signed::RELEASE_KEYS;
use serde::Deserialize;

/// Where a release publishes the file.
pub const LATEST_URL: &str = "https://files.ruxn.com/p/guesthouse/guesthouse-windows-latest.json";
/// Where an installer is published. A link anywhere else in the file is never offered.
const INSTALLERS: &str = "https://files.ruxn.com/p/guesthouse/";
/// Where a release's notes are.
const NOTES: &str = "https://github.com/ruxnlabs/guesthouse/releases/";
/// The longest file taken. A real one is a few hundred bytes.
pub const LONGEST: usize = 4096;
/// What the signature says it is for, so a signature made for any other file is never taken as one of
/// these.
pub const PURPOSE: &str = "guesthouse-windows-latest.json";

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
    /// A release key's signature of the fields above, in hex.
    #[serde(default)]
    pub signature: String,
}

impl Latest {
    /// The fields the signature is of, in order.
    fn signed_fields(&self) -> [&str; 4] {
        [&self.version, &self.installer, &self.sha256, &self.notes]
    }

    /// The signature of this file's fields by the key whose private half is `seed`, in hex, as a
    /// release writes into the file.
    pub fn sign(&self, seed: &str) -> Result<String, String> {
        signed::sign(seed, PURPOSE, &self.signed_fields())
    }
}

/// The newest Windows host, from the file's text, if the file is one a release writes and one of
/// `keys` (public halves, in hex: [`RELEASE_KEYS`] in the app) signed it. Its links have to be the
/// project's own, as the app opens them.
pub fn parse(text: &str, keys: &[&str]) -> Option<Latest> {
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
    let signed = signed::verify(keys, PURPOSE, &latest.signed_fields(), &latest.signature);
    (is_version(&latest.version) && installer_ok && notes_ok && sha_ok && signed).then_some(latest)
}

pub use guesthouse_update_check::newer;

#[cfg(test)]
mod tests {
    use super::*;
    use guesthouse_update_check::signed::new_key;

    const SHA: &str = "41a84090e83b778e0314844eb8a403010d11d4d841b2fb0f31d63f1c1efbf08e";

    /// A file as a release writes it, signed with `seed`.
    fn signed_file(seed: &str, version: &str, installer: &str, notes: &str) -> String {
        let mut latest = Latest {
            version: version.to_owned(),
            installer: installer.to_owned(),
            sha256: SHA.to_owned(),
            notes: notes.to_owned(),
            signature: String::new(),
        };
        latest.signature = latest.sign(seed).unwrap();
        serde_json::to_string_pretty(&serde_json::json!({
            "version": latest.version, "installer": latest.installer, "sha256": latest.sha256,
            "notes": latest.notes, "signature": latest.signature,
        }))
        .unwrap()
    }

    /// The same, signed with a key made for the test, and that key's public half.
    fn file(version: &str, installer: &str, notes: &str) -> (String, String) {
        let (seed, public) = new_key().unwrap();
        (signed_file(&seed, version, installer, notes), public)
    }

    const INSTALLER: &str = "https://files.ruxn.com/p/guesthouse/guesthouse-setup-0.10.0.exe";
    const RELEASE: &str = "https://github.com/ruxnlabs/guesthouse/releases/tag/windows-v0.10.0";

    #[test]
    fn a_release_file_is_read() {
        let (text, key) = file("0.10.0", INSTALLER, RELEASE);
        let latest = parse(&text, &[&key]).unwrap();
        assert_eq!(latest.version, "0.10.0");
        assert_eq!(latest.sha256, SHA);
        let (text, key) = file("0.10.0", INSTALLER, "");
        assert!(parse(&text, &[&key]).is_some());
    }

    #[test]
    fn a_file_no_release_key_signed_is_never_offered() {
        let (text, key) = file("0.10.0", INSTALLER, RELEASE);
        let (_, other) = new_key().unwrap();
        assert_eq!(parse(&text, &[&other]), None, "signed by another key");
        assert_eq!(parse(&text, &[]), None, "no key built in");

        let mut fields: serde_json::Value = serde_json::from_str(&text).unwrap();
        fields.as_object_mut().unwrap().remove("signature");
        assert_eq!(parse(&fields.to_string(), &[&key]), None, "no signature");

        // A field changed after signing, each in turn.
        let other_sha = "00".repeat(32);
        for (field, value) in [
            ("version", "0.10.1"),
            ("installer", "https://files.ruxn.com/p/guesthouse/other-0.10.0.exe"),
            ("sha256", other_sha.as_str()),
            ("notes", "https://github.com/ruxnlabs/guesthouse/releases/tag/windows-v0.10.1"),
        ] {
            let mut changed: serde_json::Value = serde_json::from_str(&text).unwrap();
            changed[field] = value.into();
            assert_eq!(parse(&changed.to_string(), &[&key]), None, "{field} changed");
        }

        // A signature of the same fields made for another file.
        let (seed, public) = new_key().unwrap();
        let mut latest = parse(&signed_file(&seed, "0.10.0", INSTALLER, RELEASE), &[&public]).unwrap();
        latest.signature =
            signed::sign(&seed, "guesthouse-linux-driver-latest.json", &latest.signed_fields()).unwrap();
        let text = serde_json::json!({
            "version": latest.version, "installer": latest.installer, "sha256": latest.sha256,
            "notes": latest.notes, "signature": latest.signature,
        });
        assert_eq!(parse(&text.to_string(), &[&public]), None, "another file's signature");
    }

    #[test]
    fn a_signed_file_keeps_the_fields_apps_without_the_signature_read() {
        /// The file as apps from before the signature read it.
        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct Before {
            version: String,
            installer: String,
            sha256: String,
            #[serde(default)]
            notes: String,
        }
        let (text, _) = file("0.10.0", INSTALLER, RELEASE);
        assert!(text.len() <= LONGEST);
        let before: Before = serde_json::from_str(&text).unwrap();
        assert_eq!((before.version.as_str(), before.sha256.as_str()), ("0.10.0", SHA));
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
            let (text, key) = file("0.10.0", installer, "");
            assert_eq!(parse(&text, &[&key]), None, "{installer}");
        }
        let (text, key) =
            file("0.10.0", "https://files.ruxn.com/p/guesthouse/x.exe", "https://example.com/notes");
        assert_eq!(parse(&text, &[&key]), None);
        let (text, key) = file("0.10", "https://files.ruxn.com/p/guesthouse/x.exe", "");
        assert_eq!(parse(&text, &[&key]), None);
        assert_eq!(parse("not json", &[&key]), None);
        assert_eq!(parse(&" ".repeat(LONGEST + 1), &[&key]), None);
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
