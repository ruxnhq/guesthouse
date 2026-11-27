//! The guest's copy of NVIDIA's driver, in its host driver store, at the version this PC gives a new
//! install. Programs in the guest load the driver from there, and Windows setup fills it once. So the
//! host notes which version each machine's guest has, and when the version it would give a new
//! install is another, the next start puts the driver on a disc in the VM, and `nv-store.ps1` in the
//! guest brings its copy to that version before the rest of the start uses the GPU.
//!
//! The version is the one in DriverVer of the driver's `nv_dispi.inf`, read on each side, so what the
//! host offers and what the guest has are compared the same way. A machine from before the note was
//! kept has none, and its first start gives the guest the disc: the script then finds the copy it has
//! is already that version, and the note is written.
//!
//! The decision and the guest's answers are plain functions with no OS call in them, so another host
//! can use them as they are.

use crate::context::Context;
use crate::failure::{Failure, Result};
use crate::gpu_maker::GpuMaker;
use crate::iso::iso_maker;
use crate::machine::Machine;
use crate::settings::driver_folder;
use crate::setup::driver::read_inf;
use crate::setup::inf::inf_driver_version;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// In the machine's folder: the version of NVIDIA's driver in its guest's host driver store.
pub const RECORD_FILE: &str = "guest-driver-version";
/// The folder the driver is in on the disc, as on the install disc.
pub const DISC_FOLDER: &str = "nvidia";
const DISC_LABEL: &str = "GPUPV_DRIVER";
const DISC_PREFIX: &str = "nvidia-driver-";
const DISC_SUFFIX: &str = ".iso";

/// The version a new install would give the guest, or `None` without the driver's files.
pub fn offered_version_in(folder: &Path) -> Option<String> {
    inf_driver_version(&read_inf(folder).ok()?).ok()
}

pub fn record_file(context: &Context, machine: &Machine) -> PathBuf {
    machine.folder(context).join(RECORD_FILE)
}

pub fn recorded_version(context: &Context, machine: &Machine) -> Option<String> {
    let text = fs::read_to_string(record_file(context, machine)).ok()?;
    Some(text.trim().to_owned()).filter(|version| !version.is_empty())
}

pub fn record_version(context: &Context, machine: &Machine, version: &str) -> Result<()> {
    fs::write(record_file(context, machine), format!("{version}\n"))?;
    Ok(())
}

/// The version to bring the guest to, when it is not the one noted. Another version, older or newer,
/// counts, as the driver this PC has is the one the daemon serves. No note counts too.
pub fn version_due<'a>(offered: Option<&'a str>, recorded: Option<&str>) -> Option<&'a str> {
    offered.filter(|offered| recorded != Some(*offered))
}

/// The same for a machine: only a Windows machine on an NVIDIA card has this copy to keep.
pub fn due(context: &Context, machine: &Machine) -> Option<String> {
    if machine.windows_maker() != Some(GpuMaker::Nvidia) {
        return None;
    }
    let offered = offered_version_in(&driver_folder(context));
    version_due(offered.as_deref(), recorded_version(context, machine).as_deref()).map(str::to_owned)
}

/// One disc per version, for every machine: it holds nothing of any machine's own.
pub fn disc_path(context: &Context, version: &str) -> PathBuf {
    context.paths.store.join("iso").join(format!("{DISC_PREFIX}{version}{DISC_SUFFIX}"))
}

fn is_driver_disc(name: &str) -> bool {
    name.starts_with(DISC_PREFIX) && name.ends_with(DISC_SUFFIX)
}

