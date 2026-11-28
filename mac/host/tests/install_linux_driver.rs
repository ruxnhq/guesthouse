// SPDX-License-Identifier: GPL-2.0-or-later
//! Ubuntu's install through the agent with a Linux driver package, with the stand-in network and
//! guest (`install::fake`): a stand-in package whose `install.sh` only says its line, which the
//! stand-in guest says as the driver's service would run it. Both ends: the driver installed and
//! the first start, and the driver refused with Try Again and Open Without the Driver.
#![cfg(target_os = "macos")]

mod support;
use guesthouse_core::agent::{Agent, Config, InstallConfig};
use guesthouse_core::install::driver::{LINUX, LINUX_SCRIPT};
use guesthouse_core::install::plan::Step;
use guesthouse_core::install::state::{InstallInfo, Phase, StepStatus};
use guesthouse_core::protocol::Response;
use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

const PASSWORD: &str = "k7mn-p3xq-ab9d-w4ht";
const WAIT: Duration = Duration::from_secs(20);

struct Support(PathBuf);

impl Drop for Support {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

impl Support {
    /// A support folder with a stand-in Linux package whose install.sh says `line`.
    fn new(name: &str, line: &str) -> Support {
        let path = std::env::temp_dir().join(format!("ghl-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(path.join("Machines")).unwrap();
        let package = path.join("linux-driver");
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join(LINUX_SCRIPT), format!("#!/bin/sh\necho \"{line}\" > /dev/ttyAMA0\n"))
            .unwrap();
        fs::write(package.join("guesthouse-pvg-dkms_0.1_all.deb"), "deb").unwrap();
        fs::write(package.join(LINUX.manifest), r#"{"version":"0.1.0","files":[]}"#).unwrap();
        Support(path)
    }

    fn agent(&self) -> Agent {
        Agent::new(Config {
            machines: self.0.join("Machines"),
            qemu: None,
            rmhost: None,
            pvg_host: None,
            install: InstallConfig {
                fake: true,
                driver_dir: Some(self.0.join("linux-driver")),
                firmware_dir: Some(support::firmware_dir(&self.0)),
            },
            signed: false,
            shutdown_timeout: Duration::from_secs(5),
            start_timeout: Duration::from_secs(10),
        })
    }

    fn machine_file(&self, id: &str) -> serde_json::Value {
        let path = self.0.join("Machines").join(id).join("machine.json");
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }
}

fn request(agent: &Agent, line: &str) -> Response {
    serde_json::from_str(&agent.handle(line)).unwrap()
}

fn status(agent: &Agent, id: &str) -> Option<InstallInfo> {
    request(agent, &format!(r#"{{"v":1,"id":1,"op":"install_status","machine":"{id}"}}"#))
        .installs?
        .into_iter()
        .next()
}

fn wait_for(agent: &Agent, id: &str, what: &str, done: impl Fn(&InstallInfo) -> bool) -> InstallInfo {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(info) = status(agent, id).filter(|i| done(i)) {
            return info;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}: {:?}", status(agent, id));
        thread::sleep(Duration::from_millis(20));
    }
}

fn install(agent: &Agent) -> String {
    let line = format!(
        r#"{{"v":1,"id":1,"op":"install","name":"Ubuntu","guest":"ubuntu","password":"{PASSWORD}"}}"#
    );
    let made = request(agent, &line);
    assert!(made.ok, "{:?}", made.error);
    made.machine.unwrap()
}

fn choose(agent: &Agent, id: &str, choice: &str) -> Response {
    request(agent, &format!(r#"{{"v":1,"id":1,"op":"install_choose","machine":"{id}","choice":"{choice}"}}"#))
}

#[test]
fn a_driver_that_installs_is_followed_by_the_first_start_and_the_install_is_done() {
    let support = Support::new("installed", "guesthouse: driver installed");
    let agent = support.agent();
    let id = install(&agent);
    let done = wait_for(&agent, &id, "the install to end", |i| i.state != Phase::Running);
    assert_eq!(done.state, Phase::Done, "{:?}", done.failure);
    assert_eq!(done.status_of(Step::InstallDriver), StepStatus::Done);
    assert_eq!(done.status_of(Step::FirstStart), StepStatus::Done);
    // The machine starts from its own disk from now on: no installer, answers, kernel or stick.
    let file = support.machine_file(&id);
    for key in [
        "installer",
        "answer_disc",
        "installer_kernel",
        "installer_initrd",
        "installer_command_line",
        "stick",
    ] {
        assert!(file.get(key).is_none(), "{key} is still in machine.json");
    }
    assert_eq!(file["kind"], "linux");
}

#[test]
fn a_driver_that_fails_says_its_code_and_offers_try_again_and_open_without_it() {
    let support = Support::new("failed", "guesthouse: driver failed nomodule");
    let agent = support.agent();
    let id = install(&agent);
    let failed = wait_for(&agent, &id, "the driver to fail", |i| i.failure.is_some());
    let failure = failed.failure.unwrap();
    assert_eq!((failure.step, failure.code.as_deref()), (Step::InstallDriver, Some("nomodule")));
    assert!(failure.reason.contains("nomodule"));
    // The install is far enough on that the stick replaced the installer in the machine's file.
    let file = support.machine_file(&id);
    assert_eq!(file["stick"], "stick.img");
    assert!(file.get("installer_kernel").is_none());

    // Try Again starts the machine with the stick once more, and the same package fails again.
    assert!(choose(&agent, &id, "try_again").ok);
    let again = wait_for(&agent, &id, "the second try to fail", |i| i.failure.is_some());
    assert_eq!(again.failure.unwrap().code.as_deref(), Some("nomodule"));

    // Open without it: the first start boots the disk alone, and the install is done.
    assert!(choose(&agent, &id, "open_without_driver").ok);
    let done = wait_for(&agent, &id, "the install to finish", |i| i.state == Phase::Done);
    assert_eq!(done.status_of(Step::InstallDriver), StepStatus::Skipped);
    assert!(support.machine_file(&id).get("stick").is_none());
}
