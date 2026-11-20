//! Which card a Windows machine's GPU-PV runs on. Windows in it was installed for that card's maker,
//! so the row offers only cards of the same maker, and only while the machine is off. It's there on a
//! PC with more than one card to run on, or when the machine's own card is gone.

use super::{Shared, is_off};
use crate::rows::choice_row;
use adw::prelude::*;
use guesthouse_app_core::card_choice::{
    learn_machines_card, machines_choice, same_maker_choices, windows_card_choices,
};
use guesthouse_app_core::machine::Kind;
use guesthouse_app_core::settings::max_vram_gib_for;
use guesthouse_app_core::tasks::Status;

const TITLE: &str = "Card";
const NOTE: &str = "Windows here was installed for this card's maker. It can move to another card of the same maker \
                    while it's off. For another maker's card, make a new machine.";

pub struct WindowsCard {
    row: Option<adw::ComboRow>,
}

impl WindowsCard {
    /// `cap` is the video memory cap's row, as another card can allow another cap.
    pub fn new(shared: &Shared, page: &adw::PreferencesPage, cap: adw::SpinRow) -> Self {
        let machine = shared.machine.borrow().clone();
        let context = shared.window.context();
        if machine.kind != Kind::Windows {
            return Self { row: None };
        }
        let choices = same_maker_choices(&context, &machine);
        let offered = windows_card_choices(&context);
        let own_card_here = offered.iter().any(|choice| choice.is_machines(&machine));
        if offered.len() < 2 && own_card_here {
            return Self { row: None };
        }
        let group = adw::PreferencesGroup::builder().title("GPU").build();
        page.add(&group);
        let labels: Vec<&str> = choices.iter().map(|choice| choice.label.as_str()).collect();
        let row = choice_row(TITLE, &labels);
        row.set_subtitle(NOTE);
        row.set_selected(machines_choice(&choices, &machine) as u32);
        let saving = shared.clone();
        row.connect_selected_notify(move |row| {
            let Some(choice) = choices.get(row.selected() as usize) else { return };
            if choice.is_machines(&saving.machine.borrow()) {
                return;
            }
            let most = {
                let mut machine = saving.machine.borrow_mut();
                machine.use_windows_card(choice.maker, &choice.pci);
                let context = saving.window.context();
                learn_machines_card(&context, &machine);
                let most = max_vram_gib_for(&context, &machine);
                machine.vram_gib = machine.vram_gib.min(most);
                most
            };
            // A lower top moves the row's value down with it, which the row saves.
            cap.set_range(0.0, most.into());
            saving.save();
        });
        group.add(&row);
        Self { row: Some(row) }
    }

    pub fn set_status(&self, status: &Status) {
        if let Some(row) = &self.row {
            row.set_sensitive(is_off(status));
        }
    }
}
