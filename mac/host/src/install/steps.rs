// SPDX-License-Identifier: GPL-2.0-or-later
//! The steps themselves: getting the installer, making the machine's disk, running the guest's
//! installer with the answers we wrote, putting our driver in, and the first start.
//!
//! What touches the system outside this file is behind two traits: `Transport` (the network) and
//! `MachineControl` (making a disc, starting the machine, reading its serial port, the clock). The
//! tests give them stand-ins, so the whole sequence runs with no network and no guest. What cannot
//! be proved this way is a real guest's side of it, which has not been run.

use super::fetch::{self, FetchError, Outcome, Request, Transport};
use super::plan::{Guest, Step, space_needed_after_installer_gb};
use super::runner::{Ended, Progress, Report, StepError, Steps};
use super::state::{InstallInfo, Source, StepStatus};
use super::{driver, password, ubuntu, windows};
use crate::store::MACHINE_FILE;
use guesthouse_basics::json_file::write_json;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

/// What the steps ask of the Mac that is not the network.
pub trait MachineControl {
    /// A disc image of `folder`'s files, with this volume label.
    fn make_disc(&mut self, folder: &Path, label: &str, out: &Path) -> Result<(), String>;
    /// A small FAT disk image holding `files`, for the guest to read.
    fn make_stick(&mut self, image: &Path, files: &[PathBuf]) -> Result<(), String>;
    /// Files of a disc image, by their paths on it, into `folder` under the same paths.
    fn extract(&mut self, image: &Path, names: &[&str], folder: &Path) -> Result<(), String>;
    /// Starts the machine as its file says.
    fn start(&mut self, id: &str) -> Result<(), String>;
    /// Where the machine stands (`running`, `stopped`, `failed`, ...) and why, if it failed.
    fn machine_state(&mut self, id: &str) -> (String, Option<String>);
    /// Everything the guest has said on its serial port so far.
    fn serial(&mut self, id: &str) -> String;
    fn now_ms(&mut self) -> u64;
    fn sleep_ms(&mut self, ms: u64);
}

/// What one install is made with.
pub struct Setup {
    pub machines: PathBuf,
    pub id: String,
    pub guest: Guest,
    pub disk_gb: u64,
    /// The Mac user's short name, for the guest's account.
    pub mac_user: String,
    /// Where downloads are kept.
    pub downloads: PathBuf,
    /// A folder of the driver's files to use instead of the published package (the lab's).
    pub driver_dir: Option<PathBuf>,
    /// The guest account's password, which only the answer files get.
    pub password: Option<String>,
    /// Whether to check the Mac has room for the whole install first; the stand-ins make nothing
    /// big, so they say no.
    pub check_space: bool,
    /// Folders to look for QEMU's firmware in.
    pub firmware_dirs: Vec<PathBuf>,
    /// The desktop an Ubuntu machine gets.
    pub desktop: crate::install::ubuntu::Desktop,
}

mod linux;

/// How often the serial port is read while waiting for the guest, and how long each wait may last.
const POLL_MS: u64 = 5_000;
const SYSTEM_INSTALL_MAX_MS: u64 = 3 * 3_600_000;
/// How long Ubuntu has to power off after saying it has finished.
const POWER_OFF_MAX_MS: u64 = 10 * 60_000;
const DRIVER_MAX_MS: u64 = 30 * 60_000;
const FIRST_START_MAX_MS: u64 = 20 * 60_000;
/// How long an Ubuntu machine must stay up after its first start to count as having come up.
const UBUNTU_UP_MS: u64 = 30_000;
/// Share of the usual time a wait is shown as done at most, so the bar never says finished early.
const GUESS_CEILING: f64 = 0.95;
/// What Ubuntu's own installer and the driver's step usually take on a Mac (2026-11-27: the
/// installer three and a half to four minutes, the driver half a minute), for the bar's guess.
/// The limits above stay as they are: these only say how fast the bar moves.
const UBUNTU_INSTALL_USUAL_MS: u64 = 5 * 60_000;
const UBUNTU_DRIVER_USUAL_MS: u64 = 2 * 60_000;
const WINDOWS_COMPUTER_NAME: &str = "GUESTHOUSE";
const WINDOWS_EDITION: &str = "Windows 11 Pro";

