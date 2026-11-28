//! Whether a newer Windows host is out, and updating to it from inside the app, as the app's
//! Settings page shows it, with the yellow dot on its Settings button. This is the plain logic: what
//! each thing that happens changes, and what's to be done next, with nothing of Windows or the
//! network in it, so its tests run on any system. The app does what it's told to through an
//! [`Updater`]; the real one is in system.rs, and [`Fake`] stands in for it in the tests and the
//! preview. `files.rs` holds the rest of the plain logic: the files' names, the hash, and curl's
//! words and answers.
//!
//! What the file drop's file says to this version is decided in `guesthouse_control::updates`, on
//! the check every host shares (`guesthouse-update-check`): newer, the newest, or can't tell. Only
//! an installer at the file drop's own address, as that check has it, is ever downloaded.
//!
//! The rules it keeps:
//!
//! - It looks by itself, and downloads by itself if the settings say to, but never installs by
//!   itself: only the person's Install does.
//! - A download is checked against the SHA-256 the same latest file gave before it's ready, and a
//!   file that doesn't match is deleted. Nothing kept from an earlier run of the app is ever
//!   trusted: every file is downloaded and checked again in this one.
//! - Each download, check and install is numbered, and an answer to one that's been overtaken (as
//!   by Cancel, or a second look) changes nothing.
//! - Nothing is looked for while a download, a check or an install goes on, or while one is ready.

// Off Windows only the tests use these.
#![cfg_attr(not(windows), allow(dead_code))]

pub mod files;

use guesthouse_control::updates::{self, Latest};
use guesthouse_control::{Machine, State};

pub use files::Failed as DownloadError;

/// Why a look didn't say.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CantTell {
    /// The file drop couldn't be reached: no network, or it didn't answer in time.
    Unreached,
    /// It answered with something that isn't a file a release writes.
    Garbled,
}

impl CantTell {
    /// What the person is told, which says what to do.
    pub fn words(&self) -> &'static str {
        match self {
            CantTell::Unreached => {
                "files.ruxn.com couldn't be reached. Check this PC's network, then look again."
            }
            CantTell::Garbled => {
                "files.ruxn.com answered with something that isn't a release's file. Look again later."
            }
        }
    }
}

/// Why an update didn't get as far as it was asked to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Failure {
    /// The download stopped.
    Download(DownloadError),
    /// The file's SHA-256 wasn't the one the latest file gave.
    Mismatch,
    /// The file couldn't be read to check it.
    Unreadable(String),
    /// The installer didn't start.
    Launch(String),
    /// The installer ended with nothing done: Windows' prompt was answered No, or its pages were
    /// closed before it installed.
    Declined,
    /// The installer ended with an error.
    Installer,
}

impl Failure {
    /// The line that heads it.
    pub fn heading(&self, version: &str) -> String {
        match self {
            Failure::Download(_) | Failure::Mismatch | Failure::Unreadable(_) => {
                "The update wasn't downloaded".to_owned()
            }
            Failure::Launch(_) | Failure::Declined | Failure::Installer => {
                format!("Guesthouse {version} wasn't installed")
            }
        }
    }

    /// What the person is told: what happened, and that nothing was installed.
    pub fn words(&self) -> String {
        match self {
            Failure::Download(failed) => format!("{} Nothing was installed.", failed.words()),
            Failure::Mismatch => {
                "It didn't match its SHA-256, so it was deleted. Nothing was installed.".to_owned()
            }
            Failure::Unreadable(why) => {
                format!("It couldn't be read to check it ({why}), so it was deleted. Nothing was installed.")
            }
            Failure::Launch(why) => format!("The installer didn't start: {why}."),
            Failure::Declined => {
                "Windows' prompt was answered No, or the installer was closed before it installed.".to_owned()
            }
            Failure::Installer => {
                "The installer stopped with an error. Its log, guesthouse-setup.log, is in your Temp folder."
                    .to_owned()
            }
        }
    }
}

