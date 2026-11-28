// SPDX-License-Identifier: GPL-2.0-or-later
use super::*;
use crate::install::plan::{Guest, Plan};
use crate::install::state::{Choice, Source, StepStatus};
use std::collections::VecDeque;
use std::sync::Mutex;

/// Steps that do what a script says, one answer for each time they are asked.
struct Scripted {
    answers: VecDeque<Result<Ended, StepError>>,
    asked: Vec<Step>,
    /// Reports this much progress before answering.
    report: Vec<Report>,
}

impl Scripted {
    fn new(answers: Vec<Result<Ended, StepError>>) -> Scripted {
        Scripted { answers: answers.into(), asked: vec![], report: vec![] }
    }
}

impl Steps for Scripted {
    fn run(&mut self, step: Step, _info: &InstallInfo, progress: Progress) -> Result<Ended, StepError> {
        self.asked.push(step);
        for report in self.report.clone() {
            if !progress(report) {
                return Ok(Ended::Stopped);
            }
        }
        self.answers.pop_front().unwrap_or(Ok(Ended::Done))
    }
}

fn dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("gh-runner-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn runner<'a>(
    folder: &Path,
    changes: &'a Mutex<Vec<InstallInfo>>,
    changed: &'a mut dyn FnMut(&InstallInfo),
    clock: &'a dyn Fn() -> u64,
) -> Runner<'a> {
    let _ = changes;
    Runner {
        folder: folder.to_path_buf(),
        info: InstallInfo::new("m", &Plan::for_guest(Guest::Ubuntu), Source::Download),
        cancel: Arc::new(AtomicBool::new(false)),
        clock,
        changed,
    }
}

