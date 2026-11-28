// SPDX-License-Identifier: GPL-2.0-or-later
//! Where the process's threads are, and a line when one of them stands still.
//!
//! The thread that serves the guest and the one that draws into the window mark each step they
//! take; the window's first thread, AppKit's, and the queue the framework calls the display's
//! handlers on are asked every so often to run a little work, and `shim/window.m` and pvg-sys's
//! shim keep which of their functions they are in. A thread that has stood in one step for
//! [`STILL`], or a first thread that has not run the work for as long, gets a line that says where
//! each of the three was, and another when all of them go on. A guest's register access waits on
//! the serving thread, and QEMU's whole machine on that access, so these lines are what is left to
//! read when a guest stops.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// How long a thread stands in one step before it is said.
const STILL: Duration = Duration::from_secs(2);
/// How often the threads are looked at.
const LOOK: Duration = Duration::from_millis(500);
/// The first of these lines are worth reading; a thread that stands still for good gives one.
const LINES_SHOWN: u64 = 32;

/// What a thread is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Step {
    /// Waiting for work: not standing still, however long.
    Waiting = 0,
    /// The serving thread answers a line of the control socket.
    Control,
    /// It asks the first thread how the window is.
    WindowState,
    /// A register read, a write or a reset of the guest, in the framework.
    Read,
    Write,
    Reset,
    /// A new mode list for the window's size, in the framework.
    Modes,
    /// The drawing thread draws a picture into the window's layer, or a copy of it.
    Draw,
    Shown,
    /// It tells the first thread of the picture's size or the cursor.
    Tell,
}

const NAMES: [&str; 10] = [
    "waiting",
    "a line of the control socket",
    "asking the window's thread how the window is",
    "a register read",
    "a register write",
    "a reset",
    "a mode list",
    "drawing into the window's layer",
    "drawing and copying the window's layer",
    "telling the window's thread",
];

impl Step {
    fn name(self) -> &'static str {
        NAMES[self as usize]
    }

    fn from(number: u8) -> Self {
        [
            Step::Waiting,
            Step::Control,
            Step::WindowState,
            Step::Read,
            Step::Write,
            Step::Reset,
            Step::Modes,
            Step::Draw,
            Step::Shown,
            Step::Tell,
        ]
        .get(usize::from(number))
        .copied()
        .unwrap_or(Step::Waiting)
    }
}

/// A thread's step and since when, in milliseconds from the process's start.
pub struct Mark {
    step: AtomicU8,
    since: AtomicU64,
}

impl Mark {
    const fn new() -> Self {
        Self { step: AtomicU8::new(0), since: AtomicU64::new(0) }
    }

    pub fn set(&self, step: Step) {
        self.since.store(now(), Ordering::Relaxed);
        self.step.store(step as u8, Ordering::Relaxed);
    }

    /// The step, and how long the thread has been in it, unless it is waiting.
    fn busy(&self) -> Option<(Step, u64)> {
        let step = Step::from(self.step.load(Ordering::Relaxed));
        let since = self.since.load(Ordering::Relaxed);
        (step != Step::Waiting).then(|| (step, now().saturating_sub(since)))
    }
}

pub static SERVING: Mark = Mark::new();
pub static DRAWING: Mark = Mark::new();
/// Mode lists handed to a queue that the framework has not taken yet, and since when the oldest
/// has waited.
static MODES_PENDING: AtomicU64 = AtomicU64::new(0);
static MODES: Mark = Mark::new();

/// A mode list was handed to a queue.
pub fn modes_queued() {
    if MODES_PENDING.fetch_add(1, Ordering::Relaxed) == 0 {
        MODES.set(Step::Modes);
    }
}

/// The framework took a mode list handed to a queue, or threw on it.
pub fn modes_done() {
    match MODES_PENDING.fetch_sub(1, Ordering::Relaxed) {
        1 => MODES.set(Step::Waiting),
        _ => MODES.set(Step::Modes),
    }
}

