//! Where the service keeps its machines: a folder each, named after the machine, under one folder
//! only the system and administrators may open. A machine's folder holds its disks, the record and
//! the account `prepare` leaves beside them, and its settings, which say what it was made with.

use guesthouse_control::{Card, Share, valid_name};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// A machine's settings, in its folder.
const SETTINGS: &str = "machine.json";
/// The copy of a Linux guest's installer, kept until it has installed.
const INSTALLER: &str = "installer.iso";
/// A machine's events, a line each, and the log before it when this one grew too long.
pub const EVENTS: &str = "events.log";
pub const EVENTS_BEFORE: &str = "events.1.log";

/// What a machine was made with and is given at each boot, and what its guest last said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub memory_mb: u64,
    pub processors: u32,
    pub card: Card,
    /// The address the guest last reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// Whether an NVIDIA guest gets the NVAPI wrapper. Older machines have it, as it was on for all.
    #[serde(default = "on")]
    pub nvapi_wrapper: bool,
    /// A machine from before these had a TPM, a network and no share.
    #[serde(default = "on")]
    pub tpm: bool,
    #[serde(default = "on")]
    pub network: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<Share>,
    /// The largest its disk may grow to. A machine from before this has it read from its disk when
    /// the service starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_gb: Option<u64>,
}

fn on() -> bool {
    true
}

