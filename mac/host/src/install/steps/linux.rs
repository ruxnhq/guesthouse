// SPDX-License-Identifier: GPL-2.0-or-later
//! Ubuntu's own parts of the steps: its installer started by its kernel, the driver from a disk
//! labelled `RUXN`, and the first start, with or without the driver.

use super::{
    DRIVER_MAX_MS, FIRST_START_MAX_MS, GUESS_CEILING, POLL_MS, POWER_OFF_MAX_MS, RealSteps,
    UBUNTU_DRIVER_USUAL_MS, UBUNTU_UP_MS, set_keys,
};
use crate::install::driver::{self, LINUX};
use crate::install::runner::{Ended, Progress, Report, StepError};
use crate::install::ubuntu::{self, marker};
use std::fs;
use std::path::{Path, PathBuf};

/// Where the installer's kernel and initrd are put in the machine's folder while it installs.
const BOOT_FOLDER: &str = "installer-boot";
/// The option ROM that gives the firmware its picture on the graphics device, as the machine's
/// folder keeps it.
pub(super) const OPTION_ROM: &str = "pvgfb.rom";

/// The keys of `machine.json` that only the installer's start needs.
const INSTALLER_KEYS: [&str; 5] =
    ["installer", "answer_disc", "installer_kernel", "installer_initrd", "installer_command_line"];

/// The graphics device's option ROM, from the first of QEMU's folders that has one. QEMU keeps a
/// device's ROMs in its data folder, beside the firmware.
pub(super) fn find_option_rom(dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter().map(|dir| dir.join(OPTION_ROM)).find(|path| path.is_file())
}

