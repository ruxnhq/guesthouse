//! Lean Windows on a machine made with it: that it was, what it took out, and a button to do it again
//! in the running guest. A machine made without it has no group here.

use super::Shared;
use crate::recipe_dialog;
use crate::rows::row_button;
use adw::prelude::*;
use guesthouse_app_core::actions::LEAN_TASK;
use guesthouse_app_core::lean::{is_lean, what_it_does};
use guesthouse_app_core::tasks::{State, Status};

const TITLE: &str = "Lean Windows";
const NOTE: &str =
    "Made without the apps Windows adds for every account. Run Again takes out any that came back.";
const NOT_RUNNING: &str = "The machine has to be running: this runs in the guest.";

pub struct Lean {
    again: gtk::Button,
}

impl Lean {
    /// `None` for a machine made without it.
    pub fn new(shared: &Shared, page: &adw::PreferencesPage) -> Option<Self> {
        let context = shared.window.context();
        if !is_lean(&context, &shared.machine.borrow()) {
            return None;
        }
        let group = adw::PreferencesGroup::builder().build();
        page.add(&group);
        let row = adw::ActionRow::builder().title(TITLE).subtitle(NOTE).subtitle_lines(0).build();
        let show = row_button("Show");
        let again = row_button("Run Again");
        again.set_sensitive(false);
        row.add_suffix(&show);
        row.add_suffix(&again);
        group.add(&row);
        let dialog = shared.dialog.clone();
        show.connect_clicked(move |_| recipe_dialog::show(&dialog, TITLE, &what_it_does()));
        let running = shared.clone();
        again.connect_clicked(move |_| {
            let machine = running.machine.borrow().clone();
            running.window.launch(LEAN_TASK, &machine, &[], None);
        });
        Some(Self { again })
    }

    pub fn set_status(&self, status: &Status) {
        let usable = status.state == State::Running && !status.busy;
        self.again.set_sensitive(usable);
        self.again.set_tooltip_text((!usable).then_some(NOT_RUNNING));
    }
}
