//! An NVIDIA Windows machine's card holds its memory clock up while a viewer streams the machine's
//! screen to another device. It's on the Advanced page, under DLSS.

use super::Shared;
use crate::rows::explain;
use adw::prelude::*;
use guesthouse_app_core::gpu_maker::GpuMaker;
use guesthouse_app_core::remote_view::stream_clocks_changed;

const TITLE: &str = "Full Memory Clock While Viewed";
const SHORT: &str = "Faster pictures on other devices, at the card's idle power";
const NOTE: &str = "While another device views this machine's screen, the card's memory clock is held at its \
                    highest, so each change on the desktop is sent in a few milliseconds instead of tens. The \
                    card then draws about as much as under load even when idle, 100 W rather than 17 W on an RTX \
                    3080 Ti, until half a minute after the device has gone. Turned off, the card clocks itself. \
                    Takes effect at once.";

pub struct StreamClocks;

impl StreamClocks {
    pub fn new(shared: &Shared, group: &adw::PreferencesGroup) -> Self {
        let machine = shared.machine.borrow().clone();
        if machine.windows_maker() != Some(GpuMaker::Nvidia) {
            return Self;
        }
        let row =
            adw::SwitchRow::builder().title(TITLE).subtitle(SHORT).active(machine.stream_clocks()).build();
        explain(&row, NOTE);
        let saving = shared.clone();
        row.connect_active_notify(move |row| {
            saving.machine.borrow_mut().stream_clocks_off = !row.is_active();
            saving.save();
            if saving.is_draft() {
                return;
            }
            // It asks the machine's remote view over its socket, which may take a moment.
            let (context, machine) = (saving.window.context(), saving.machine.borrow().clone());
            std::thread::spawn(move || stream_clocks_changed(&context, &machine));
        });
        group.add(&row);
        Self
    }
}