/// Where the looking and updating stand.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Phase {
    /// A build of the repo, which is whatever it was built from, and never looks.
    NotInstalled,
    /// Not looked yet since the app started.
    NotLooked,
    Looking,
    /// This is the newest, as the file said at `looked`, in seconds since 1970.
    Newest {
        looked: u64,
    },
    /// A newer one is out.
    Newer {
        latest: Latest,
        looked: u64,
    },
    CantTell {
        why: CantTell,
        looked: u64,
    },
    /// Its installer is being downloaded: `got` bytes so far, of `total` when the file drop said.
    Downloading {
        latest: Latest,
        got: u64,
        total: Option<u64>,
    },
    /// The download is being checked against its SHA-256.
    Verifying {
        latest: Latest,
    },
    /// Downloaded and checked, for the person to install, with why the last try to didn't, if one
    /// didn't.
    Ready {
        latest: Latest,
        note: Option<Failure>,
    },
    /// The installer has been started, and hasn't ended.
    Installing {
        latest: Latest,
    },
    /// The installer ended having installed it, while this app was still open.
    Installed {
        latest: Latest,
    },
    /// The download or its check failed, and the file is gone.
    Failed {
        latest: Latest,
        failure: Failure,
    },
}

/// What a fetch of the latest file came back with.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Fetched {
    Text(String),
    Unreached,
}

/// What the installer's starter ended with: done, failed, or cancelled (as `guesthouse-setup` says,
/// windows/setup/src/ui.rs).
pub const INSTALLER_DONE: i32 = 0;
pub const INSTALLER_CANCELLED: i32 = 2;

/// What happened, which the app hands to [`Updates::step`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Input {
    /// Look now, as the person asked, or as the app does by itself.
    Look,
    /// The fetch started as `attempt` came back.
    Looked { attempt: u64, fetched: Fetched },
    /// Put the newer one aside until the app next starts: the dot goes, the page still says.
    PutAside,
    /// Download the newer one, as the person asked, or again after a failure.
    Download,
    /// So much of download `attempt` has come.
    Progress { attempt: u64, got: u64, total: Option<u64> },
    /// Download `attempt` ended.
    Downloaded { attempt: u64, result: Result<(), DownloadError> },
    /// Stop the download, or its check.
    Cancel,
    /// The check of download `attempt` read the file and found this SHA-256, in hex, or couldn't.
    Hashed { attempt: u64, result: Result<String, String> },
    /// Install the one that's ready, as the person asked, having been told what it stops.
    Install,
    /// The installer of install `attempt` started, or didn't.
    Launched { attempt: u64, result: Result<(), String> },
    /// The installer of install `attempt` ended with this code.
    InstallerEnded { attempt: u64, code: i32 },
    /// The service stopped answering, as the installer stops it.
    ServiceGone,
}

/// What the app is to do next, for the step that asked.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Effect {
    /// Fetch the latest file, and hand back what came as `Input::Looked` with this attempt.
    Fetch { attempt: u64 },
    /// Download `latest`'s installer into the app's own folder for updates, telling how far it's got
    /// as `Input::Progress`, and how it ended as `Input::Downloaded`.
    Download { attempt: u64, latest: Latest },
    /// Stop the download that's going on.
    StopDownload,
    /// Read the downloaded file, held so nothing can change it from now until it's run, and hand
    /// back its SHA-256 as `Input::Hashed`.
    Hash { attempt: u64 },
    /// Let go of the downloaded file, and delete it and anything else in the folder.
    Discard,
    /// Start the installer, as the person, so Windows asks for an administrator, and hand back
    /// `Input::Launched`, then `Input::InstallerEnded`.
    Launch { attempt: u64, latest: Latest },
    /// Close the app, as the installer is about to replace it.
    Close,
}

/// The updates' state.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Updates {
    pub phase: Phase,
    /// Counts each fetch, download, check and install, so an answer to one that's been overtaken
    /// changes nothing.
    attempt: u64,
    /// The newer one was put aside, so the dot doesn't show.
    pub put_aside: bool,
    /// Download a newer one as soon as a look finds it, as the settings say.
    pub download_by_itself: bool,
    /// This app's version.
    current: String,
}

