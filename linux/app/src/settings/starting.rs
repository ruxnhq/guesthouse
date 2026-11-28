//! Whether the machine starts by itself when a login starts the app.

use super::Shared;
use crate::rows::explain;
use adw::prelude::*;

const NOTE: &str = "Starts this machine when Guesthouse starts at login. That is turned on in Preferences.";

pub struct Starting;

impl Starting {
    pub fn new(shared: &Shared, group: &adw::PreferencesGroup) -> Self {
        let row = adw::SwitchRow::builder()
            .title("Start With This PC")
            .subtitle("When Guesthouse starts at login")
            .active(shared.machine.borrow().start_with_pc)
            .build();
        explain(&row, NOTE);
        let saving = shared.clone();
        row.connect_active_notify(move |row| {
            saving.machine.borrow_mut().start_with_pc = row.is_active();
            saving.save();
        });
        group.add(&row);
        Self
    }
}
