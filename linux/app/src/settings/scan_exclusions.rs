//! Folders Defender leaves out of its scanning in this machine, said plainly as the security fact it
//! is, with a button that has Defender scan them again. A machine that leaves none out has no row.

use super::Shared;
use crate::rows::row_button;
use adw::prelude::*;
use guesthouse_app_core::actions::SCAN_ALL_TASK;
use guesthouse_app_core::scan_exclusions::state_text;
use guesthouse_app_core::tasks::{State, Status};

const NOT_RUNNING: &str = "The machine has to be running: Defender's setting is in the guest.";

pub struct ScanExclusions {
    row: adw::ActionRow,
    again: gtk::Button,
    shared: Shared,
}

impl ScanExclusions {
    /// `None` for a machine that leaves no folder out.
    /// `None` for a draft too: New Machine chooses it.
    pub fn new(shared: &Shared, group: &adw::PreferencesGroup) -> Option<Self> {
        if shared.is_draft() {
            return None;
        }
        let text = state_text(&shared.window.context(), &shared.machine.borrow())?;
        let row = adw::ActionRow::builder().title("Virus Scanning").subtitle(text).subtitle_lines(0).build();
        let again = row_button("Scan Them Again");
        again.set_sensitive(false);
        row.add_suffix(&again);
        group.add(&row);
        let running = shared.clone();
        again.connect_clicked(move |_| {
            let machine = running.machine.borrow().clone();
            running.window.launch(SCAN_ALL_TASK, &machine, &[], None);
        });
        Some(Self { row, again, shared: shared.clone() })
    }

    pub fn set_status(&self, status: &Status) {
        let text = state_text(&self.shared.window.context(), &self.shared.machine.borrow());
        self.row.set_visible(text.is_some());
        if let Some(text) = text {
            self.row.set_subtitle(&text);
        }
        let usable = status.state == State::Running && !status.busy;
        self.again.set_sensitive(usable);
        self.again.set_tooltip_text((!usable).then_some(NOT_RUNNING));
    }
}