impl Updates {
    /// An installed app of version `current`, or a build of the repo when not `installed`.
    pub fn new(installed: bool, current: &str) -> Updates {
        Updates {
            phase: if installed { Phase::NotLooked } else { Phase::NotInstalled },
            attempt: 0,
            put_aside: false,
            download_by_itself: false,
            current: current.to_owned(),
        }
    }

    /// Takes in what happened at `now`, in seconds since 1970, and says what's to be done next.
    pub fn step(&mut self, input: Input, now: u64) -> Vec<Effect> {
        match input {
            Input::Look => {
                if !self.can_look() {
                    return Vec::new();
                }
                self.phase = Phase::Looking;
                vec![Effect::Fetch { attempt: self.next() }]
            }
            Input::Looked { attempt, fetched } => {
                if attempt != self.attempt || self.phase != Phase::Looking {
                    return Vec::new();
                }
                self.phase = self.judge(fetched, now);
                if self.download_by_itself && matches!(self.phase, Phase::Newer { .. }) {
                    return self.download();
                }
                Vec::new()
            }
            Input::PutAside => {
                self.put_aside = true;
                Vec::new()
            }
            Input::Download => self.download(),
            Input::Progress { attempt, got, total } => {
                if attempt == self.attempt
                    && let Phase::Downloading { got: shown, total: whole, .. } = &mut self.phase
                {
                    *shown = got.max(*shown);
                    *whole = total.or(*whole);
                }
                Vec::new()
            }
            Input::Downloaded { attempt, result } => {
                let Phase::Downloading { latest, .. } = &self.phase else { return Vec::new() };
                if attempt != self.attempt {
                    return Vec::new();
                }
                let latest = latest.clone();
                match result {
                    Ok(()) => {
                        self.phase = Phase::Verifying { latest };
                        vec![Effect::Hash { attempt }]
                    }
                    Err(failed) => {
                        self.phase = Phase::Failed { latest, failure: Failure::Download(failed) };
                        vec![Effect::Discard]
                    }
                }
            }
            Input::Cancel => {
                let latest = match &self.phase {
                    Phase::Downloading { latest, .. } | Phase::Verifying { latest } => latest.clone(),
                    _ => return Vec::new(),
                };
                let downloading = matches!(self.phase, Phase::Downloading { .. });
                // What the overtaken download or check says after this changes nothing.
                self.next();
                self.phase = Phase::Newer { latest, looked: now };
                if downloading { vec![Effect::StopDownload, Effect::Discard] } else { vec![Effect::Discard] }
            }
            Input::Hashed { attempt, result } => {
                let Phase::Verifying { latest } = &self.phase else { return Vec::new() };
                if attempt != self.attempt {
                    return Vec::new();
                }
                let latest = latest.clone();
                let failure = match result {
                    Ok(found) if found.eq_ignore_ascii_case(&latest.sha256) => {
                        self.phase = Phase::Ready { latest, note: None };
                        return Vec::new();
                    }
                    Ok(_) => Failure::Mismatch,
                    Err(why) => Failure::Unreadable(why),
                };
                self.phase = Phase::Failed { latest, failure };
                vec![Effect::Discard]
            }
            Input::Install => {
                let Phase::Ready { latest, .. } = &self.phase else { return Vec::new() };
                let latest = latest.clone();
                self.phase = Phase::Installing { latest: latest.clone() };
                vec![Effect::Launch { attempt: self.next(), latest }]
            }
            Input::Launched { attempt, result } => {
                if let (Phase::Installing { latest }, Err(why)) = (&self.phase, result)
                    && attempt == self.attempt
                {
                    self.phase = Phase::Ready { latest: latest.clone(), note: Some(Failure::Launch(why)) };
                }
                Vec::new()
            }
            Input::InstallerEnded { attempt, code } => {
                let Phase::Installing { latest } = &self.phase else { return Vec::new() };
                if attempt != self.attempt {
                    return Vec::new();
                }
                let latest = latest.clone();
                self.phase = match code {
                    INSTALLER_DONE => Phase::Installed { latest },
                    INSTALLER_CANCELLED => Phase::Ready { latest, note: Some(Failure::Declined) },
                    _ => Phase::Ready { latest, note: Some(Failure::Installer) },
                };
                Vec::new()
            }
            Input::ServiceGone => {
                if matches!(self.phase, Phase::Installing { .. }) {
                    vec![Effect::Close]
                } else {
                    Vec::new()
                }
            }
        }
    }

