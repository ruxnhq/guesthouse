//! Folders Defender leaves out of its scanning in this machine, said plainly as the security fact it
//! is, with a button that has Defender scan them again. A machine that leaves none out has no group.

use super::Shared;
use crate::rows::row_button;
use adw::prelude::*;
use guesthouse_app_core::actions::SCAN_ALL_TASK;
use guesthouse_app_core::scan_exclusions::read_exclusions;
use guesthouse_app_core::tasks::{State, Status};

const TITLE: &str = "Virus Scanning";
const NOT_RUNNING: &str = "The machine has to be running: Defender's setting is in the guest.";

pub struct ScanExclusions {
    group: adw::PreferencesGroup,
    row: adw::ActionRow,
    again: gtk::Button,
    shared: Shared,
}

fn said(folders: &[String]) -> String {
    format!("Defender doesn't scan {}. Everything else is scanned.", folders.join(", "))
}

impl ScanExclusions {
    /// `None` for a machine that leaves no folder out.
    pub fn new(shared: &Shared, page: &adw::PreferencesPage) -> Option<Self> {
        let folders = read_exclusions(&shared.window.context(), &shared.machine.borrow());
        if folders.is_empty() {
            return None;
        }
        let group = adw::PreferencesGroup::builder().title(TITLE).build();
        page.add(&group);
        let row = adw::ActionRow::builder()
            .title("Folders Left Out")
            .subtitle(said(&folders))
            .subtitle_lines(0)
            .build();
        let again = row_button("Scan Them Again");
        again.set_sensitive(false);
        row.add_suffix(&again);
        group.add(&row);
        let running = shared.clone();
        again.connect_clicked(move |_| {
            let machine = running.machine.borrow().clone();
            running.window.launch(SCAN_ALL_TASK, &machine, &[], None);
        });
        Some(Self { group, row, again, shared: shared.clone() })
    }

    pub fn set_status(&self, status: &Status) {
        let folders = read_exclusions(&self.shared.window.context(), &self.shared.machine.borrow());
        self.group.set_visible(!folders.is_empty());
        if !folders.is_empty() {
            self.row.set_subtitle(&said(&folders));
        }
        let usable = status.state == State::Running && !status.busy;
        self.again.set_sensitive(usable);
        self.again.set_tooltip_text((!usable).then_some(NOT_RUNNING));
    }
}
