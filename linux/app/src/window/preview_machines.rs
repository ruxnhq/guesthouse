//! The preview's made-up machines in the window: each shows the status it's held at, whatever a
//! look at the machines finds. See [`crate::preview`].

use super::Window;
use gtk::glib;

impl Window {
    pub(super) fn show_preview(&self, asked: &str) {
        for (machine, status) in crate::preview::machines(&self.0.context) {
            self.0.held.borrow_mut().insert(machine.id.clone(), status.clone());
            let id = machine.id.clone();
            self.add_machine(machine);
            if let Some(card) = self.card(&id) {
                card.set_status(&status, false);
            }
        }
        if asked == crate::preview::SETTINGS {
            let this = self.clone();
            glib::idle_add_local_once(move || {
                let off = this.0.cards.borrow().values().find(|card| card.shown().is_off()).cloned();
                if let Some(card) = off {
                    card.show_settings();
                }
            });
        }
    }
}
