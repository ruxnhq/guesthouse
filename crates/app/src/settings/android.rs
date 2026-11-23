//! An Android machine's GPU. Android's Mesa draws through QEMU's virtio GPU, which runs on one of this
//! PC's cards with that card's own drivers, so any maker's card will do. The card is the only choice.

use super::{Shared, is_off};
use crate::rows::choice_row;
use adw::prelude::*;
use guesthouse_app_core::gpu_maker::GpuMaker;
use guesthouse_app_core::render_cards::render_cards;
use guesthouse_app_core::tasks::Status;

const NOTE: &str = "Android draws through QEMU's virtio GPU on this PC's card: OpenGL ES through virgl and \
                    Vulkan through Venus. Its screen and input come through this window. A preview.";
const AUTOMATIC: &str = "Any, NVIDIA's first";
const CARD_NOTE: &str = "The card Android draws on. An Intel card has not been tried yet.";
const VM_MUST_BE_OFF: &str = "The VM must be off to change this.";

pub struct Android {
    group: Option<adw::PreferencesGroup>,
    card: Option<adw::ComboRow>,
}

/// Every card with a render node, by address, with what to call it.
fn choices(shared: &Shared, chosen: &str) -> Vec<(String, String)> {
    let context = shared.window.context();
    let mut choices = vec![(String::new(), AUTOMATIC.to_owned())];
    for maker in GpuMaker::ALL {
        let cards =
            render_cards(&context, maker).into_iter().filter(|card| !card.render_node.as_os_str().is_empty());
        choices.extend(cards.map(|card| (card.pci.clone(), format!("{} at {}", card.name, card.pci))));
    }
    if !choices.iter().any(|(pci, _)| pci == chosen) {
        choices.push((chosen.to_owned(), format!("The card at {chosen}, which isn't in this PC now")));
    }
    choices
}

impl Android {
    pub fn new(shared: &Shared, page: &adw::PreferencesPage) -> Self {
        if !shared.machine.borrow().android() {
            return Self { group: None, card: None };
        }
        let group = adw::PreferencesGroup::builder().title("Android GPU").build();
        page.add(&group);
        group.add(&adw::ActionRow::builder().title("GPU acceleration").subtitle(NOTE).build());
        let chosen = shared.machine.borrow().android_gpu.clone();
        let choices = choices(shared, &chosen);
        let names: Vec<&str> = choices.iter().map(|(_, name)| name.as_str()).collect();
        let row = choice_row("Card", &names);
        row.set_subtitle(CARD_NOTE);
        row.set_selected(choices.iter().position(|(pci, _)| *pci == chosen).unwrap_or(0) as u32);
        let saving = shared.clone();
        row.connect_selected_notify(move |row| {
            let Some((pci, _)) = choices.get(row.selected() as usize) else { return };
            saving.machine.borrow_mut().android_gpu.clone_from(pci);
            saving.save();
        });
        group.add(&row);
        Self { group: Some(group), card: Some(row) }
    }

    pub fn set_status(&self, status: &Status) {
        let Some(group) = &self.group else { return };
        let off = is_off(status);
        if let Some(card) = &self.card {
            card.set_sensitive(off);
        }
        group.set_description((!off).then_some(VM_MUST_BE_OFF));
    }
}