#[test]
fn every_step_is_done_in_order_and_the_state_is_kept_after_each_change() {
    let folder = dir("order");
    let seen = Mutex::new(Vec::new());
    let mut changed = |info: &InstallInfo| seen.lock().unwrap().push(info.clone());
    let clock = || 0;
    let mut runner = runner(&folder, &seen, &mut changed, &clock);
    let mut steps = Scripted::new(vec![]);
    runner.run(&mut steps);
    assert_eq!(steps.asked, Step::ALL);
    assert_eq!(runner.info.state, Phase::Done);
    assert_eq!(state::load(&folder).unwrap().state, Phase::Done);
    // The screens heard of each step starting.
    let running: Vec<Option<Step>> = seen.lock().unwrap().iter().map(|i| i.current).collect();
    assert!(running.contains(&Some(Step::InstallDriver)));
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn progress_from_a_step_reaches_the_state() {
    let folder = dir("progress");
    let seen = Mutex::new(Vec::new());
    let mut changed = |info: &InstallInfo| seen.lock().unwrap().push(info.clone());
    let clock = || 5_000;
    let mut runner = runner(&folder, &seen, &mut changed, &clock);
    let mut steps = Scripted::new(vec![]);
    steps.report = vec![Report::Bytes(500, Some(1000))];
    runner.run(&mut steps);
    let halves = seen.lock().unwrap().iter().filter(|i| i.steps[0].progress.fraction == 0.5).count();
    assert!(halves >= 1);
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn a_download_s_many_reports_are_told_at_most_every_tenth_of_a_second_and_its_end_always() {
    let folder = dir("pace");
    let seen = Mutex::new(Vec::new());
    let mut changed = |info: &InstallInfo| seen.lock().unwrap().push(info.clone());
    // A thousand chunks within 50 ms of a clock that moves 50 microseconds a chunk.
    let tick = std::sync::atomic::AtomicU64::new(0);
    let clock = || tick.fetch_add(1, Ordering::Relaxed) / 20;
    let mut runner = runner(&folder, &seen, &mut changed, &clock);
    let mut steps = Scripted::new(vec![]);
    steps.report = (1..=1000).map(|done| Report::Bytes(done, Some(1000))).collect();
    runner.run(&mut steps);
    let seen = seen.lock().unwrap();
    let during = seen
        .iter()
        .filter(|i| {
            let fraction = i.steps[0].progress.fraction;
            fraction > 0.0 && fraction < 1.0
        })
        .count();
    assert!(during <= 2, "{during} changes told for 1000 chunks in 50 ms");
    assert!(seen.iter().any(|i| i.steps[0].progress.fraction == 1.0), "the end is told");
    drop(seen);
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn a_failure_stops_the_run_with_its_code_and_choices_and_try_again_goes_on_from_there() {
    let folder = dir("fail");
    let seen = Mutex::new(Vec::new());
    let mut changed = |info: &InstallInfo| seen.lock().unwrap().push(info.clone());
    let clock = || 0;
    let mut runner = runner(&folder, &seen, &mut changed, &clock);
    let mut steps = Scripted::new(vec![
        Ok(Ended::Done),
        Ok(Ended::Done),
        Ok(Ended::Done),
        Err(StepError::coded("0x80070005", "Windows turned the driver away.")),
    ]);
    runner.run(&mut steps);
    assert_eq!(runner.info.state, Phase::Failed);
    let failure = runner.info.failure.clone().unwrap();
    assert_eq!((failure.step, failure.code.as_deref()), (Step::InstallDriver, Some("0x80070005")));
    assert!(failure.choices.contains(&Choice::OpenWithoutDriver));
    assert_eq!(steps.asked.len(), 4);

    // Try Again: only the failed step and what is after it are done.
    assert!(runner.info.retry());
    runner.run(&mut steps);
    assert_eq!(runner.info.state, Phase::Done);
    assert_eq!(&steps.asked[4..], [Step::InstallDriver, Step::FirstStart]);
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn opening_without_the_driver_goes_on_without_asking_for_it_again() {
    let folder = dir("skip");
    let seen = Mutex::new(Vec::new());
    let mut changed = |info: &InstallInfo| seen.lock().unwrap().push(info.clone());
    let clock = || 0;
    let mut runner = runner(&folder, &seen, &mut changed, &clock);
    let mut steps = Scripted::new(vec![
        Ok(Ended::Done),
        Ok(Ended::Done),
        Ok(Ended::Done),
        Err(StepError::new("no driver")),
    ]);
    runner.run(&mut steps);
    assert!(runner.info.open_without_driver());
    runner.run(&mut steps);
    assert_eq!(runner.info.state, Phase::Done);
    assert_eq!(runner.info.status_of(Step::InstallDriver), StepStatus::Skipped);
    assert_eq!(steps.asked.last(), Some(&Step::FirstStart));
    assert_eq!(steps.asked.iter().filter(|s| **s == Step::InstallDriver).count(), 1);
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn stopping_ends_the_step_in_hand_and_leaves_the_rest_for_later() {
    let folder = dir("stop");
    let seen = Mutex::new(Vec::new());
    let mut changed = |info: &InstallInfo| seen.lock().unwrap().push(info.clone());
    let clock = || 0;
    let mut runner = runner(&folder, &seen, &mut changed, &clock);
    runner.cancel.store(true, Ordering::Relaxed);
    let mut steps = Scripted::new(vec![]);
    runner.run(&mut steps);
    assert_eq!(runner.info.state, Phase::Cancelled);
    assert!(steps.asked.is_empty());

    // Stopped from inside a step, by the progress call.
    let mut again = self::runner(&folder, &seen, &mut changed, &clock);
    let mut steps = Scripted::new(vec![]);
    steps.report = vec![Report::Bytes(1, Some(10))];
    let flag = again.cancel.clone();
    flag.store(false, Ordering::Relaxed);
    struct StopAfterFirst(Arc<AtomicBool>, Scripted);
    impl Steps for StopAfterFirst {
        fn run(&mut self, step: Step, info: &InstallInfo, progress: Progress) -> Result<Ended, StepError> {
            self.0.store(true, Ordering::Relaxed);
            self.1.run(step, info, progress)
        }
    }
    again.run(&mut StopAfterFirst(flag, steps));
    assert_eq!(again.info.state, Phase::Cancelled);
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn an_install_found_half_done_is_gone_on_from_by_a_new_runner() {
    let folder = dir("resume");
    let seen = Mutex::new(Vec::new());
    let mut changed = |info: &InstallInfo| seen.lock().unwrap().push(info.clone());
    let clock = || 0;
    {
        let mut first = runner(&folder, &seen, &mut changed, &clock);
        let mut steps =
            Scripted::new(vec![Ok(Ended::Done), Ok(Ended::Done), Err(StepError::new("closed mid-way"))]);
        first.run(&mut steps);
    }
    // The app closed with the third step running; as found, it is running.
    let mut found = state::load(&folder).unwrap();
    found.state = Phase::Running;
    found.failure = None;
    found.steps[2].status = StepStatus::Running;
    let mut second = runner(&folder, &seen, &mut changed, &clock);
    second.info = found;
    let mut steps = Scripted::new(vec![]);
    second.run(&mut steps);
    assert_eq!(steps.asked, [Step::InstallSystem, Step::InstallDriver, Step::FirstStart]);
    assert_eq!(second.info.state, Phase::Done);
    let _ = std::fs::remove_dir_all(folder);
}