pub struct RealSteps<'a> {
    pub setup: Setup,
    pub transport: &'a mut dyn Transport,
    pub control: &'a mut dyn MachineControl,
}

fn from_fetch(error: FetchError) -> StepError {
    StepError { code: error.code, message: error.message }
}

/// Sets and removes keys in the machine's `machine.json`, keeping every other.
pub fn set_keys(folder: &Path, changes: &[(&str, Option<&str>)]) -> Result<(), StepError> {
    let path = folder.join(MACHINE_FILE);
    let text =
        fs::read_to_string(&path).map_err(|e| StepError::new(format!("Cannot read {MACHINE_FILE}: {e}")))?;
    let mut settings: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| StepError::new(format!("{MACHINE_FILE} is not valid: {e}")))?;
    for (key, value) in changes {
        match value {
            Some(value) => settings[*key] = (*value).into(),
            None => {
                settings.as_object_mut().map(|object| object.remove(*key));
            }
        }
    }
    write_json(&path, &settings).map_err(|e| StepError::new(format!("Cannot write {MACHINE_FILE}: {e}")))
}

/// The installer the fetch named in the machine's file.
fn installer_of(folder: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(folder.join(MACHINE_FILE)).ok()?;
    let settings: serde_json::Value = serde_json::from_str(&text).ok()?;
    settings["installer"].as_str().map(PathBuf::from)
}

/// Reads a firmware file, which QEMU's own folder may keep compressed.
fn read_firmware(path: &Path) -> Option<Vec<u8>> {
    if path.is_file() {
        return fs::read(path).ok();
    }
    let compressed = PathBuf::from(format!("{}.bz2", path.display()));
    let output = std::process::Command::new("bunzip2").arg("-c").arg(&compressed).output().ok()?;
    (compressed.is_file() && output.status.success()).then_some(output.stdout)
}

/// The folders a QEMU's firmware may be in: `share/qemu` of the prefix it is installed in, then
/// `pc-bios` beside it, where a QEMU run from its own build folder keeps it, and, for a QEMU in an
/// app bundle (`Contents/Helpers`), the bundle's own `Contents/Resources/qemu`.
pub fn firmware_dirs_of(qemu: &Path) -> Vec<PathBuf> {
    let Some(folder) = qemu.parent() else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = folder.parent().map(|p| p.join("share/qemu")).into_iter().collect();
    dirs.push(folder.join("pc-bios"));
    dirs.extend(folder.parent().map(|p| p.join("Resources/qemu")));
    dirs
}

/// QEMU's firmware for ARM: the code and the empty variables, from the first folder that has them.
pub fn find_firmware(dirs: &[PathBuf]) -> Result<(Vec<u8>, Vec<u8>), StepError> {
    for dir in dirs {
        let code = read_firmware(&dir.join("edk2-aarch64-code.fd"));
        let vars = read_firmware(&dir.join("edk2-aarch64-vars.fd"))
            .or_else(|| read_firmware(&dir.join("edk2-arm-vars.fd")));
        if let (Some(code), Some(vars)) = (code, vars) {
            return Ok((code, vars));
        }
    }
    Err(StepError::coded("no_firmware", "QEMU's firmware for ARM (edk2) was not found next to QEMU."))
}

