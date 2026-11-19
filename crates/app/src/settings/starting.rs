//! Whether the machine starts by itself when a login starts the app.

use super::Shared;
use adw::prelude::*;

const NOTE: &str = "Starts this machine when Guesthouse starts at login. That is turned on in Preferences.";

pub struct Starting;

impl Starting {
    pub fn new(shared: &Shared, page: &adw::PreferencesPage) -> Self {
        let group = adw::PreferencesGroup::builder().title("Starting").build();
        page.add(&group);
        let row = adw::SwitchRow::builder()
            .title("Start with this PC")
            .subtitle(NOTE)
            .active(shared.machine.borrow().start_with_pc)
            .build();
        let saving = shared.clone();
        row.connect_active_notify(move |row| {
            saving.machine.borrow_mut().start_with_pc = row.is_active();
            saving.save();
        });
        group.add(&row);
        Self
    }
}
