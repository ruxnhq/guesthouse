// SPDX-License-Identifier: GPL-2.0-or-later
//! Where an install stands, as data: each step's status and progress, a failure with the choices it
//! offers, and the moves between them. Everything here is pure (no clock, no files except `save`
//! and `load`), so the screens and the tests read the same thing. It is written to `install.json`
//! in the machine's folder as it changes, so an install carries on after the app was closed.

use super::plan::{Guest, Plan, Step};
use guesthouse_basics::json_file::write_json;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// The file in the machine's folder that holds the state.
pub const STATE_FILE: &str = "install.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepStatus {
    Pending,
    Running,
    Done,
    Failed,
    /// Left out on the person's word (the graphics driver, after it failed).
    Skipped,
}

/// How far a step has got. What is not known is left out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    /// 0 to 1.
    pub fraction: f64,
    pub bytes_done: Option<u64>,
    pub bytes_total: Option<u64>,
    /// An estimate from the pace so far, in seconds.
    pub seconds_left: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StepState {
    pub step: Step,
    pub status: StepStatus,
    pub progress: Progress,
    /// When it began, in milliseconds on the caller's clock; not kept in the file.
    #[serde(skip)]
    pub started_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Running,
    Failed,
    Done,
    Cancelled,
}

/// What the person may do about a failure, as the failure screen offers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Choice {
    /// Try the step again; for the download it goes on from where it stopped.
    TryAgain,
    /// Pick an installer already on the Mac, in place of downloading.
    ChooseInstaller,
    /// Open the machine without the graphics driver.
    OpenWithoutDriver,
    /// Send a report with the machine's log.
    SendReport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    pub step: Step,
    /// The guest's or the system's own code, such as `0x800b0109`, when there is one.
    pub code: Option<String>,
    /// A sentence for the person.
    pub reason: String,
    pub choices: Vec<Choice>,
}

/// Where the installer comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Source {
    Download,
    File { path: String },
}

/// The state of one install, as in a listing and as an event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstallInfo {
    /// The machine's folder name.
    pub machine: String,
    pub guest: Guest,
    pub state: Phase,
    pub steps: Vec<StepState>,
    /// The step in hand, while there is one.
    pub current: Option<Step>,
    pub failure: Option<Failure>,
    /// For the whole install, in seconds.
    pub seconds_left: Option<u64>,
    pub source: Source,
    /// The guest account's name, for the screens that say how to sign in. The password is not
    /// here or anywhere on disk: it is in the Mac's Keychain, kept by the app.
    #[serde(default)]
    pub account: String,
    /// The disk size the person chose (or the guest's default), kept so an install taken up again
    /// by a new agent makes the same disk. Not secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_gb: Option<u64>,
    /// Ubuntu's desktop as the person chose it (`gnome`, `plasma`, `weston`), kept for the same
    /// reason. Left out, weston.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desktop: Option<String>,
    /// While the guest's installer is running: for how many minutes its disk has written nothing,
    /// once that is a while. The screen says it under the bar, as a hint that a stopped install and a
    /// working one differ; it is not an error and goes with the step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quiet_minutes: Option<u64>,
}

/// A step's usual time, in seconds, for guessing what is left before a pace is known.
fn usual_seconds(guest: Guest, step: Step) -> u64 {
    match (guest, step) {
        (Guest::Windows, Step::Fetch) => 600,
        (Guest::Ubuntu, Step::Fetch) => 300,
        (_, Step::MakeDisk) => 20,
        (Guest::Windows, Step::InstallSystem) => 1500,
        (Guest::Ubuntu, Step::InstallSystem) => 600,
        (Guest::Windows, Step::InstallDriver) => 120,
        (Guest::Ubuntu, Step::InstallDriver) => 180,
        (_, Step::FirstStart) => 90,
    }
}

/// Below this much done the pace says little, so the usual time is used.
const MIN_FRACTION_FOR_PACE: f64 = 0.02;

