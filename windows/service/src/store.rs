//! Where the service keeps its machines: a folder each, named after the machine, under one folder
//! only the system and administrators may open. A machine's folder holds its settings, which say what
//! it was made with, and its events. Its disks, the record and the account `prepare` leaves beside
//! them are in that folder too, unless the machine was given another place: then they're in a folder
//! of the machine's name there, and a small file in the machine's own folder says where.
//!
//! The place is somewhere a user chose, so the service holds it open while it runs, which stops it
//! being renamed away and replaced, and trusts it only if the service's own check of it passes. A
//! place that fails the check, or isn't there, is a problem the machine is listed with.

use guesthouse_control::{Card, Forward, NetworkMode, Share, valid_name};
use guesthouse_remote_view_core::switch::SWITCH_FILE;
use serde::{Deserialize, Serialize};
use std::any::Any;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

/// A machine's settings, in its folder.
const SETTINGS: &str = "machine.json";
/// Where a machine's disks are, in its folder, if it isn't where it's kept.
const LOCATION: &str = "location.json";
/// The copy of a Linux guest's installer, kept until it has installed.
const INSTALLER: &str = "installer.iso";
/// A machine's events, a line each, and the log before it when this one grew too long.
pub const EVENTS: &str = "events.log";
pub const EVENTS_BEFORE: &str = "events.1.log";
/// What stays in a machine's own folder when its disks are somewhere else: the service's own files,
/// its remote view's switch among them, which are never moved or counted as disks.
pub const OWN_FILES: [&str; 5] = [SETTINGS, LOCATION, EVENTS, EVENTS_BEFORE, SWITCH_FILE];

/// Something kept for as long as it's held, such as a folder opened so it can't be moved.
pub type Held = Box<dyn Any + Send>;

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
    /// The folders it shares. A machine from before there could be more than one has `share`, one
    /// or none.
    #[serde(
        default,
        alias = "share",
        deserialize_with = "guesthouse_control::shares_or_share",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub shares: Vec<Share>,
    /// Which network its card is plugged into. A machine from before bridging has the shared one.
    #[serde(default, skip_serializing_if = "is_shared")]
    pub network_mode: NetworkMode,
    /// The ports of this PC passed on to the guest's while it runs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forwards: Vec<Forward>,
    /// Whether the guest may run virtual machines of its own. A machine from before this has it off.
    #[serde(default)]
    pub nested_virtualization: bool,
    /// The port on this PC's loopback that reaches an Ubuntu guest's SSH. It's chosen once, kept, and
    /// never shared with another machine, and chosen again only if it's busy at a start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_port: Option<u16>,
    /// The largest its disk may grow to. A machine from before this has it read from its disk when
    /// the service starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_gb: Option<u64>,
    /// The account, as a SID, of the user whose machine it is: the one who made it. A machine from
    /// before this has none until a user takes it (see `requests::owners`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}

fn on() -> bool {
    true
}

fn is_shared(mode: &NetworkMode) -> bool {
    *mode == NetworkMode::Shared
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
            shares: Vec::new(),
            network_mode: NetworkMode::Shared,
            forwards: Vec::new(),
            nested_virtualization: false,
            ssh_port: None,
            size_gb: None,
            owner: None,
        }
    }
}

/// Opens a folder a user chose to keep a machine in, if it's fit to: a folder of its own that the
/// system and administrators made, not a link and not reached through one. What it returns keeps it
/// from being renamed or deleted.
pub type Hold = Arc<dyn Fn(&Path) -> Result<Held, String> + Send + Sync>;

/// Where a machine's disks are, and what holds the place.
struct Place {
    folder: PathBuf,
    /// Why the place can't be used, if it can't.
    held: Result<Held, String>,
}

#[derive(Clone)]
pub struct Store {
    root: PathBuf,
    hold: Hold,
    /// The machines that have a place other than their own folder, by name.
    places: Arc<Mutex<BTreeMap<String, Place>>>,
    /// Held while a place is read, held again or changed, so those don't run over each other.
    changing: Arc<Mutex<()>>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").field("root", &self.root).finish_non_exhaustive()
    }
}

/// What a machine's own folder says of where its disks are.
#[derive(Debug, Serialize, Deserialize)]
struct Location {
    folder: PathBuf,
}

impl Store {
    pub fn new(root: PathBuf) -> Self {
        Store { root, hold: Arc::new(|_| Ok(Box::new(()))), places: Arc::default(), changing: Arc::default() }
    }

