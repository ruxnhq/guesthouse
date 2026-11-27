// SPDX-License-Identifier: GPL-2.0-or-later
//! Installing a guest through the agent, with the stand-in network and guest (`install::fake`):
//! no download and no machine. Covers the steps in order, a failure and what is chosen at it,
//! stopping, and an install found again by a new agent.
#![cfg(target_os = "macos")]

mod support;
use guesthouse_core::agent::{Agent, Config, InstallConfig};
use guesthouse_core::install::plan::{Guest, Plan, Step};
use guesthouse_core::install::state::{self, InstallInfo, Source, StepStatus};
use guesthouse_core::protocol::Response;
use std::fs::{self, File};
use std::path::PathBuf;
use std::sync::mpsc;
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
    fn new(name: &str) -> Support {
        let path = std::env::temp_dir().join(format!("ghi-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(path.join("Machines")).unwrap();
        // The driver's files, as its build gathers them, and QEMU's firmware.
        let driver = path.join("driver");
        fs::create_dir_all(&driver).unwrap();
        for name in guesthouse_core::install::driver::REQUIRED {
            fs::write(driver.join(name), "x").unwrap();
        }
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
                driver_dir: Some(self.0.join("driver")),
                firmware_dir: Some(support::firmware_dir(&self.0)),
            },
            shutdown_timeout: Duration::from_secs(5),
            start_timeout: Duration::from_secs(10),
        })
    }

    fn iso(&self) -> String {
        let path = self.0.join("Win11_arm64.iso");
        File::create(&path).unwrap().set_len(2_000_000_001).unwrap();
        path.display().to_string()
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

fn install_line(name: &str, guest: &str, file: Option<&str>) -> String {
    let file = file.map(|f| format!(r#","file":"{f}""#)).unwrap_or_default();
    format!(
        r#"{{"v":1,"id":1,"op":"install","name":"{name}","guest":"{guest}","password":"{PASSWORD}"{file}}}"#
    )
}

fn choose(agent: &Agent, id: &str, choice: &str, path: Option<&str>) -> Response {
    let path = path.map(|p| format!(r#","path":"{p}""#)).unwrap_or_default();
    request(
        agent,
        &format!(r#"{{"v":1,"id":1,"op":"install_choose","machine":"{id}","choice":"{choice}"{path}}}"#),
    )
}

#[test]
fn ubuntu_installs_to_the_driver_step_and_opens_without_it_and_every_change_is_told() {
    let support = Support::new("ubuntu");
    let agent = support.agent();
    let (sender, events) = mpsc::channel();
    let _keep = agent.subscribe(sender);
    let made = request(&agent, &install_line("Ubuntu", "ubuntu", None));
    assert!(made.ok, "{:?}", made.error);
    let id = made.machine.unwrap();
    assert_eq!(id, "ubuntu");

    // It goes through the download and the install and stops at the driver, which is not packaged.
    let stopped = wait_for(&agent, &id, "the driver step to fail", |i| i.failure.is_some());
    assert_eq!(stopped.failure.as_ref().unwrap().step, Step::InstallDriver);
    assert_eq!(stopped.failure.as_ref().unwrap().code.as_deref(), Some("no_package"));
    assert_eq!(stopped.status_of(Step::InstallSystem), StepStatus::Done);
    assert!(support.0.join("Downloads/ubuntu/ubuntu-26.04-live-server-arm64.iso").is_file());

    // Open it without the driver: the first start, and it is done.
    assert!(choose(&agent, &id, "open_without_driver", None).ok);
    let done = wait_for(&agent, &id, "the install to finish", |i| i.state == state::Phase::Done);
    assert_eq!(done.status_of(Step::InstallDriver), StepStatus::Skipped);
    // Kept in the machine's folder, and told on the way.
    assert_eq!(state::load(&support.0.join("Machines").join(&id)).unwrap().state, state::Phase::Done);
    let told: Vec<serde_json::Value> = events
        .try_iter()
        .map(|l| serde_json::from_str(&l).unwrap())
        .filter(|v: &serde_json::Value| v["event"] == "install")
        .collect();
    assert!(told.len() > 5, "{}", told.len());
    assert!(told.iter().any(|v| v["failure"]["code"] == "no_package"));
    // A listing has it too.
    let listed = request(&agent, r#"{"v":1,"id":2,"op":"list"}"#).installs.unwrap();
    assert_eq!(listed[0].machine, id);
}

#[test]
fn windows_asks_for_an_installer_and_goes_on_with_the_one_chosen() {
    let support = Support::new("windows");
    let agent = support.agent();
    let id = request(&agent, &install_line("Windows 11", "windows", None)).machine.unwrap();
    let stopped = wait_for(&agent, &id, "the download step to fail", |i| i.failure.is_some());
    let failure = stopped.failure.unwrap();
    assert_eq!((failure.step, failure.code.as_deref()), (Step::Fetch, Some("choose_installer")));

    // Not offered at another point, and a missing path is refused.
    assert!(!choose(&agent, &id, "open_without_driver", None).ok);
    assert!(!choose(&agent, &id, "choose_installer", None).ok);
    assert!(!choose(&agent, &id, "explode", None).ok);

    assert!(choose(&agent, &id, "choose_installer", Some(&support.iso())).ok);
    let done = wait_for(&agent, &id, "the install to finish", |i| i.state == state::Phase::Done);
    assert_eq!(done.source, Source::File { path: support.iso() });
    assert!(done.steps.iter().all(|s| s.status == StepStatus::Done));
}

#[test]
fn an_install_can_be_stopped_and_what_it_made_is_kept() {
    let support = Support::new("cancel");
    let agent = support.agent();
    let id = request(&agent, &install_line("Windows 11", "windows", Some(&support.iso()))).machine.unwrap();
    thread::sleep(Duration::from_millis(40));
    assert!(request(&agent, &format!(r#"{{"v":1,"id":1,"op":"install_cancel","machine":"{id}"}}"#)).ok);
    let stopped = wait_for(&agent, &id, "the install to stop", |i| i.state == state::Phase::Cancelled);
    assert!(stopped.seconds_left.is_none());
    assert!(support.0.join("Machines").join(&id).join("machine.json").is_file());
    // Nothing is running to stop any more.
    assert!(
        !request(&agent, &format!(r#"{{"v":1,"id":1,"op":"install_cancel","machine":"{id}"}}"#)).ok
            || status(&agent, &id).is_some()
    );
}

#[test]
fn a_name_that_is_taken_or_a_guest_that_is_not_one_is_refused() {
    let support = Support::new("refused");
    let agent = support.agent();
    assert!(request(&agent, &install_line("Ubuntu", "ubuntu", None)).ok);
    let again = request(&agent, &install_line("Ubuntu", "ubuntu", None));
    assert!(!again.ok);
    assert!(again.error.unwrap().contains("already exists"));
    let wrong = request(&agent, &install_line("Other", "plan9", None));
    assert!(!wrong.ok);
    assert!(wrong.error.unwrap().contains("not a guest"));
}

#[test]
fn a_new_agent_takes_up_an_install_that_was_running_and_does_not_fetch_again() {
    let support = Support::new("resume");
    let machines = support.0.join("Machines");
    let id = guesthouse_core::install::machine::create_machine(&machines, "Ubuntu", Guest::Ubuntu, 4, 4096)
        .unwrap();
    // As the last agent left it: the download and the disk done, the install itself running.
    let mut info = InstallInfo::new(&id, &Plan::for_guest(Guest::Ubuntu), Source::Download);
    for step in [Step::Fetch, Step::MakeDisk] {
        info.start_step(step, 0);
        info.finish_step(step);
    }
    info.start_step(Step::InstallSystem, 0);
    let folder = machines.join(&id);
    state::save(&folder, &info).unwrap();
    // The first steps' files, which the stand-in installer needs from them.
    fs::write(folder.join("disk.img"), b"").unwrap();
    fs::write(folder.join("code.fd"), b"c").unwrap();
    fs::write(folder.join("vars.fd"), b"v").unwrap();

    let agent = support.agent();
    let stopped = wait_for(&agent, &id, "the resumed install to reach the driver", |i| i.failure.is_some());
    // The new agent does not have the password (it is only ever in an agent's memory), so it says
    // so and takes it again with Try Again, which the app does from the Keychain.
    let failure = stopped.failure.unwrap();
    assert_eq!((failure.step, failure.code.as_deref()), (Step::InstallSystem, Some("password_missing")));
    let again = request(
        &agent,
        &format!(
            r#"{{"v":1,"id":1,"op":"install_choose","machine":"{id}","choice":"try_again","password":"{PASSWORD}"}}"#
        ),
    );
    assert!(again.ok, "{:?}", again.error);
    let stopped = wait_for(&agent, &id, "the resumed install to reach the driver", |i| {
        i.failure.as_ref().is_some_and(|f| f.step == Step::InstallDriver)
    });
    assert_eq!(stopped.failure.unwrap().step, Step::InstallDriver);
    assert!(!support.0.join("Downloads").exists(), "the installer was fetched again");
}

#[test]
fn sizes_chosen_with_customise_are_the_machines_sizes() {
    let support = Support::new("sizes");
    let agent = support.agent();
    let line = r#"{"v":1,"id":1,"op":"install","name":"Small","guest":"ubuntu","cores":2,"memory_mb":3072,"disk_gb":20,"password":"k7mn-p3xq-ab9d-w4ht"}"#;
    let made = request(&agent, line);
    assert!(made.ok, "{:?}", made.error);
    let id = made.machine.unwrap();
    let settings = fs::read_to_string(support.0.join("Machines").join(&id).join("machine.json")).unwrap();
    let settings: serde_json::Value = serde_json::from_str(&settings).unwrap();
    assert_eq!(settings["cores"], 2);
    assert_eq!(settings["memory_mb"], 3072);
    // Let the stand-in install reach its end, so the test leaves nothing running.
    wait_for(&agent, &id, "the install to stop", |i| i.failure.is_some());
}
