// SPDX-License-Identifier: GPL-2.0-or-later
//! Taking an install one step after another: starting each, passing its progress on, saving the
//! state after every change (so the app can be closed and opened again), and stopping where
//! something fails or the person says stop. What each step does is `Steps`; the runner knows only
//! the order and the state.

use super::plan::Step;
use super::state::{self, InstallInfo, Phase};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Why a step could not be done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepError {
    /// The system's or the guest's own code, when it has one.
    pub code: Option<String>,
    pub message: String,
}

impl StepError {
    pub fn new(message: impl Into<String>) -> StepError {
        StepError { code: None, message: message.into() }
    }

    pub fn coded(code: &str, message: impl Into<String>) -> StepError {
        StepError { code: Some(code.to_string()), message: message.into() }
    }
}

/// How a step ended, when it did not fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    Done,
    /// It was told to stop, by the progress call saying no.
    Stopped,
}

/// How far a step says it has got.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Report {
    /// Bytes done and, if known, bytes in all: a download.
    Bytes(u64, Option<u64>),
    /// A guess from 0 to 1, for what cannot count: the guest's own installer.
    Fraction(f64),
}

/// A step's way to say how far it has got. It answers whether to go on.
pub type Progress<'a> = &'a mut dyn FnMut(Report) -> bool;

/// Progress is saved and told at most this often. A download reports every chunk it gets, some
/// thousand a second, and each told change goes to the app as an event and to disk as the state.
/// The step's end is always told, as the run records it after the step.
pub const PROGRESS_EVERY_MS: u64 = 100;

pub trait Steps {
    /// Does one step. `info` is the state as it is, for what a step needs of earlier ones (the
    /// source of the installer, the guest).
    fn run(&mut self, step: Step, info: &InstallInfo, progress: Progress) -> Result<Ended, StepError>;
}

/// Everything a run needs besides the steps.
pub struct Runner<'a> {
    /// The machine's folder, where the state is kept.
    pub folder: PathBuf,
    pub info: InstallInfo,
    /// Set by whoever stops the install.
    pub cancel: Arc<AtomicBool>,
    /// Milliseconds on any steady clock.
    pub clock: &'a dyn Fn() -> u64,
    /// Told after every change, with the state as it now is.
    pub changed: &'a mut dyn FnMut(&InstallInfo),
}

impl Runner<'_> {
    fn record(&mut self) {
        let _ = state::save(&self.folder, &self.info);
        (self.changed)(&self.info);
    }

    /// Runs until the install is done, has failed, or was stopped. Steps already done are not done
    /// again, so this is also how an install goes on.
    pub fn run(&mut self, steps: &mut dyn Steps) {
        self.info.resume();
        self.record();
        while self.info.state == Phase::Running {
            let Some(step) = self.info.next_step() else {
                break;
            };
            if self.cancel.load(Ordering::Relaxed) {
                self.info.cancel();
                break;
            }
            self.info.start_step(step, (self.clock)());
            self.record();
            let snapshot = self.info.clone();
            let cancel = self.cancel.clone();
            let clock = self.clock;
            let (info, folder, changed) = (&mut self.info, &self.folder, &mut *self.changed);
            let mut told_at: Option<u64> = None;
            let mut progress = |report: Report| {
                let now = clock();
                match report {
                    Report::Bytes(done, total) => info.progress(step, done, total, now),
                    Report::Fraction(fraction) => info.progress_fraction(step, fraction),
                }
                if told_at.is_none_or(|at| now.saturating_sub(at) >= PROGRESS_EVERY_MS) {
                    told_at = Some(now);
                    let _ = state::save(folder, info);
                    changed(info);
                }
                !cancel.load(Ordering::Relaxed)
            };
            match steps.run(step, &snapshot, &mut progress) {
                Ok(Ended::Done) => self.info.finish_step(step),
                Ok(Ended::Stopped) => self.info.cancel(),
                Err(error) => self.info.fail(step, error.code, &error.message),
            }
            self.record();
        }
        self.record();
    }
}

/// The folder an install's state is kept in, for the callers that only have the machines folder.
pub fn machine_folder(machines: &Path, id: &str) -> PathBuf {
    machines.join(id)
}

#[cfg(test)]
mod tests;
