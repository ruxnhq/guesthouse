// SPDX-License-Identifier: GPL-2.0-or-later
//! The rules a machine's page and menus follow, judged from facts handed in: what a state allows,
//! whether the Start and Open Screen buttons work, whether an install is shown, and the order the
//! start checks run in. The app only draws what comes back.
//!
//! The shape is the same as `start_checks`: JSON in, JSON out. States are the shared names
//! (`guesthouse-state-core`), so another host's page can ask the same questions.

use guesthouse_state_core::common::State;
use serde::Deserialize;
use serde_json::json;

/// The start checks, in the order they run: memory, then disk, then the graphics driver. The first
/// that finds a problem is the one shown.
pub const START_CHECK_ORDER: [&str; 3] = ["memory", "disk", "driver"];

/// What a state offers next, if anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Start,
    OpenScreen,
    ForceOff,
}

impl Step {
    fn name(self) -> &'static str {
        match self {
            Step::Start => "start",
            Step::OpenScreen => "open_screen",
            Step::ForceOff => "force_off",
        }
    }
}

/// Whether a process for the machine is, or may be, there.
pub fn is_alive(state: State) -> bool {
    matches!(state, State::Starting | State::Running | State::Stopping | State::StoppingStalled)
}

/// The action that makes sense next: open the screen of a running machine, turn off a shutdown
/// that has stalled, start one that is off or failed.
pub fn next_step(state: State) -> Option<Step> {
    match state {
        State::Running => Some(Step::OpenScreen),
        State::StoppingStalled => Some(Step::ForceOff),
        State::Off | State::Failed => Some(Step::Start),
        _ => None,
    }
}

/// Force Off is for a machine that is coming up or running, and for a shutdown that has stalled.
pub fn can_force_off(state: State) -> bool {
    matches!(state, State::Starting | State::Running | State::StoppingStalled)
}

/// A machine the app runs shows its screen while it is up; one run by hand shows it whenever it
/// has one.
pub fn can_open_screen(has_screen: bool, startable: bool, state: State) -> bool {
    has_screen && (!startable || is_alive(state))
}

/// Start works for a machine the app can start, with nothing in the way, while the agent is there
/// and the machine is off or failed.
pub fn can_start(startable: bool, problem: bool, agent_connected: bool, state: State) -> bool {
    startable && !problem && agent_connected && next_step(state) == Some(Step::Start)
}

/// Whether a machine's page is the install's: while it runs or has failed, and once when it is
/// done, until the person has seen it.
pub fn shows_install(install_state: &str, seen: bool) -> bool {
    match install_state {
        "running" | "failed" => true,
        "done" => !seen,
        _ => false,
    }
}

#[derive(Deserialize)]
#[serde(tag = "rule", rename_all = "snake_case")]
enum Rule {
    NextStep { state: State },
    IsAlive { state: State },
    CanForceOff { state: State },
    CanOpenScreen { has_screen: bool, startable: bool, state: State },
    CanStart { startable: bool, problem: bool, agent_connected: bool, state: State },
    ShowsInstall { install_state: String, seen: bool },
    StartCheckOrder,
}

/// Answers one rule: `{"step": ...}`, `{"yes": bool}`, `{"order": [...]}` or `{"error": "..."}`.
pub fn answer(request: &str) -> String {
    let value = match serde_json::from_str::<Rule>(request) {
        Ok(rule) => match rule {
            Rule::NextStep { state } => json!({"step": next_step(state).map(Step::name)}),
            Rule::IsAlive { state } => json!({"yes": is_alive(state)}),
            Rule::CanForceOff { state } => json!({"yes": can_force_off(state)}),
            Rule::CanOpenScreen { has_screen, startable, state } => {
                json!({"yes": can_open_screen(has_screen, startable, state)})
            }
            Rule::CanStart { startable, problem, agent_connected, state } => {
                json!({"yes": can_start(startable, problem, agent_connected, state)})
            }
            Rule::ShowsInstall { install_state, seen } => json!({"yes": shows_install(&install_state, seen)}),
            Rule::StartCheckOrder => json!({"order": START_CHECK_ORDER}),
        },
        Err(error) => json!({"error": error.to_string()}),
    };
    value.to_string()
}

#[cfg(test)]
mod tests;
