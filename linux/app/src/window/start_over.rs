//! The window's part when GTK loses the GPU: what to hand the new process, and what it opens again.
//! See [`crate::gpu_reset`].

use super::Window;
use super::machine_card::MachineCard;
use crate::gpu_reset::{self, Restart};
use adw::prelude::*;
use gtk::glib;
use std::time::{Duration, Instant};

/// How often the new window looks for the machine's screen, and for how long. The cards learn
/// where each machine is within a second of opening.
const LOOK_EVERY: Duration = Duration::from_millis(250);
const LOOK_FOR: Duration = Duration::from_secs(10);

impl Window {
    /// Starts the app over, for the new process to open the screen open now. `earlier` is the note
    /// this process was started with.
    pub fn start_over(&self, earlier: Option<&Restart>) {
        let note = Restart {
            at: gpu_reset::now(),
            screen: self.open_screen().map(|page| page.machine_id()),
            fullscreen: self.0.window.is_fullscreen(),
        };
        gpu_reset::start_over(&self.0.context, &note, earlier);
    }

    /// Says why the app started over, and opens the screen it had open once the machine has one.
    pub fn reopen(&self, note: &Restart) {
        self.toast("The graphics card was reset, so Guesthouse started over", adw::ToastPriority::High);
        let Some(id) = note.screen.clone() else { return };
        let fullscreen = note.fullscreen;
        let this = self.clone();
        let started = Instant::now();
        glib::timeout_add_local(LOOK_EVERY, move || {
            let card = this.0.cards.borrow().get(&id).cloned();
            if let Some(card) = card.filter(MachineCard::has_screen) {
                card.show_screen();
                if fullscreen
                    && !this.0.window.is_fullscreen()
                    && let Some(page) = this.open_screen()
                {
                    page.toggle_fullscreen();
                }
                return glib::ControlFlow::Break;
            }
            if started.elapsed() < LOOK_FOR { glib::ControlFlow::Continue } else { glib::ControlFlow::Break }
        });
    }
}
