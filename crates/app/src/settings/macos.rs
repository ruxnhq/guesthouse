//! A macOS machine's GPU. Its own drivers draw through reims-vgpu's device, and the device draws on
//! one of this PC's cards, which is the only thing to choose.

use super::gpu_choice::add_gpu_choice_saying;
use super::{Shared, is_off};
use adw::prelude::*;
use guesthouse_app_core::tasks::Status;

const NOTE: &str = "macOS draws through reims-vgpu on this PC's GPU, and its screen and input come through \
                    this window. A preview: so far it has only run on my PC, on an NVIDIA card.";
const NVIDIA: &str = "The NVIDIA card";
const CARD_NOTE: &str = "The card macOS draws on. An AMD card has not been tried yet.";
const VM_MUST_BE_OFF: &str = "The VM must be off to change this.";

pub struct Macos {
    group: Option<adw::PreferencesGroup>,
    card: Option<adw::ComboRow>,
}

impl Macos {
    pub fn new(shared: &Shared, page: &adw::PreferencesPage) -> Self {
        if !shared.machine.borrow().macos() {
            return Self { group: None, card: None };
        }
        let group = adw::PreferencesGroup::builder().title("macOS GPU").build();
        page.add(&group);
        group.add(&adw::ActionRow::builder().title("GPU acceleration").subtitle(NOTE).build());
        let card = add_gpu_choice_saying(shared, &group, NVIDIA, CARD_NOTE);
        Self { group: Some(group), card }
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
