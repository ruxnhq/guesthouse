//! How heavily a machine sits in the library: calm when it's off, stirring while it runs, swelling
//! when it needs the user, and spilling when it failed. Each is a CSS class on the machine's card
//! and on the hero's state dot, which a look can move and colour as it likes.
//!
//! The Linux host never reports a machine as failed or stalled: a start that went wrong leaves it
//! off with the reason beside it, and what needs the user comes as a note on a running machine.
//! So the weight is read from the whole status, not from the state alone.

use guesthouse_app_core::tasks::{State, Status};

/// The CSS classes, one for each weight but calm.
const RUNNING: &str = "running";
const ATTENTION: &str = "attention";
const FAILED: &str = "failed";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Weight {
    #[default]
    Calm,
    /// Running, or on its way up or down.
    Stirs,
    /// Something needs the user: a shutdown that hangs, an install cut short, a note on a running
    /// machine about its screen, its token or its jail.
    Swells,
    /// It didn't start, or stopped with an error.
    Spills,
}

impl Weight {
    /// `installing` is the machine's own word that its install hasn't finished.
    pub fn of(status: &Status, installing: bool) -> Self {
        let notes = [&status.error, &status.display_problem, &status.token_note, &status.jail_note];
        let has_note = notes.into_iter().any(|note| !note.is_empty());
        match status.state {
            State::Failed => Self::Spills,
            State::Off if !status.busy && (!status.error.is_empty() || status.restore_point.is_some()) => {
                Self::Spills
            }
            State::Off if installing && !status.busy => Self::Swells,
            State::Off => Self::Calm,
            State::StoppingStalled => Self::Swells,
            State::Running if has_note => Self::Swells,
            State::Starting | State::Installing | State::Running | State::Stopping => Self::Stirs,
        }
    }

    fn class(self) -> Option<&'static str> {
        match self {
            Self::Calm => None,
            Self::Stirs => Some(RUNNING),
            Self::Swells => Some(ATTENTION),
            Self::Spills => Some(FAILED),
        }
    }

    /// Gives the widget this weight's class, and takes the others off.
    pub fn mark(self, widget: &impl gtk::prelude::WidgetExt) {
        for class in [RUNNING, ATTENTION, FAILED] {
            if self.class() == Some(class) {
                widget.add_css_class(class);
            } else {
                widget.remove_css_class(class);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(state: State) -> Status {
        Status { state, ..Status::default() }
    }

    #[test]
    fn off_is_calm_and_running_stirs() {
        assert_eq!(Weight::of(&status(State::Off), false), Weight::Calm);
        assert_eq!(Weight::of(&status(State::Running), false), Weight::Stirs);
        assert_eq!(Weight::of(&status(State::Starting), false), Weight::Stirs);
        assert_eq!(Weight::of(&status(State::Stopping), false), Weight::Stirs);
    }

    #[test]
    fn a_start_that_went_wrong_spills() {
        let failed = Status { error: "QEMU could not start.".into(), ..status(State::Off) };
        assert_eq!(Weight::of(&failed, false), Weight::Spills);
        assert_eq!(Weight::of(&status(State::Failed), false), Weight::Spills);
        // A task still at work isn't done failing.
        let busy = Status { busy: true, ..failed };
        assert_eq!(Weight::of(&busy, false), Weight::Calm);
    }

    #[test]
    fn what_needs_the_user_swells() {
        let noted =
            Status { display_problem: "The screen is smaller than asked.".into(), ..status(State::Running) };
        assert_eq!(Weight::of(&noted, false), Weight::Swells);
        assert_eq!(Weight::of(&status(State::StoppingStalled), false), Weight::Swells);
        assert_eq!(Weight::of(&status(State::Off), true), Weight::Swells);
    }
}