    /// The next attempt's number.
    fn next(&mut self) -> u64 {
        self.attempt += 1;
        self.attempt
    }

    /// Starts downloading the newer one, from a look that found it or after a failure.
    fn download(&mut self) -> Vec<Effect> {
        let latest = match &self.phase {
            Phase::Newer { latest, .. } | Phase::Failed { latest, .. } => latest.clone(),
            _ => return Vec::new(),
        };
        self.phase = Phase::Downloading { latest: latest.clone(), got: 0, total: None };
        vec![Effect::Download { attempt: self.next(), latest }]
    }

    /// What a fetch's answer says to this version.
    fn judge(&self, fetched: Fetched, now: u64) -> Phase {
        let text = match fetched {
            Fetched::Text(text) => text,
            Fetched::Unreached => return Phase::CantTell { why: CantTell::Unreached, looked: now },
        };
        match updates::parse(&text) {
            Some(latest) if updates::newer(&latest.version, &self.current) => {
                Phase::Newer { latest, looked: now }
            }
            Some(_) => Phase::Newest { looked: now },
            None => Phase::CantTell { why: CantTell::Garbled, looked: now },
        }
    }

    /// Whether a look can be asked for now: an installed app with nothing going on, and nothing
    /// ready to install.
    pub fn can_look(&self) -> bool {
        matches!(
            self.phase,
            Phase::NotLooked
                | Phase::Newest { .. }
                | Phase::Newer { .. }
                | Phase::CantTell { .. }
                | Phase::Failed { .. }
        )
    }

    /// The newer one, if one is out and hasn't been installed.
    pub fn newer(&self) -> Option<&Latest> {
        match &self.phase {
            Phase::Newer { latest, .. }
            | Phase::Downloading { latest, .. }
            | Phase::Verifying { latest }
            | Phase::Ready { latest, .. }
            | Phase::Installing { latest }
            | Phase::Failed { latest, .. } => Some(latest),
            _ => None,
        }
    }

    /// Whether the Settings button has its dot: a newer one is out, and wasn't put aside.
    pub fn dot(&self) -> bool {
        self.newer().is_some() && !self.put_aside
    }

    /// Whether the newer one is downloaded and checked, ready to install.
    pub fn ready(&self) -> bool {
        matches!(self.phase, Phase::Ready { .. })
    }
}

/// Why an install can't start now, if it can't: a machine being made, or with something being done
/// to it, would be cut off half done when the installer stops the service, which stops only what
/// runs. `making` is this app's own New machine, while it goes on.
pub fn install_blocked(machines: &[Machine], making: bool) -> Option<String> {
    if let Some(machine) = machines.iter().find(|machine| machine.state == State::Installing) {
        return Some(format!(
            "{} is being made. Install once it's done: the installer stops the service, which would leave it half made.",
            machine.name
        ));
    }
    if let Some((machine, doing)) =
        machines.iter().find_map(|machine| Some((machine, machine.working.as_deref()?)))
    {
        return Some(format!(
            "{}: {}. Install once that's done: the installer stops the service, which would cut it off.",
            machine.name,
            doing.trim_end_matches('.')
        ));
    }
    making.then(|| {
        "A new machine is being made. Install once it's done: the installer stops the service, which would leave it half made."
            .to_owned()
    })
}