impl InstallInfo {
    pub fn new(machine: &str, plan: &Plan, source: Source) -> InstallInfo {
        let steps = plan
            .steps
            .iter()
            .map(|step| StepState {
                step: *step,
                status: StepStatus::Pending,
                progress: Progress::default(),
                started_ms: None,
            })
            .collect();
        let mut info = InstallInfo {
            machine: machine.to_string(),
            guest: plan.guest,
            state: Phase::Running,
            steps,
            current: None,
            failure: None,
            seconds_left: None,
            source,
            account: String::new(),
            disk_gb: None,
            desktop: None,
            quiet_minutes: None,
        };
        info.refresh();
        info
    }

    fn step_mut(&mut self, step: Step) -> &mut StepState {
        self.steps.iter_mut().find(|s| s.step == step).expect("every step of the plan is in the state")
    }

    pub fn status_of(&self, step: Step) -> StepStatus {
        self.steps.iter().find(|s| s.step == step).map_or(StepStatus::Pending, |s| s.status)
    }

    /// The next step to do: the first that is neither done nor skipped.
    pub fn next_step(&self) -> Option<Step> {
        self.steps
            .iter()
            .find(|s| !matches!(s.status, StepStatus::Done | StepStatus::Skipped))
            .map(|s| s.step)
    }

    pub fn start_step(&mut self, step: Step, now_ms: u64) {
        let state = self.step_mut(step);
        state.status = StepStatus::Running;
        state.started_ms = Some(now_ms);
        self.current = Some(step);
        self.refresh();
    }

    /// A step's progress. With a total it is bytes, and the time left comes from the pace.
    pub fn progress(&mut self, step: Step, done: u64, total: Option<u64>, now_ms: u64) {
        let guest = self.guest;
        let state = self.step_mut(step);
        let fraction =
            total.filter(|t| *t > 0).map_or(state.progress.fraction, |t| (done as f64 / t as f64).min(1.0));
        let seconds_left = match (state.started_ms, total) {
            (Some(start), Some(_)) if fraction >= MIN_FRACTION_FOR_PACE && now_ms > start => {
                let elapsed = (now_ms - start) as f64 / 1000.0;
                Some((elapsed * (1.0 - fraction) / fraction).round() as u64)
            }
            _ => Some(((1.0 - fraction) * usual_seconds(guest, step) as f64).round() as u64),
        };
        state.progress =
            Progress { fraction, bytes_done: total.map(|_| done), bytes_total: total, seconds_left };
        self.refresh();
    }

    /// A step that cannot count bytes, such as the guest's own installer: how far it is guessed to
    /// have got, from 0 to 1, and the time left from its usual time.
    pub fn progress_fraction(&mut self, step: Step, fraction: f64) {
        let guest = self.guest;
        let state = self.step_mut(step);
        let fraction = fraction.clamp(0.0, 1.0);
        let left = ((1.0 - fraction) * usual_seconds(guest, step) as f64).round() as u64;
        state.progress = Progress { fraction, bytes_done: None, bytes_total: None, seconds_left: Some(left) };
        self.refresh();
    }

    pub fn finish_step(&mut self, step: Step) {
        self.quiet_minutes = None;
        let state = self.step_mut(step);
        state.status = StepStatus::Done;
        state.progress = Progress { fraction: 1.0, seconds_left: Some(0), ..state.progress };
        self.current = self.next_step();
        if self.current.is_none() {
            self.state = Phase::Done;
        }
        self.refresh();
    }

    /// A step stopped. The choices depend on the step, as the failure screen says: the download
    /// can go on or take a chosen installer; the driver can be left out; every step but the first
    /// can send a report.
    pub fn fail(&mut self, step: Step, code: Option<String>, reason: &str) {
        self.step_mut(step).status = StepStatus::Failed;
        let choices = match step {
            Step::Fetch => vec![Choice::TryAgain, Choice::ChooseInstaller],
            Step::InstallDriver => vec![Choice::TryAgain, Choice::OpenWithoutDriver, Choice::SendReport],
            Step::MakeDisk | Step::InstallSystem | Step::FirstStart => {
                vec![Choice::TryAgain, Choice::SendReport]
            }
        };
        self.failure = Some(Failure { step, code, reason: reason.to_string(), choices });
        self.state = Phase::Failed;
        self.current = Some(step);
        self.refresh();
    }

