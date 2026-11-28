//! The screen page keeping in step with the look and with how the compositor holds the window.

use super::ScreenPage;
use crate::theme::{self, Drawn};

impl ScreenPage {
    /// Draws the bar in the look's form, with the window's own buttons where the window has a bar.
    pub fn follow_look(&self) {
        let parts = &self.0;
        parts.pill.set_tide(theme::drawn() == Drawn::Tide, parts.window.has_bar());
        self.follow_header();
    }
}