fn now() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// How the window's first thread is doing: how long ago it last ran the work it was given, the
/// function of the shim it is in and the one it last left, and its run loop's mode.
pub struct Main {
    pub answered_ago: Duration,
    pub inside: String,
    pub left: String,
    pub mode: String,
}

/// Looks at the threads for as long as the process lives, from a thread of its own. `main` asks
/// the first thread to run its work again and says how it did, if there is a window; `device`
/// says how the device is, for the end of the line.
pub fn start(main: Option<fn() -> Main>, device: Box<dyn Fn() -> String + Send>) {
    now();
    std::thread::spawn(move || {
        let mut still_since: Option<Instant> = None;
        let mut said = 0u64;
        loop {
            std::thread::sleep(LOOK);
            let main = main.map(|look| look());
            let display = guesthouse_pvg_sys::display_look();
            let serving = SERVING.busy();
            let drawing = DRAWING.busy();
            let modes = MODES.busy();
            let still =
                |busy: Option<(Step, u64)>| busy.is_some_and(|(_, ms)| ms >= STILL.as_millis() as u64);
            let main_still = main.as_ref().is_some_and(|main| main.answered_ago >= STILL);
            let display_still = display.is_some_and(|(answered_ago, _)| answered_ago >= STILL);
            if main_still || display_still || still(serving) || still(drawing) || still(modes) {
                if still_since.is_none() {
                    still_since = Some(Instant::now());
                    if said < LINES_SHOWN {
                        eprintln!(
                            "pvg-host: a thread stands still: {}; {}",
                            describe(main.as_ref(), display, serving, drawing, modes),
                            device()
                        );
                    } else if said == LINES_SHOWN {
                        eprintln!("pvg-host: more threads stand still, not shown");
                    }
                    said += 1;
                }
            } else if let Some(since) = still_since.take()
                && said <= LINES_SHOWN
            {
                eprintln!(
                    "pvg-host: the threads go on, {:.1} s after the line that one stood still",
                    since.elapsed().as_secs_f64()
                );
            }
        }
    });
}

fn describe(
    main: Option<&Main>,
    display: Option<(Duration, Option<&str>)>,
    serving: Option<(Step, u64)>,
    drawing: Option<(Step, u64)>,
    modes: Option<(Step, u64)>,
) -> String {
    let thread = |busy: Option<(Step, u64)>| match busy {
        None => "waiting".to_owned(),
        Some((step, ms)) => format!("in {} for {:.1} s", step.name(), ms as f64 / 1000.0),
    };
    let mut line = format!("serving thread {}, drawing thread {}", thread(serving), thread(drawing));
    if let Some(step) = guesthouse_pvg_sys::draw_step() {
        line.push_str(&format!(" (at {step})"));
    }
    if let Some((answered_ago, inside)) = display {
        line.push_str(&format!(
            ", the display's queue last answered {:.1} s ago, in {}",
            answered_ago.as_secs_f64(),
            inside.unwrap_or("none of ours")
        ));
    }
    if let Some((_, ms)) = modes {
        line.push_str(&format!(
            ", {} mode lists not taken by the framework, the oldest for {:.1} s",
            MODES_PENDING.load(Ordering::Relaxed),
            ms as f64 / 1000.0
        ));
    }
    if let Some(main) = main {
        line = format!(
            "window's thread last answered {:.1} s ago, in {}, last left {}, run loop mode {}; {line}",
            main.answered_ago.as_secs_f64(),
            main.inside,
            main.left,
            main.mode
        );
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_step_has_its_name() {
        for number in 0..NAMES.len() as u8 {
            assert_eq!(Step::from(number) as u8, number);
        }
        assert_eq!(Step::from(200), Step::Waiting);
        let mark = Mark::new();
        assert!(mark.busy().is_none());
        mark.set(Step::Read);
        assert_eq!(mark.busy().map(|(step, _)| step), Some(Step::Read));
        mark.set(Step::Waiting);
        assert!(mark.busy().is_none());
    }
}
