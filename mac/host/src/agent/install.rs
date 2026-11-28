// SPDX-License-Identifier: GPL-2.0-or-later
//! Installing a guest, as the agent does it: a thread for each install that runs the steps, keeps
//! the state in the machine's folder, and tells everyone connected as it changes. An install that
//! was running when the agent went away is taken up again where it was.
//!
//! The real steps use the network (`CurlTransport`) and the machines the agent starts. With
//! `InstallConfig::fake` set they use the stand-ins of `install::fake`, so the screens and the
//! command line can be tried with neither.

use super::{Agent, lock};
use crate::install::fake::{FakeGuest, FakeNetwork};
use crate::install::fetch::CurlTransport;
use crate::install::machine::{create_machine, default_sizes};
use crate::install::password;
use crate::install::plan::{Guest, Plan};
use crate::install::runner::Runner;
use crate::install::state::{self, InstallInfo, Phase, Source};
use crate::install::steps::{MachineControl, RealSteps, Setup, firmware_dirs_of};
use crate::protocol::{Secret, install_event_line};
use crate::store;
use crate::tools::{self, Places, Tool};
use guesthouse_basics::clock::{Clock, RealClock};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

/// The programs one install is run with, as the request named them.
#[derive(Debug, Clone, Default)]
pub(super) struct Tools {
    pub qemu: Option<String>,
    pub pvg_host: Option<String>,
    pub driver_dir: Option<PathBuf>,
    pub sizes: Sizes,
    /// The guest account's password, in memory only.
    pub password: Option<Secret>,
    /// Ubuntu's desktop as the app sent it.
    pub desktop: Option<String>,
}

/// What Customise changed; each left out is the guest's default.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Sizes {
    pub cores: Option<u32>,
    pub memory_mb: Option<u64>,
    pub disk_gb: Option<u64>,
}

#[derive(Default)]
pub(super) struct InstallSlot {
    info: Option<InstallInfo>,
    cancel: Option<Arc<AtomicBool>>,
    running: bool,
    tools: Tools,
}

const FAKE_DISK_GB: u64 = 1;
/// The system's tar, which reads ISO 9660 images.
const TAR: &str = "/usr/bin/tar";

/// The Mac's side of the steps, with the agent's machines behind it.
struct AgentControl {
    agent: Agent,
    tools: Tools,
    run_dir: PathBuf,
    /// The install's own clock, the shared one, from where the install began.
    clock: Arc<RealClock>,
}

impl MachineControl for AgentControl {
    fn make_disc(&mut self, folder: &Path, label: &str, out: &Path) -> Result<(), String> {
        let _ = fs::remove_file(out);
        let output = Command::new("hdiutil")
            .args(["makehybrid", "-quiet", "-iso", "-joliet", "-default-volume-name", label, "-o"])
            .arg(out)
            .arg(folder)
            .output()
            .map_err(|e| format!("hdiutil could not be started: {e}"))?;
        if output.status.success() {
            return Ok(());
        }
        Err(format!("The answer disc could not be made: {}", String::from_utf8_lossy(&output.stderr).trim()))
    }

    fn make_stick(&mut self, image: &Path, files: &[PathBuf]) -> Result<(), String> {
        crate::install::stick::make(image, files)
    }

