//! The forwarded GPU for a plain machine that holds a Linux the user installed. It's a preview, and
//! the guest needs setting up once from the driver image the app then attaches.

use super::gpu_choice::add_gpu_choice;
use super::{Shared, is_off};
use crate::rows::{explain, row_button};
use adw::prelude::*;
use guesthouse_app_core::machine::Kind;
use guesthouse_app_core::tasks::Status;

const SHORT: &str = "A preview, for programs";
const NOTE: &str = "For a Linux you installed here yourself, on Fedora, Debian, Ubuntu, Arch or openSUSE. The VM \
                    gets the driver image and runs in the GPU's jail. The desktop stays on this screen, and the GPU \
                    is for programs. A preview: so far it has only run on my PC.";
const SETUP_TITLE: &str = "Set Up the Guest";
const SETUP_SHORT: &str = "Once, as root in the guest";
const SETUP_BODY: &str = "Run these once as root in the guest, then reboot it. nvfwd-check says what works.";
const SETUP: &str = "mount -o ro LABEL=NVFWD_DRIVER /mnt\nbash /mnt/setup/setup-own.sh";
const VM_MUST_BE_OFF: &str = "The VM must be off to change this.";

pub struct LinuxGpu {
    group: Option<adw::PreferencesGroup>,
    switch: Option<adw::SwitchRow>,
    card: Option<adw::ComboRow>,
}

impl LinuxGpu {
    pub fn new(shared: &Shared, group: &adw::PreferencesGroup) -> Self {
        let machine = shared.machine.borrow().clone();
        // A plain machine that holds macOS or Android has a GPU of its own kind.
        if machine.kind != Kind::Plain || machine.macos() || machine.android() {
            return Self { group: None, switch: None, card: None };
        }
        let switch = adw::SwitchRow::builder().title("GPU").subtitle(SHORT).active(machine.linux_gpu).build();
        explain(&switch, NOTE);
        let saving = shared.clone();
        switch.connect_active_notify(move |row| {
            saving.machine.borrow_mut().linux_gpu = row.is_active();
            saving.save();
        });
        group.add(&switch);
        let card = add_gpu_choice(shared, group);
        let setup = adw::ActionRow::builder().title(SETUP_TITLE).subtitle(SETUP_SHORT).build();
        let show = row_button("Show");
        let dialog = shared.dialog.clone();
        show.connect_clicked(move |_| show_setup(&dialog));
        setup.add_suffix(&show);
        group.add(&setup);
        Self { group: Some(group.clone()), switch: Some(switch), card }
    }

    pub fn set_status(&self, status: &Status) {
        let (Some(group), Some(switch)) = (&self.group, &self.switch) else { return };
        let off = is_off(status);
        switch.set_sensitive(off);
        if let Some(card) = &self.card {
            card.set_sensitive(off);
        }
        group.set_description((!off).then_some(VM_MUST_BE_OFF));
    }
}

/// The commands, where they can be copied.
fn show_setup(parent: &adw::Dialog) {
    let dialog = adw::AlertDialog::new(Some(SETUP_TITLE), Some(SETUP_BODY));
    dialog.set_extra_child(Some(&super::command_card(SETUP)));
    dialog.add_response("close", "Close");
    dialog.present(Some(parent));
}