impl RealSteps<'_> {
    /// The answer disc's files for Ubuntu: the answers, the seed's name, and the driver's service
    /// the answers put into the installed system.
    pub(super) fn ubuntu_answers(
        &mut self,
        answers: &Path,
        account: &str,
        secret: &str,
    ) -> Result<(), StepError> {
        let files = [
            (
                "user-data",
                ubuntu::user_data(
                    account,
                    &self.setup.id,
                    &ubuntu::mac_time_zone(),
                    self.setup.desktop,
                    crate::keyboard_layout::mac_layout(),
                ),
            ),
            (ubuntu::ACCOUNT_FILE, ubuntu::account_file(account, secret)),
            ("meta-data", ubuntu::meta_data(&self.setup.id)),
            (ubuntu::DRIVER_SERVICE_FILE, ubuntu::DRIVER_SERVICE.to_string()),
            (ubuntu::DRIVER_RUNNER_FILE, ubuntu::DRIVER_RUNNER.to_string()),
        ];
        for (name, text) in files {
            fs::write(answers.join(name), text).map_err(|e| StepError::new(e.to_string()))?;
        }
        Ok(())
    }

    /// The installer's kernel and initrd taken off its disc, for the firmware to start with the
    /// kernel line that runs the installer with no questions.
    pub(super) fn ubuntu_installer_boot(&mut self, installer: &Path) -> Result<(), StepError> {
        let folder = self.folder();
        let boot = folder.join(BOOT_FOLDER);
        let _ = fs::remove_dir_all(&boot);
        self.control
            .extract(installer, &[ubuntu::INSTALLER_KERNEL, ubuntu::INSTALLER_INITRD], &boot)
            .map_err(|m| StepError::coded("installer_boot", m))?;
        let kernel = format!("{BOOT_FOLDER}/{}", ubuntu::INSTALLER_KERNEL);
        let initrd = format!("{BOOT_FOLDER}/{}", ubuntu::INSTALLER_INITRD);
        set_keys(
            &folder,
            &[
                ("installer_kernel", Some(&kernel)),
                ("installer_initrd", Some(&initrd)),
                ("installer_command_line", Some(ubuntu::INSTALLER_COMMAND_LINE)),
            ],
        )
    }

    /// The Linux package to install: the lab's folder when it holds one (it may hold Windows'
    /// instead, since one setting names both), else the one fetched with the installer.
    fn linux_package(&self) -> Option<PathBuf> {
        self.setup
            .driver_dir
            .clone()
            .filter(|dir| dir.join(driver::LINUX_SCRIPT).is_file())
            .or_else(|| driver::kept_package_of(&LINUX, &self.setup.downloads))
    }

    /// The driver on its disk and the machine started from its own disk with it. The service the
    /// answers put in installs it and says how it went; it restarts into the desktop when it
    /// worked and turns the machine off when it did not, so Try Again starts it once more.
    pub(super) fn ubuntu_driver(&mut self, progress: Progress) -> Result<Ended, StepError> {
        let dir = match self.linux_package() {
            Some(dir) => dir,
            // Not fetched with the installer, or that failed: fetched now, so that Try Again
            // fetches again, and a failure says why it failed.
            None => driver::fetch_package_of(&LINUX, self.transport, &self.setup.downloads).map_err(|e| {
                StepError::coded(
                    e.code.as_deref().unwrap_or("driver_download"),
                    ubuntu::driver_download_failed(&e.message),
                )
            })?,
        };
        let files = driver::package_files_of(&LINUX, &dir).map_err(|m| StepError::coded("no_driver", m))?;
        let folder = self.folder();
        self.control
            .make_stick(&folder.join("stick.img"), &files)
            .map_err(|m| StepError::coded("stick", m))?;
        let mut changes: Vec<(&str, Option<&str>)> = INSTALLER_KEYS.iter().map(|key| (*key, None)).collect();
        changes.push(("stick", Some("stick.img")));
        set_keys(&folder, &changes)?;
        let _ = fs::remove_dir_all(folder.join(BOOT_FOLDER));
        let id = self.setup.id.clone();
        if self.control.machine_state(&id).0 != "running" {
            self.control.start(&id).map_err(StepError::new)?;
        }
        let heard = self.wait_for(DRIVER_MAX_MS, UBUNTU_DRIVER_USUAL_MS, 0, progress, |text| {
            if let Some(line) = text.lines().find(|l| l.contains(marker::DRIVER_FAILED)) {
                let code = line.rsplit(' ').next().unwrap_or("").trim().to_string();
                return Some(Ok(Some(code)));
            }
            text.contains(marker::DRIVER_INSTALLED).then_some(Ok(None))
        })?;
        match heard {
            None => Ok(Ended::Stopped),
            Some(None) => Ok(Ended::Done),
            Some(Some(code)) => {
                // The guest turns itself off after a failure; Try Again is then a start of its own.
                self.wait_stopped(&id);
                Err(StepError {
                    message: format!("Ubuntu could not install the driver ({code})."),
                    code: Some(code),
                })
            }
        }
    }

    /// Waits a while for the machine to stop by itself.
    fn wait_stopped(&mut self, id: &str) {
        let began = self.control.now_ms();
        while self.control.machine_state(id).0 == "running"
            && self.control.now_ms() - began < POWER_OFF_MAX_MS
        {
            self.control.sleep_ms(POLL_MS);
        }
    }

    /// With the driver the guest has restarted into its desktop by itself and says so; without
    /// it the machine is started from its own disk and must stay up for half a minute.
    pub(super) fn ubuntu_first_start(
        &mut self,
        with_driver: bool,
        progress: Progress,
    ) -> Result<Ended, StepError> {
        if with_driver {
            if self.control.machine_state(&self.setup.id).0 != "running" {
                self.control.start(&self.setup.id).map_err(StepError::new)?;
            }
            let heard = self.wait_for(FIRST_START_MAX_MS, FIRST_START_MAX_MS, 0, progress, |text| {
                text.contains(marker::FIRST_START).then_some(Ok(()))
            })?;
            return Ok(if heard.is_some() { Ended::Done } else { Ended::Stopped });
        }
        self.control.start(&self.setup.id).map_err(StepError::new)?;
        let began = self.control.now_ms();
        let mut up_since: Option<u64> = None;
        loop {
            let (state, why) = self.control.machine_state(&self.setup.id);
            let now = self.control.now_ms();
            match state.as_str() {
                "running" => {
                    let since = *up_since.get_or_insert(now);
                    if now - since >= UBUNTU_UP_MS {
                        return Ok(Ended::Done);
                    }
                }
                "failed" | "off" if now - began > POLL_MS => {
                    return Err(StepError::new(
                        why.unwrap_or_else(|| "The machine stopped soon after it started.".into()),
                    ));
                }
                _ => up_since = None,
            }
            if now - began >= FIRST_START_MAX_MS {
                return Err(StepError::coded("timeout", "The machine did not come up."));
            }
            if !progress(Report::Fraction(((now - began) as f64 / UBUNTU_UP_MS as f64).min(GUESS_CEILING))) {
                return Ok(Ended::Stopped);
            }
            self.control.sleep_ms(POLL_MS);
        }
    }
}

#[cfg(test)]
mod tests;