/// The disc with the driver of `folder` on it, made if there is none yet for its version. It is made
/// beside its place and only takes it at the end, and the discs of other versions go, as no start
/// gives a guest those any more.
pub fn make_disc(context: &Context, folder: &Path, version: &str) -> Result<PathBuf> {
    let path = disc_path(context, version);
    if path.is_file() {
        return Ok(path);
    }
    let parent = path.parent().ok_or_else(|| Failure::new("The driver disc has no folder."))?;
    fs::create_dir_all(parent)?;
    // Its own name, as two machines can start at once and make the same disc.
    let part = path.with_extension(format!("iso.{}.part", std::process::id()));
    let result = Command::new(iso_maker())
        .args(["-quiet", "-J", "-joliet-long", "-r", "-V", DISC_LABEL, "-graft-points", "-o"])
        .arg(&part)
        .arg(format!("{DISC_FOLDER}/={}", folder.display()))
        .output()?;
    if !result.status.success() {
        let _ = fs::remove_file(&part);
        let said = String::from_utf8_lossy(&result.stderr);
        let said = said.trim();
        let tail = said.char_indices().rev().nth(199).map_or(0, |found| found.0);
        return Err(Failure(format!("The driver disc could not be made: {}", &said[tail..])));
    }
    fs::rename(&part, &path)?;
    for entry in fs::read_dir(parent)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_driver_disc(&name) && entry.path() != path {
            let _ = fs::remove_file(entry.path());
        }
    }
    Ok(path)
}

/// What `nv-store.ps1` said it did, from its last line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreAnswer {
    /// The guest's copy is the version, now or from before. `restart` when the old copy could not be
    /// removed, so something may still have its files loaded.
    Current {
        version: String,
        restart: bool,
    },
    /// The copy took the old one's place just now.
    Replaced {
        version: String,
        restart: bool,
    },
    /// The change waits for the next boot, when Windows makes it before anything loads the driver.
    Queued,
    /// Still copying.
    Copying,
    NoDisc,
    /// Not enough room on the guest's drive, in the script's words.
    NoRoom(String),
    /// What went wrong, with the old copy left as it was.
    Failed(String),
}

impl StoreAnswer {
    /// Whether Windows has to start again before the guest runs the version through and through.
    pub fn restart(&self) -> bool {
        match self {
            Self::Current { restart, .. } | Self::Replaced { restart, .. } => *restart,
            Self::Queued => true,
            _ => false,
        }
    }

    /// The version the guest's copy is at, once it is there.
    pub fn version(&self) -> Option<&str> {
        match self {
            Self::Current { version, .. } | Self::Replaced { version, .. } => Some(version),
            _ => None,
        }
    }
}

const ANSWER_MARK: &str = "store ";
const RESTART_WORD: &str = " restart";