impl Settings {
    /// A machine's settings with only these, as tests and the oldest machines have them.
    pub fn basic(memory_mb: u64, processors: u32, card: Card) -> Self {
        Settings {
            memory_mb,
            processors,
            card,
            address: None,
            nvapi_wrapper: true,
            tpm: true,
            network: true,
            share: None,
            size_gb: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(root: PathBuf) -> Self {
        Store { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn folder(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    pub fn disk(&self, name: &str) -> PathBuf {
        self.folder(name).join(format!("{name}.vhdx"))
    }

    pub fn installer(&self, name: &str) -> PathBuf {
        self.folder(name).join(INSTALLER)
    }

    /// The file a machine's events are kept in, which is begun afresh when it grows long.
    pub fn events(&self, name: &str) -> PathBuf {
        self.folder(name).join(EVENTS)
    }

    /// The events before the current file's, which it is renamed to when it's full.
    pub fn events_before(&self, name: &str) -> PathBuf {
        self.folder(name).join(EVENTS_BEFORE)
    }

    /// Every machine with settings, by name. A folder that isn't a machine's, such as one left by
    /// making a machine that failed, or a link to somewhere else, is passed over.
    pub fn load(&self) -> Vec<(String, Settings)> {
        let Ok(entries) = fs::read_dir(&self.root) else { return Vec::new() };
        let mut machines: Vec<(String, Settings)> = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir() && !kind.is_symlink()))
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| usable_name(name))
            .filter_map(|name| {
                let text = fs::read_to_string(self.folder(&name).join(SETTINGS)).ok()?;
                Some((name, serde_json::from_str(&text).ok()?))
            })
            .collect();
        machines.sort_by(|a, b| a.0.cmp(&b.0));
        machines
    }

    pub fn save(&self, name: &str, settings: &Settings) -> Result<(), String> {
        let path = self.folder(name).join(SETTINGS);
        let text = serde_json::to_string_pretty(settings).expect("settings always serialise");
        fs::write(&path, text + "\n").map_err(|error| format!("{}: {error}", path.display()))
    }

    /// Makes a new machine's folder, which must not be there yet.
    pub fn make(&self, name: &str) -> Result<(), String> {
        let folder = self.folder(name);
        fs::create_dir(&folder).map_err(|error| format!("{}: {error}", folder.display()))
    }

    /// The guest's administrator and password, which `prepare` keeps beside its disk.
    pub fn account(&self, name: &str) -> Result<(String, String), String> {
        let path = self.disk(name).with_extension("account.txt");
        let text = fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        parse_account(&text).ok_or_else(|| format!("{} doesn't say a user and a password.", path.display()))
    }

    /// Deletes a machine's folder and everything in it. A link in its place is refused, not followed.
    pub fn remove(&self, name: &str) -> Result<(), String> {
        let folder = self.folder(name);
        let kind = fs::symlink_metadata(&folder).map_err(|error| format!("{}: {error}", folder.display()))?;
        if !kind.is_dir() || kind.file_type().is_symlink() {
            return Err(format!("{} isn't a folder of the service's own, so it's left.", folder.display()));
        }
        fs::remove_dir_all(&folder).map_err(|error| format!("{}: {error}", folder.display()))
    }
}

/// Whether `name` can name a machine: `guesthouse-control`'s rule, and not a name Windows keeps for
/// a device, which no folder can have.
pub fn usable_name(name: &str) -> bool {
    const DEVICES: [&str; 4] = ["con", "prn", "aux", "nul"];
    let lower = name.to_ascii_lowercase();
    let numbered_port = ["com", "lpt"].iter().any(|port| {
        lower.strip_prefix(port).is_some_and(|rest| rest.len() == 1 && rest.as_bytes()[0].is_ascii_digit())
    });
    valid_name(name) && !DEVICES.contains(&lower.as_str()) && !numbered_port
}

/// The user and password in the account `prepare` writes: `user: NAME` and `password: WORDS` lines.
pub fn parse_account(text: &str) -> Option<(String, String)> {
    let field =
        |key: &str| text.lines().find_map(|line| line.strip_prefix(key)).map(|value| value.trim().to_owned());
    Some((field("user:")?, field("password:")?))
        .filter(|(user, password)| !user.is_empty() && !password.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(test: &str) -> Store {
        let root = std::env::temp_dir().join(format!("gh-store-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Store::new(root)
    }

    fn settings() -> Settings {
        Settings::basic(8192, 4, Card::Default)
    }

    #[test]
    fn a_machine_from_before_its_switches_has_a_tpm_and_a_network() {
        let old: Settings =
            serde_json::from_str(r#"{"memory_mb": 8192, "processors": 4, "card": {"card": "default"}}"#)
                .unwrap();
        assert_eq!(old, settings());
    }

    #[test]
    fn a_machine_reads_back_as_it_was_saved() {
        let store = store("save");
        store.make("ubuntu").unwrap();
        let saved = Settings { address: Some("172.29.1.2".into()), ..settings() };
        store.save("ubuntu", &saved).unwrap();
        assert_eq!(store.load(), [("ubuntu".to_owned(), saved)]);
        assert!(store.make("ubuntu").is_err(), "a machine's folder is made once");
        let _ = fs::remove_dir_all(store.root());
    }

    #[test]
    fn a_folder_without_settings_is_no_machine() {
        let store = store("bare");
        store.make("half").unwrap();
        fs::create_dir(store.root().join("not a name")).unwrap();
        assert!(store.load().is_empty());
        let _ = fs::remove_dir_all(store.root());
    }

    #[test]
    fn a_removed_machine_is_gone_with_its_disks() {
        let store = store("remove");
        store.make("win").unwrap();
        store.save("win", &settings()).unwrap();
        fs::write(store.disk("win"), b"disk").unwrap();
        store.remove("win").unwrap();
        assert!(!store.folder("win").exists());
        assert!(store.remove("win").is_err());
        let _ = fs::remove_dir_all(store.root());
    }

    #[test]
    fn the_account_is_read_as_prepare_writes_it() {
        assert_eq!(parse_account("user: guesthouse\npassword: x\n"), Some(("guesthouse".into(), "x".into())));
        assert_eq!(parse_account("user: guesthouse\n"), None);
        let store = store("account");
        store.make("win").unwrap();
        fs::write(store.disk("win").with_extension("account.txt"), "user: guesthouse\npassword: p\n")
            .unwrap();
        assert_eq!(store.account("win").unwrap(), ("guesthouse".into(), "p".into()));
        let _ = fs::remove_dir_all(store.root());
    }

    #[test]
    fn device_names_are_no_machines_names() {
        assert!(usable_name("ubuntu-2"));
        assert!(usable_name("console"));
        assert!(usable_name("com10"));
        for name in ["con", "NUL", "Com1", "lpt9", "../x", ""] {
            assert!(!usable_name(name), "{name}");
        }
    }
}