impl RealSteps<'_> {
    fn folder(&self) -> PathBuf {
        self.setup.machines.join(&self.setup.id)
    }

    fn fetch(&mut self, info: &InstallInfo, progress: Progress) -> Result<Ended, StepError> {
        let installer = match (self.setup.guest, &info.source) {
            (Guest::Windows, Source::File { path }) => {
                windows::check_installer(Path::new(path)).map_err(|m| StepError::coded("bad_installer", m))?
            }
            (Guest::Windows, Source::Download) => {
                return Err(StepError::coded(
                    "choose_installer",
                    "Windows needs an installer you choose. Save the Windows 11 installer for ARM from Microsoft's page, then choose it here.",
                ));
            }
            (Guest::Ubuntu, Source::File { path }) => {
                let path = PathBuf::from(path);
                if !path.is_file() {
                    return Err(StepError::coded(
                        "bad_installer",
                        format!("{} is not there.", path.display()),
                    ));
                }
                path
            }
            (Guest::Ubuntu, Source::Download) => {
                let sums = self
                    .transport
                    .text(&format!("{}{}", ubuntu::RELEASE_URL, ubuntu::SUMS_FILE))
                    .map_err(from_fetch)?;
                let image = ubuntu::pick_image(&sums).map_err(StepError::new)?;
                let folder = self.setup.downloads.join("ubuntu");
                let request = Request {
                    url: &image.url,
                    folder: &folder,
                    name: &image.name,
                    sha256: &image.sha256,
                    size: None,
                };
                match fetch::download(self.transport, &request, &mut |done, total| {
                    progress(Report::Bytes(done, Some(total)))
                })
                .map_err(from_fetch)?
                {
                    Outcome::Done(path) => path,
                    Outcome::Stopped => return Ok(Ended::Stopped),
                }
            }
        };
        set_keys(&self.folder(), &[("installer", Some(&installer.display().to_string()))])?;
        // The driver comes with the installer, so a missing package stops the install here and
        // not an hour in. A folder named by the lab takes its place.
        if self.setup.guest == Guest::Windows && self.setup.driver_dir.is_none() {
            driver::fetch_package(self.transport, &self.setup.downloads).map_err(from_fetch)?;
        }
        // Ubuntu can be opened without it, so a package that is not there is said at the driver
        // step, where that is offered.
        let lab_linux = self.setup.driver_dir.as_ref();
        if self.setup.guest == Guest::Ubuntu
            && !lab_linux.is_some_and(|dir| dir.join(driver::LINUX_SCRIPT).is_file())
        {
            let _ = driver::fetch_package_of(&driver::LINUX, self.transport, &self.setup.downloads);
        }
        Ok(Ended::Done)
    }

    fn make_disk(&mut self) -> Result<Ended, StepError> {
        let folder = self.folder();
        fs::create_dir_all(&folder)
            .map_err(|e| StepError::new(format!("Cannot make {}: {e}", folder.display())))?;
        let need = space_needed_after_installer_gb(self.setup.guest, self.setup.disk_gb);
        if let Some(free) = fetch::free_bytes(&folder).filter(|_| self.setup.check_space) {
            room_for(need, free)?;
        }
        let disk = folder.join("disk.img");
        if !disk.exists() {
            File::create(&disk)
                .and_then(|file| file.set_len(self.setup.disk_gb * 1024 * 1024 * 1024))
                .map_err(|e| StepError::new(format!("Cannot make the disk: {e}")))?;
        }
        if !folder.join("code.fd").exists() || !folder.join("vars.fd").exists() {
            let (code, vars) = find_firmware(&self.setup.firmware_dirs)?;
            fs::write(folder.join("code.fd"), code)
                .and_then(|()| fs::write(folder.join("vars.fd"), vars))
                .map_err(|e| StepError::new(format!("Cannot write the firmware: {e}")))?;
        }
        // The graphics device's ROM gives the firmware and the installer a picture. Without one
        // the guest is still installed, and shows nothing until its driver runs.
        if self.setup.guest == Guest::Ubuntu
            && !folder.join(linux::OPTION_ROM).exists()
            && let Some(rom) = linux::find_option_rom(&self.setup.firmware_dirs)
        {
            fs::copy(&rom, folder.join(linux::OPTION_ROM))
                .map_err(|e| StepError::new(format!("Cannot copy the option ROM: {e}")))?;
            set_keys(&folder, &[("option_rom", Some(linux::OPTION_ROM))])?;
        }
        Ok(Ended::Done)
    }

    /// The answers on a disc, and for Windows the driver on a stick, then the machine started.
    fn start_installer(&mut self) -> Result<(), StepError> {
        let folder = self.folder();
        let answers = folder.join("answers");
        let _ = fs::remove_dir_all(&answers);
        fs::create_dir_all(&answers).map_err(|e| StepError::new(e.to_string()))?;
        let account = windows::account_name(&self.setup.mac_user);
        let secret = self.setup.password.clone().ok_or_else(|| {
            StepError::coded(
                "password_missing",
                "Guesthouse no longer has the password it was to give this machine's account. Try again from the app, which has it.",
            )
        })?;
        password::check(&secret).map_err(|m| StepError::coded("password_invalid", m))?;
        let label = match self.setup.guest {
            Guest::Windows => {
                let text = windows::autounattend(&windows::Answers {
                    account: &account,
                    password: &secret,
                    computer_name: WINDOWS_COMPUTER_NAME,
                    edition: WINDOWS_EDITION,
                });
                fs::write(answers.join("autounattend.xml"), text)
                    .map_err(|e| StepError::new(e.to_string()))?;
                let dir = self
                    .setup
                    .driver_dir
                    .clone()
                    .or_else(|| driver::kept_package(&self.setup.downloads))
                    .ok_or_else(|| {
                        StepError::coded(
                            "no_driver",
                            "The driver's files were not found. Download them again.",
                        )
                    })?;
                let files = driver::package_files(&dir).map_err(|m| StepError::coded("no_driver", m))?;
                self.control
                    .make_stick(&folder.join("stick.img"), &files)
                    .map_err(|m| StepError::coded("stick", m))?;
                set_keys(&folder, &[("stick", Some("stick.img"))])?;
                "GPUPV_SETUP"
            }
            Guest::Ubuntu => {
                self.ubuntu_answers(&answers, &account, &secret)?;
                // Its kernel from the installer the fetch named; a machine whose file names
                // none starts as its file says.
                if let Some(installer) = installer_of(&folder) {
                    self.ubuntu_installer_boot(&installer)?;
                }
                "CIDATA"
            }
        };
        self.control
            .make_disc(&answers, label, &folder.join("answers.iso"))
            .map_err(|m| StepError::coded("disc", m))?;
        set_keys(&folder, &[("answer_disc", Some("answers.iso"))])?;
        let _ = fs::remove_dir_all(&answers);
        self.control.start(&self.setup.id).map_err(StepError::new)
    }

    /// Waits for a line the guest says, or the machine to go wrong. `done` reads the serial text.
    /// The bar shows the time gone of `usual_ms`, at most `GUESS_CEILING`; `max_ms` is the limit.
    fn wait_for<T>(
        &mut self,
        max_ms: u64,
        usual_ms: u64,
        from: usize,
        progress: Progress,
        mut done: impl FnMut(&str) -> Option<Result<T, StepError>>,
    ) -> Result<Option<T>, StepError> {
        let began = self.control.now_ms();
        loop {
            let serial = self.control.serial(&self.setup.id);
            let heard = serial.get(from..).unwrap_or("");
            if let Some(answer) = done(heard) {
                return answer.map(Some);
            }
            let (state, why) = self.control.machine_state(&self.setup.id);
            if state == "failed" {
                return Err(StepError::new(
                    why.unwrap_or_else(|| "The machine stopped with an error.".into()),
                ));
            }
            let elapsed = self.control.now_ms() - began;
            if elapsed >= max_ms {
                return Err(StepError::coded(
                    "timeout",
                    format!("The guest did not finish in {} minutes.", max_ms / 60_000),
                ));
            }
            if !progress(Report::Fraction((elapsed as f64 / usual_ms.max(1) as f64).min(GUESS_CEILING))) {
                return Ok(None);
            }
            self.control.sleep_ms(POLL_MS);
        }
    }

    fn install_system(&mut self, progress: Progress) -> Result<Ended, StepError> {
        self.start_installer()?;
        let guest = self.setup.guest;
        let id = self.setup.id.clone();
        let usual = match guest {
            Guest::Windows => SYSTEM_INSTALL_MAX_MS,
            Guest::Ubuntu => UBUNTU_INSTALL_USUAL_MS,
        };
        let heard = self.wait_for(SYSTEM_INSTALL_MAX_MS, usual, 0, progress, |text| match guest {
            Guest::Windows => text.contains(windows::marker::SYSTEM_INSTALLED).then_some(Ok(())),
            Guest::Ubuntu if text.contains(ubuntu::FAILED) => Some(Err(StepError::coded(
                "autoinstall",
                "Ubuntu's installer reported an error. Its log is in the machine's serial.log.",
            ))),
            Guest::Ubuntu => text.contains(ubuntu::FINISHED).then_some(Ok(())),
        })?;
        if heard.is_none() {
            return Ok(Ended::Stopped);
        }
        // Ubuntu powers itself off when it has finished; wait for that, so the next start is its own.
        if guest == Guest::Ubuntu {
            let began = self.control.now_ms();
            while self.control.machine_state(&id).0 != "off" {
                if self.control.now_ms() - began >= POWER_OFF_MAX_MS {
                    return Err(StepError::coded(
                        "timeout",
                        "Ubuntu did not turn itself off when it had finished.",
                    ));
                }
                self.control.sleep_ms(POLL_MS);
            }
        }
        Ok(Ended::Done)
    }

    fn install_driver(&mut self, progress: Progress) -> Result<Ended, StepError> {
        if self.setup.guest == Guest::Ubuntu {
            return self.ubuntu_driver(progress);
        }
        let heard = self.wait_for(DRIVER_MAX_MS, DRIVER_MAX_MS, 0, progress, |text| {
            if let Some(line) = text.lines().find(|l| l.contains(windows::marker::DRIVER_FAILED)) {
                let code = line.rsplit(' ').next().unwrap_or("").trim().to_string();
                return Some(Err(StepError {
                    message: format!("Windows turned the driver away with error {code}."),
                    code: Some(code),
                }));
            }
            text.contains(windows::marker::DRIVER_INSTALLED).then_some(Ok(()))
        })?;
        Ok(if heard.is_some() { Ended::Done } else { Ended::Stopped })
    }

    fn first_start(&mut self, info: &InstallInfo, progress: Progress) -> Result<Ended, StepError> {
        let folder = self.folder();
        set_keys(&folder, &[("installer", None), ("answer_disc", None), ("stick", None)])?;
        // The answers hold the account's password. Once the disc is no longer in the machine
        // nothing needs it, so it does not stay on the Mac.
        let _ = fs::remove_file(folder.join("answers.iso"));
        match self.setup.guest {
            Guest::Windows => {
                // The guest restarts itself after the driver; its next start says it came up.
                let heard = self.wait_for(FIRST_START_MAX_MS, FIRST_START_MAX_MS, 0, progress, |text| {
                    text.contains(windows::marker::FIRST_START).then_some(Ok(()))
                })?;
                Ok(if heard.is_some() { Ended::Done } else { Ended::Stopped })
            }
            Guest::Ubuntu => {
                let with_driver = info.status_of(Step::InstallDriver) == StepStatus::Done;
                self.ubuntu_first_start(with_driver, progress)
            }
        }
    }
}

impl Steps for RealSteps<'_> {
    fn run(&mut self, step: Step, info: &InstallInfo, progress: Progress) -> Result<Ended, StepError> {
        match step {
            Step::Fetch => self.fetch(info, progress),
            Step::MakeDisk => self.make_disk(),
            Step::InstallSystem => self.install_system(progress),
            Step::InstallDriver => self.install_driver(progress),
            Step::FirstStart => self.first_start(info, progress),
        }
    }
}

/// An error saying how much to free, when `free` bytes are fewer than `need` gigabytes.
fn room_for(need: u64, free: u64) -> Result<(), StepError> {
    if free / 1_000_000_000 >= need {
        return Ok(());
    }
    let more = need - free / 1_000_000_000;
    Err(StepError::coded(
        "no_space",
        format!(
            "Not enough room. This install needs {need} GB and this Mac has {} free: free {more} GB and try again.",
            fetch::gigabytes(free)
        ),
    ))
}

#[cfg(test)]
mod tests;