    /// The same, with `hold` to say whether a place is fit to keep a machine in.
    pub fn with_hold(mut self, hold: Hold) -> Self {
        self.hold = hold;
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn places(&self) -> MutexGuard<'_, BTreeMap<String, Place>> {
        self.places.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A machine's own folder, where its settings and events are. It's always under the root.
    pub fn folder(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    /// The folder a machine's disks are in: its own, or the place it was given. A place that can't be
    /// trusted now is nowhere, so what's read or written there is a path nothing is at, and the
    /// service never touches a folder it isn't holding.
    pub fn place(&self, name: &str) -> PathBuf {
        match self.places().get(name) {
            None => self.folder(name),
            Some(place) if place.held.is_ok() => place.folder.clone(),
            Some(_) => self.folder(name).join("(unusable)"),
        }
    }

    /// The folder a machine's own folder says its disks are in, whether it can be trusted or not.
    pub fn recorded(&self, name: &str) -> PathBuf {
        self.places().get(name).map_or_else(|| self.folder(name), |place| place.folder.clone())
    }

    /// Why a machine's place can't be used now, if it can't.
    pub fn problem(&self, name: &str) -> Option<String> {
        self.places().get(name).and_then(|place| place.held.as_ref().err().cloned())
    }

    pub fn disk(&self, name: &str) -> PathBuf {
        self.place(name).join(format!("{name}.vhdx"))
    }

    pub fn installer(&self, name: &str) -> PathBuf {
        self.place(name).join(INSTALLER)
    }

    /// The file a machine's events are kept in, which is begun afresh when it grows long.
    pub fn events(&self, name: &str) -> PathBuf {
        self.folder(name).join(EVENTS)
    }

    /// The events before the current file's, which it is renamed to when it's full.
    pub fn events_before(&self, name: &str) -> PathBuf {
        self.folder(name).join(EVENTS_BEFORE)
    }

    /// Opens `folder` as a place for a machine's disks, as the service's check of it says it may be.
    pub fn hold(&self, folder: &Path) -> Result<Held, String> {
        if !folder.is_dir() {
            return Err(format!(
                "{} isn't there. If it's on a drive that's off or gone, the machine is back when the drive is.",
                folder.display()
            ));
        }
        (self.hold)(folder)
    }

    /// Reads where a machine's own folder says its disks are, and holds the place, if it has one.
    /// A place that can't be held is kept as the machine's problem, which `retry` tries again.
    fn attach(&self, name: &str) {
        let _one = self.changing.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let path = self.folder(name).join(LOCATION);
        let broken = |why: String| Place { folder: self.folder(name), held: Err(why) };
        let place = match fs::read_to_string(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => Some(broken(format!("{}: {error}", path.display()))),
            Ok(text) => Some(match serde_json::from_str::<Location>(&text) {
                Ok(location) => Place { held: self.hold(&location.folder), folder: location.folder },
                Err(error) => broken(format!("{}: {error}", path.display())),
            }),
        };
        match place {
            Some(place) => self.places().insert(name.to_owned(), place),
            None => self.places().remove(name),
        };
    }

    /// Reads and holds again the place of a machine that has one, as it stands now. A place that was
    /// fine may be gone, as a drive is, or be another drive's at the same letter, and one that
    /// couldn't be held may be back. The new hold is taken before the old is let go.
    pub fn refresh(&self, name: &str) {
        if self.places().contains_key(name) {
            self.attach(name);
        }
    }

    /// Makes `folder` the place of the machine `name`, whose own folder is made. The folder is held
    /// first, and the record written before the place is used, so a machine whose record isn't kept
    /// is where it was.
    pub fn locate(&self, name: &str, folder: &Path) -> Result<(), String> {
        let _one = self.changing.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let held = self.hold(folder)?;
        self.write_location(name, Some(folder))?;
        self.places().insert(name.to_owned(), Place { folder: folder.to_owned(), held: Ok(held) });
        Ok(())
    }

    /// Makes `folder`, held as `held`, the place of the machine `name` from now on, and gives back
    /// the place it had and what held it, and takes `held` only if it does. A folder that is the machine's own folder is the default,
    /// which has no record. The record is replaced in one step, so a machine is always in one place.
    pub fn switch(
        &self,
        name: &str,
        folder: &Path,
        held: &mut Option<Held>,
    ) -> Result<(PathBuf, Result<Held, String>), String> {
        let _one = self.changing.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let own = folder == self.folder(name);
        self.write_location(name, if own { None } else { Some(folder) })?;
        // Only now is the hold taken from the caller, who keeps it if the record couldn't be written.
        let held = held.take().expect("a hold is given to switch");
        let before = self.recorded(name);
        let old = if own {
            self.places().remove(name).map(|place| place.held)
        } else {
            self.places()
                .insert(name.to_owned(), Place { folder: folder.to_owned(), held: Ok(held) })
                .map(|place| place.held)
        };
        Ok((before, old.unwrap_or_else(|| Ok(Box::new(())))))
    }

    /// Writes where a machine's disks are, through a file of its own renamed over the record, or
    /// takes the record away for the machine's own folder.
    fn write_location(&self, name: &str, folder: Option<&Path>) -> Result<(), String> {
        let path = self.folder(name).join(LOCATION);
        let Some(folder) = folder else {
            return match fs::remove_file(&path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    Err(format!("{}: {error}", path.display()))
                }
                _ => Ok(()),
            };
        };
        let new = self.folder(name).join("location.new.json");
        let text = serde_json::to_string_pretty(&Location { folder: folder.to_owned() })
            .expect("a location always serialises");
        fs::write(&new, text + "\n").map_err(|error| format!("{}: {error}", new.display()))?;
        fs::rename(&new, &path).map_err(|error| format!("{}: {error}", path.display()))
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
                let settings = serde_json::from_str(&text).ok()?;
                // Its place is found for every machine that's listed, a missing one included.
                self.attach(&name);
                Some((name, settings))
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

    /// Deletes a machine's folder and everything in it, and its disks where they are. A link in
    /// place of either is refused, not followed. Disks in a place that can't be held are left, as the
    /// service can't tell what's there now.
    pub fn remove(&self, name: &str) -> Result<(), String> {
        let _one = self.changing.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let folder = self.folder(name);
        let kind = fs::symlink_metadata(&folder).map_err(|error| format!("{}: {error}", folder.display()))?;
        if !kind.is_dir() || kind.file_type().is_symlink() {
            return Err(format!("{} isn't a folder of the service's own, so it's left.", folder.display()));
        }
        if let Some(place) = self.places().get_mut(name)
            && place.held.is_ok()
        {
            // Emptied while it's held, and only then let go and taken away itself, so what's removed
            // is what was checked.
            empty(&place.folder)?;
            place.held = Err("It's being deleted.".into());
            fs::remove_dir(&place.folder).map_err(|error| format!("{}: {error}", place.folder.display()))?;
        }
        fs::remove_dir_all(&folder).map_err(|error| format!("{}: {error}", folder.display()))?;
        self.places().remove(name);
        Ok(())
    }
}

/// Deletes what's in `folder`, and not the folder.
pub fn empty(folder: &Path) -> Result<(), String> {
    let entries = fs::read_dir(folder).map_err(|error| format!("{}: {error}", folder.display()))?;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        // A folder that is a link is removed as the link, not followed.
        let gone = match entry.file_type() {
            Ok(kind) if kind.is_dir() => fs::remove_dir_all(&path),
            _ => fs::remove_file(&path),
        };
        gone.map_err(|error| format!("{}: {error}", path.display()))?;
    }
    Ok(())
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
    fn a_machines_network_and_forwards_are_kept_and_a_machine_without_them_has_none() {
        let store = store("network");
        store.make("ubuntu").unwrap();
        let saved = Settings {
            network_mode: NetworkMode::Bridged { adapter: "Ethernet".into() },
            forwards: vec![Forward { host_port: 8080, guest_port: 80 }],
            ..settings()
        };
        store.save("ubuntu", &saved).unwrap();
        assert_eq!(store.load(), [("ubuntu".to_owned(), saved)]);
        let plain = serde_json::to_string(&settings()).unwrap();
        assert!(!plain.contains("network_mode") && !plain.contains("forwards"), "{plain}");
        let _ = fs::remove_dir_all(store.root());
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
        // The account rules are for a new name only: a machine's own account is read whatever it is.
        for user in ["system", "Administrator", "a@b", &"x".repeat(40)] {
            let text = format!("user: {user}\npassword: x\n");
            assert_eq!(parse_account(&text), Some((user.to_owned(), "x".into())), "{user}");
        }
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