/// The script's last answer in `said`, if it gave one.
pub fn read_answer(said: &str) -> Option<StoreAnswer> {
    let line = said.lines().map(str::trim).rfind(|line| line.starts_with(ANSWER_MARK))?;
    let rest = &line[ANSWER_MARK.len()..];
    let versioned = |words: &str| {
        let (version, restart) = match words.strip_suffix(RESTART_WORD) {
            Some(version) => (version, true),
            None => (words, false),
        };
        (version.trim().to_owned(), restart)
    };
    if let Some(words) = rest.strip_prefix("current ") {
        let (version, restart) = versioned(words);
        return Some(StoreAnswer::Current { version, restart });
    }
    if let Some(words) = rest.strip_prefix("replaced ") {
        let (version, restart) = versioned(words);
        return Some(StoreAnswer::Replaced { version, restart });
    }
    Some(match rest {
        _ if rest.starts_with("queued") => StoreAnswer::Queued,
        "copying" => StoreAnswer::Copying,
        "no disc" => StoreAnswer::NoDisc,
        _ if rest.starts_with("no room") => StoreAnswer::NoRoom(
            rest.strip_prefix("no room").unwrap_or("").trim_start_matches(':').trim().to_owned(),
        ),
        _ => StoreAnswer::Failed(
            rest.strip_prefix("failed").unwrap_or(rest).trim_start_matches(':').trim().to_owned(),
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Scratch, scratch_context};

    #[test]
    fn a_version_is_due_when_the_note_says_another_or_nothing() {
        assert_eq!(version_due(Some("32.0.16.1800"), Some("32.0.15.9159")), Some("32.0.16.1800"));
        assert_eq!(
            version_due(Some("32.0.15.9159"), Some("32.0.16.1800")),
            Some("32.0.15.9159"),
            "older too"
        );
        assert_eq!(version_due(Some("32.0.16.1800"), None), Some("32.0.16.1800"), "no note yet");
        assert_eq!(version_due(Some("32.0.16.1800"), Some("32.0.16.1800")), None);
        assert_eq!(version_due(None, Some("32.0.16.1800")), None, "no driver on this PC, nothing to give");
    }

    #[test]
    fn only_a_windows_machine_on_nvidia_with_setups_driver_is_due() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let windows = Machine::new("w", "W", "/d", "/v");
        fs::create_dir_all(windows.folder(&context)).unwrap();
        assert_eq!(due(&context, &windows), None, "setup has no driver yet");
        let folder = driver_folder(&context);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("nv_dispi.inf"), "[Version]\nDriverVer = 01/01/2026, 32.0.16.1800\n").unwrap();
        assert_eq!(due(&context, &windows).as_deref(), Some("32.0.16.1800"));
        record_version(&context, &windows, "32.0.16.1800").unwrap();
        assert_eq!(due(&context, &windows), None);
        let mut intel = windows.clone();
        intel.use_windows_card(GpuMaker::Intel, "0000:00:02.0");
        fs::remove_file(record_file(&context, &windows)).unwrap();
        assert_eq!(due(&context, &intel), None, "another maker's machine keeps its own driver");
    }

    #[test]
    fn the_scripts_answers_are_read_from_its_last_line() {
        let read = |said: &str| read_answer(said).unwrap();
        assert_eq!(
            read("store copying\r\nstore replaced 32.0.16.1800\r\n"),
            StoreAnswer::Replaced { version: "32.0.16.1800".into(), restart: false }
        );
        let current = read("store current 32.0.16.1800 restart");
        assert_eq!(current, StoreAnswer::Current { version: "32.0.16.1800".into(), restart: true });
        assert!(current.restart());
        assert_eq!(read("store queued 32.0.16.1800"), StoreAnswer::Queued);
        assert!(read("store queued 32.0.16.1800").restart());
        assert_eq!(read("store copying"), StoreAnswer::Copying);
        assert_eq!(read("store no disc"), StoreAnswer::NoDisc);
        assert_eq!(
            read("store no room: the driver needs 2048 MB on C:\\, and it has 900 MB free"),
            StoreAnswer::NoRoom("the driver needs 2048 MB on C:\\, and it has 900 MB free".into())
        );
        assert_eq!(read("store failed: the copy stopped"), StoreAnswer::Failed("the copy stopped".into()));
        assert_eq!(read_answer("ERROR: no such file"), None);
        assert_eq!(read_answer(""), None);
    }

    #[test]
    fn the_disc_is_made_once_per_version_and_an_older_ones_goes() {
        if !crate::iso::any_iso_maker() {
            return;
        }
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let folder = scratch.path("driver");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("nv_dispi.inf"), "[Version]\nDriverVer = 01/01/2026, 32.0.16.1800\n").unwrap();
        let older = disc_path(&context, "32.0.15.9159");
        fs::create_dir_all(older.parent().unwrap()).unwrap();
        fs::write(&older, b"old").unwrap();
        let tools = older.with_file_name("tools.iso");
        fs::write(&tools, b"tools").unwrap();
        let made = make_disc(&context, &folder, "32.0.16.1800").unwrap();
        assert_eq!(made, disc_path(&context, "32.0.16.1800"));
        assert!(fs::metadata(&made).unwrap().len() > 0);
        assert!(!older.exists(), "no start gives a guest the older driver any more");
        assert!(tools.exists(), "another disc stays");
        let time = fs::metadata(&made).unwrap().modified().unwrap();
        make_disc(&context, &folder, "32.0.16.1800").unwrap();
        assert_eq!(fs::metadata(&made).unwrap().modified().unwrap(), time, "made once");
    }
}