    /// Try again: the failed step is to be done again, from the top, and keeps what it made.
    pub fn retry(&mut self) -> bool {
        let Some(failure) = self.failure.take() else {
            return false;
        };
        let state = self.step_mut(failure.step);
        state.status = StepStatus::Pending;
        state.progress = Progress::default();
        self.state = Phase::Running;
        self.current = Some(failure.step);
        self.refresh();
        true
    }

    /// Take an installer the person chose, in place of downloading one.
    pub fn choose_installer(&mut self, path: &str) -> bool {
        let chosen = self.failure.as_ref().is_some_and(|f| f.choices.contains(&Choice::ChooseInstaller));
        if !chosen {
            return false;
        }
        self.source = Source::File { path: path.to_string() };
        self.retry()
    }

    /// Leave the graphics driver out and go on to the first start. Only offered after it failed.
    pub fn open_without_driver(&mut self) -> bool {
        let offered = self.failure.as_ref().is_some_and(|f| f.choices.contains(&Choice::OpenWithoutDriver));
        if !offered {
            return false;
        }
        self.failure = None;
        self.step_mut(Step::InstallDriver).status = StepStatus::Skipped;
        self.state = Phase::Running;
        self.current = self.next_step();
        self.refresh();
        true
    }

    pub fn cancel(&mut self) {
        self.state = Phase::Cancelled;
        for step in &mut self.steps {
            if step.status == StepStatus::Running {
                step.status = StepStatus::Pending;
            }
        }
        self.seconds_left = None;
    }

    /// An install found as the app was closed: what was running is to be done again from where it
    /// had got, and the rest is as it was. A finished, failed or cancelled one is left alone.
    pub fn resume(&mut self) {
        if self.state != Phase::Running {
            return;
        }
        for step in &mut self.steps {
            if step.status == StepStatus::Running {
                step.status = StepStatus::Pending;
            }
        }
        self.current = self.next_step();
        self.refresh();
    }

    /// The time left for the whole install: the step in hand by its pace, the rest by their usual.
    fn refresh(&mut self) {
        if self.state != Phase::Running {
            self.seconds_left = None;
            return;
        }
        let guest = self.guest;
        self.seconds_left = Some(
            self.steps
                .iter()
                .map(|s| match s.status {
                    StepStatus::Done | StepStatus::Skipped => 0,
                    StepStatus::Running => {
                        s.progress.seconds_left.unwrap_or_else(|| usual_seconds(guest, s.step))
                    }
                    _ => usual_seconds(guest, s.step),
                })
                .sum(),
        );
    }

    /// The text for the sidebar: "Installing, 38%", from how far the whole has got.
    pub fn sidebar_text(&self) -> String {
        match self.state {
            Phase::Done => "Ready".to_string(),
            Phase::Failed => "Needs attention".to_string(),
            Phase::Cancelled => "Install stopped".to_string(),
            Phase::Running => {
                let shown = self.steps.iter().filter(|s| s.step != Step::MakeDisk).count().max(1) as f64;
                let done: f64 = self
                    .steps
                    .iter()
                    .filter(|s| s.step != Step::MakeDisk)
                    .map(|s| match s.status {
                        StepStatus::Done | StepStatus::Skipped => 1.0,
                        StepStatus::Running => s.progress.fraction,
                        _ => 0.0,
                    })
                    .sum();
                format!("Installing, {}%", (done / shown * 100.0).round() as u32)
            }
        }
    }
}

/// Writes the state through a temporary file, so a crash never leaves half of it.
pub fn save(folder: &Path, info: &InstallInfo) -> std::io::Result<()> {
    write_json(&folder.join(STATE_FILE), info)
}

/// The state kept in a machine's folder, if there is one that can be read.
pub fn load(folder: &Path) -> Option<InstallInfo> {
    serde_json::from_str(&fs::read_to_string(folder.join(STATE_FILE)).ok()?).ok()
}

#[cfg(test)]
mod tests;
