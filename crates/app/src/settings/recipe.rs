//! The recipe a machine was made for: where it came from, how its steps went, what it runs, and a
//! button to run the steps again. A machine made with none has no group here.

use super::Shared;
use crate::recipe_dialog;
use crate::rows::row_button;
use adw::prelude::*;
use guesthouse_app_core::actions::RECIPE_TASK;
use guesthouse_app_core::recipe::{read_chosen, summary, what_it_will_run};
use guesthouse_app_core::tasks::{State, Status};

const NOT_RUNNING: &str = "The machine has to be running: the steps run in the guest.";

pub struct Recipe {
    row: adw::ActionRow,
    again: gtk::Button,
    shown: std::cell::RefCell<String>,
    shared: Shared,
}

impl Recipe {
    /// `None` for a machine with no recipe.
    pub fn new(shared: &Shared, page: &adw::PreferencesPage) -> Option<Self> {
        let context = shared.window.context();
        let machine = shared.machine.borrow().clone();
        let chosen = read_chosen(&context, &machine)?;
        let group = adw::PreferencesGroup::builder().title("Recipe").build();
        page.add(&group);
        let row = adw::ActionRow::builder()
            .title(chosen.recipe.name.as_str())
            .subtitle(summary(&context, &machine).unwrap_or_default())
            .subtitle_lines(0)
            .use_markup(false)
            .build();
        let show = row_button("Show");
        let again = row_button("Run Again");
        again.set_sensitive(false);
        row.add_suffix(&show);
        row.add_suffix(&again);
        group.add(&row);
        let looking = shared.clone();
        show.connect_clicked(move |_| {
            let context = looking.window.context();
            let machine = looking.machine.borrow().clone();
            if let Some(chosen) = read_chosen(&context, &machine) {
                let text = what_it_will_run(&chosen.recipe, chosen.source, &chosen.file);
                recipe_dialog::show(&looking.dialog, &chosen.recipe.name, &text);
            }
        });
        let running = shared.clone();
        again.connect_clicked(move |_| {
            let machine = running.machine.borrow().clone();
            running.window.launch(RECIPE_TASK, &machine, &[], None);
        });
        Some(Self { row, again, shown: Default::default(), shared: shared.clone() })
    }

    pub fn set_status(&self, status: &Status) {
        let usable = status.state == State::Running && !status.busy;
        self.again.set_sensitive(usable);
        self.again.set_tooltip_text((!usable).then_some(NOT_RUNNING));
        let context = self.shared.window.context();
        let text = summary(&context, &self.shared.machine.borrow()).unwrap_or_default();
        if *self.shown.borrow() != text {
            self.row.set_subtitle(&text);
            *self.shown.borrow_mut() = text;
        }
    }
}