/// The machines an install stops: every one that's on, which the service asks to shut down when the
/// installer stops it.
pub fn stopped_by_install(machines: &[Machine]) -> Vec<&Machine> {
    machines.iter().filter(|machine| !matches!(machine.state, State::Off | State::Failed)).collect()
}

/// An installer started, which says how it ended once it has.
pub trait Running: Send {
    /// Waits for it to end, and returns its exit code.
    fn wait(self: Box<Self>) -> i32;
}

/// What the app asks of the network and of Windows for an update. The real one is in system.rs,
/// and [`Fake`] stands in for it in the tests and the preview.
pub trait Updater: Send + Sync {
    /// Fetches the latest file from the file drop.
    fn fetch(&self) -> Fetched;
    /// Downloads `latest`'s installer as download `attempt`, calling `progress` as it comes, until
    /// it ends or `stop` is set.
    fn download(
        &self,
        latest: &Latest,
        attempt: u64,
        progress: &mut dyn FnMut(u64, Option<u64>),
        stop: &std::sync::atomic::AtomicBool,
    ) -> Result<(), DownloadError>;
    /// Reads download `attempt`'s file, holding it so nothing can change it until it's discarded,
    /// and returns its SHA-256 in hex.
    fn hash(&self, attempt: u64) -> Result<String, String>;
    /// Starts download `attempt`'s installer.
    fn launch(&self, attempt: u64) -> Result<Box<dyn Running>, String>;
    /// Lets go of what's downloaded, and deletes it.
    fn discard(&self);
}

/// An updater that answers as it's told to, at once, and touches nothing.
#[cfg(any(test, feature = "preview"))]
#[derive(Clone)]
pub struct Fake {
    pub fetched: Fetched,
    /// What a download ends with, after it has said it got half and then all of `size`.
    pub downloaded: Result<(), DownloadError>,
    pub size: u64,
    /// The hash a check finds.
    pub hash: Result<String, String>,
    /// Whether the installer starts, and what it ends with.
    pub launched: Result<i32, String>,
}

#[cfg(any(test, feature = "preview"))]
impl Fake {
    /// A fake whose look finds `fetched`, and whose download, check and install go as a real one's
    /// would when all is well.
    pub fn finding(fetched: Fetched) -> Fake {
        let hash = match &fetched {
            Fetched::Text(text) => updates::parse(text).map(|latest| latest.sha256).ok_or_else(String::new),
            Fetched::Unreached => Err(String::new()),
        };
        Fake { fetched, downloaded: Ok(()), size: 68 * 1024 * 1024, hash, launched: Ok(INSTALLER_CANCELLED) }
    }
}

#[cfg(any(test, feature = "preview"))]
impl Updater for Fake {
    fn fetch(&self) -> Fetched {
        self.fetched.clone()
    }

    fn download(
        &self,
        _: &Latest,
        _: u64,
        progress: &mut dyn FnMut(u64, Option<u64>),
        _: &std::sync::atomic::AtomicBool,
    ) -> Result<(), DownloadError> {
        progress(self.size / 2, Some(self.size));
        progress(self.size, Some(self.size));
        self.downloaded.clone()
    }

    fn hash(&self, _: u64) -> Result<String, String> {
        self.hash.clone()
    }

    fn launch(&self, _: u64) -> Result<Box<dyn Running>, String> {
        struct Ended(i32);
        impl Running for Ended {
            fn wait(self: Box<Self>) -> i32 {
                self.0
            }
        }
        self.launched.clone().map(|code| Box::new(Ended(code)) as Box<dyn Running>)
    }

    fn discard(&self) {}
}

/// A latest file as a release writes it, for `version`, for the tests and the preview.
#[cfg(any(test, feature = "preview"))]
pub fn release_file(version: &str) -> String {
    format!(
        r#"{{"version": "{version}", "installer": "https://files.ruxn.com/p/guesthouse/guesthouse-setup-{version}.exe", "sha256": "{}", "notes": "https://github.com/ruxnlabs/guesthouse/releases/tag/windows-v{version}"}}"#,
        "ab".repeat(32)
    )
}

#[cfg(test)]
mod tests;