    fn extract(&mut self, image: &Path, names: &[&str], folder: &Path) -> Result<(), String> {
        fs::create_dir_all(folder).map_err(|e| format!("Cannot make {}: {e}", folder.display()))?;
        let output = Command::new(TAR)
            .arg("-xf")
            .arg(image)
            .arg("-C")
            .arg(folder)
            .args(names)
            .output()
            .map_err(|e| format!("tar could not be started: {e}"))?;
        if let Some(missing) = names.iter().find(|name| !folder.join(name).is_file()) {
            return Err(format!(
                "{missing} could not be taken from {}: {}",
                image.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(())
    }

    fn start(&mut self, id: &str) -> Result<(), String> {
        self.agent.start(
            id,
            self.tools.qemu.as_deref(),
            None,
            self.tools.pvg_host.as_deref(),
            // The installer is not told; the first start from the app is.
            None,
        )
    }

    fn machine_state(&mut self, id: &str) -> (String, Option<String>) {
        let slots = lock(&self.agent.shared.slots);
        match slots.get(id).map(|slot| slot.status()) {
            Some(status) => (status.name(), status.reason().map(String::from)),
            None => ("off".to_string(), None),
        }
    }

    fn serial(&mut self, _: &str) -> String {
        fs::read_to_string(self.run_dir.join("serial.log")).unwrap_or_default()
    }

    fn now_ms(&mut self) -> u64 {
        self.clock.now().as_millis() as u64
    }

    fn sleep_ms(&mut self, ms: u64) {
        self.clock.sleep(Duration::from_millis(ms));
    }
}

impl Agent {
    /// Starts installing `guest` in a new machine called `name`. Returns the machine's id.
    pub(super) fn install_start(
        &self,
        name: &str,
        guest: &str,
        file: Option<String>,
        tools: Tools,
    ) -> Result<String, String> {
        let guest =
            Guest::from_name(guest).ok_or_else(|| format!("{guest} is not a guest this can install."))?;
        let machines = &self.shared.config.machines;
        if let Some(password) = &tools.password {
            password::check(&password.0)?;
        }
        if let Some(word) = &tools.desktop
            && crate::install::ubuntu::Desktop::parse(word).is_none()
        {
            return Err(format!("{word} is not a desktop this can install."));
        }
        let sizes = tools.sizes;
        let (cores, memory_mb, _) =
            default_sizes(guest, std::thread::available_parallelism().map_or(4, |n| n.get() as u32));
        let id = create_machine(
            machines,
            name,
            guest,
            sizes.cores.unwrap_or(cores),
            sizes.memory_mb.unwrap_or(memory_mb),
        )?;
        let source = file.map_or(Source::Download, |path| Source::File { path });
        let mut info = InstallInfo::new(&id, &Plan::for_guest(guest), source);
        info.account = crate::install::windows::account_name(&crate::install::windows::mac_user());
        info.disk_gb = Some(sizes.disk_gb.unwrap_or(default_sizes(guest, 4).2));
        info.desktop = tools.desktop.clone();
        {
            let mut slots = lock(&self.shared.slots);
            let slot = slots.entry(id.clone()).or_default();
            slot.install.tools = tools;
            slot.install.info = Some(info.clone());
        }
        let _ = state::save(&machines.join(&id), &info);
        self.publish_install(&id, info);
        self.spawn_install(&id);
        Ok(id)
    }

    /// What the person chose at a failure.
    pub(super) fn install_choose(
        &self,
        id: &str,
        choice: &str,
        path: Option<String>,
        password: Option<Secret>,
    ) -> Result<(), String> {
        let mut info = self.install_info(id).ok_or("There is no install for this machine.")?;
        let done = match choice {
            "try_again" => info.retry(),
            "choose_installer" => path.is_some_and(|path| info.choose_installer(&path)),
            "open_without_driver" => info.open_without_driver(),
            other => return Err(format!("{other} is not something an install can be told.")),
        };
        if !done {
            return Err("That is not offered at this point of the install.".into());
        }
        let _ = state::save(&store::default_folder_of(&self.shared.config.machines, id), &info);
        if let Some(password) = password {
            lock(&self.shared.slots).entry(id.to_string()).or_default().install.tools.password =
                Some(password);
        }
        self.publish_install(id, info);
        self.spawn_install(id);
        Ok(())
    }

    pub(super) fn install_cancel(&self, id: &str) -> Result<(), String> {
        let slots = lock(&self.shared.slots);
        let cancel = slots
            .get(id)
            .and_then(|slot| slot.install.cancel.clone())
            .ok_or("There is no install running for this machine.")?;
        cancel.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Every install the agent knows, for a listing.
    pub(super) fn installs(&self) -> Vec<InstallInfo> {
        lock(&self.shared.slots).values().filter_map(|slot| slot.install.info.clone()).collect()
    }

    fn install_info(&self, id: &str) -> Option<InstallInfo> {
        lock(&self.shared.slots)
            .get(id)
            .and_then(|slot| slot.install.info.clone())
            .or_else(|| state::load(&self.shared.config.machines.join(id)))
    }

    fn publish_install(&self, id: &str, info: InstallInfo) {
        let line = install_event_line(&info);
        lock(&self.shared.slots).entry(id.to_string()).or_default().install.info = Some(info);
        lock(&self.shared.subscribers).retain(|(_, sender)| sender.send(line.clone()).is_ok());
    }

    /// Takes up the installs that were running when the last agent went away.
    pub(super) fn resume_installs(&self) {
        let Ok(entries) = fs::read_dir(&self.shared.config.machines) else {
            return;
        };
        for entry in entries.flatten() {
            let id = entry.file_name().to_string_lossy().into_owned();
            let Some(info) = state::load(&entry.path()) else {
                continue;
            };
            if info.state != Phase::Running {
                // Kept for the screens: done, failed or stopped installs are still listed.
                lock(&self.shared.slots).entry(id).or_default().install.info = Some(info);
                continue;
            }
            lock(&self.shared.slots).entry(id.clone()).or_default().install.info = Some(info);
            self.spawn_install(&id);
        }
    }

    /// Runs the install's steps on a thread, unless one is running for this machine.
    fn spawn_install(&self, id: &str) {
        let (cancel, tools) = {
            let mut slots = lock(&self.shared.slots);
            let slot = slots.entry(id.to_string()).or_default();
            if slot.install.running {
                return;
            }
            slot.install.running = true;
            let cancel = Arc::new(AtomicBool::new(false));
            slot.install.cancel = Some(cancel.clone());
            (cancel, slot.install.tools.clone())
        };
        let agent = self.clone();
        let id = id.to_string();
        thread::spawn(move || {
            agent.run_install(&id, cancel, tools);
            lock(&agent.shared.slots).entry(id).or_default().install.running = false;
        });
    }

    fn run_install(&self, id: &str, cancel: Arc<AtomicBool>, tools: Tools) {
        let Some(info) = self.install_info(id) else {
            return;
        };
        let config = &self.shared.config;
        let machines = config.machines.clone();
        let support = machines.parent().map(Path::to_path_buf).unwrap_or_else(|| machines.clone());
        // The firmware beside the QEMU the request or the agent names, then beside the protected
        // path's QEMU as the app finds it with none named (the environment, the lab's build).
        let mut qemus: Vec<PathBuf> = tools.qemu.iter().map(PathBuf::from).collect();
        qemus.extend(config.qemu.clone());
        qemus.extend(
            tools::locate(Tool::PvgQemu, &Places::on_this_mac(Tool::PvgQemu, None, None))
                .ok()
                .map(|found| found.path),
        );
        let mut firmware_dirs: Vec<PathBuf> = qemus.iter().flat_map(|q| firmware_dirs_of(q)).collect();
        firmware_dirs.extend(config.install.firmware_dir.clone());
        firmware_dirs
            .extend(std::env::var_os("HOME").and_then(|home| crate::lab::firmware_dir(Path::new(&home))));
        let setup = Setup {
            machines: machines.clone(),
            id: id.to_string(),
            guest: info.guest,
            // The stand-in install makes a small disk, so it needs no room.
            disk_gb: disk_gb_for(&info, &tools, config.install.fake),
            mac_user: crate::install::windows::mac_user(),
            downloads: support.join("Downloads"),
            password: tools.password.as_ref().map(|p| p.0.clone()),
            driver_dir: tools.driver_dir.clone().or_else(|| config.install.driver_dir.clone()),
            check_space: !config.install.fake,
            firmware_dirs,
            desktop: desktop_for(&info, &tools),
        };
        let real_clock = Arc::new(RealClock::default());
        let clock = {
            let real_clock = Arc::clone(&real_clock);
            move || real_clock.now().as_millis() as u64
        };
        let mut changed = |info: &InstallInfo| self.publish_install(id, info.clone());
        let mut runner =
            Runner { folder: machines.join(id), info, cancel, clock: &clock, changed: &mut changed };
        if config.install.fake {
            let mut network = FakeNetwork;
            let mut control = FakeGuest::quick(runner.info.guest);
            // Slower on request, so a person can watch the screens go through the steps.
            if let Some(ms) =
                std::env::var("GUESTHOUSE_INSTALL_FAKE_PAUSE_MS").ok().and_then(|v| v.parse().ok())
            {
                control.real_pause_ms = ms;
            }
            let mut steps = RealSteps { setup, transport: &mut network, control: &mut control };
            runner.run(&mut steps);
        } else {
            let mut network = CurlTransport;
            let mut control = AgentControl {
                agent: self.clone(),
                tools,
                run_dir: store::run_folder(&machines, id),
                clock: real_clock,
            };
            let mut steps = RealSteps { setup, transport: &mut network, control: &mut control };
            runner.run(&mut steps);
        }
    }
}

/// The desktop: what was asked for with this agent, or what the install record kept, or weston.
fn desktop_for(info: &InstallInfo, tools: &Tools) -> crate::install::ubuntu::Desktop {
    tools
        .desktop
        .as_deref()
        .or(info.desktop.as_deref())
        .and_then(crate::install::ubuntu::Desktop::parse)
        .unwrap_or_default()
}

/// The disk an install makes: the stand-in's small one, or what was asked for with this agent, or
/// what the install record kept (so an agent that took the install up again makes the same disk),
/// or the guest's default.
fn disk_gb_for(info: &InstallInfo, tools: &Tools, fake: bool) -> u64 {
    if fake {
        return FAKE_DISK_GB;
    }
    tools.sizes.disk_gb.or(info.disk_gb).unwrap_or(default_sizes(info.guest, 4).2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(guest: Guest, kept: Option<u64>) -> InstallInfo {
        let mut info = InstallInfo::new("m", &Plan::for_guest(guest), Source::Download);
        info.disk_gb = kept;
        info
    }

    #[test]
    fn an_agent_that_took_the_install_up_again_makes_the_disk_the_record_kept() {
        let fresh_agent = Tools::default();
        assert_eq!(disk_gb_for(&info(Guest::Windows, Some(96)), &fresh_agent, false), 96);
        // With nothing kept (an older record) it is the guest's default, as before.
        assert_eq!(disk_gb_for(&info(Guest::Windows, None), &fresh_agent, false), 64);
        assert_eq!(disk_gb_for(&info(Guest::Ubuntu, None), &fresh_agent, false), 32);
    }

    #[test]
    fn what_this_agent_was_asked_wins_and_the_stand_in_stays_small() {
        let asked =
            Tools { sizes: Sizes { cores: None, memory_mb: None, disk_gb: Some(40) }, ..Tools::default() };
        assert_eq!(disk_gb_for(&info(Guest::Windows, Some(96)), &asked, false), 40);
        assert_eq!(disk_gb_for(&info(Guest::Windows, Some(96)), &asked, true), FAKE_DISK_GB);
    }

    #[test]
    fn the_record_keeps_the_disk_and_not_a_password() {
        let json = serde_json::to_string(&info(Guest::Windows, Some(96))).unwrap();
        assert!(json.contains("\"disk_gb\":96"));
        assert!(!json.to_lowercase().contains("password"));
        let back: InstallInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back.disk_gb, Some(96));
        // An older record, with no disk in it, still loads.
        let older = json.replace(",\"disk_gb\":96", "");
        assert_eq!(serde_json::from_str::<InstallInfo>(&older).unwrap().disk_gb, None);
    }
}
