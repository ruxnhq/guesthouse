//! What the machine's window says while a device on the network has its screen to itself
//! (remoteman's `only`): the guest's own monitor is off its desktop then, and no frame comes. A
//! label over the absent picture says who has it and that it comes back, instead of a dark window.
//! It is a widget over the picture, never a drawing of frames.

use gtk::prelude::*;
use gtk::{gio, glib};
use guesthouse_app_core::context::Context;
use guesthouse_app_core::machine::Machine;
use guesthouse_app_core::remote_view;
use guesthouse_remote_view_core::wording::only_notice;
use std::sync::Arc;
use std::time::Duration;

const LOOK: Duration = Duration::from_secs(2);

/// Puts the notice over `screen` and keeps it right while the screen is there. A machine with its
/// remote view off is looked at only to see that it still is: nothing asks its guest.
pub fn attach(screen: &gtk::Overlay, context: Arc<Context>, machine: Machine) {
    let label = gtk::Label::builder()
        .wrap(true)
        .justify(gtk::Justification::Center)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .max_width_chars(48)
        .can_target(false)
        .visible(false)
        .build();
    label.add_css_class("performance-overlay");
    label.add_css_class("title-4");
    screen.add_overlay(&label);
    let weak = label.downgrade();
    let asking = Arc::new(std::sync::atomic::AtomicBool::new(false));
    glib::timeout_add_local(LOOK, move || {
        let Some(label) = weak.upgrade() else { return glib::ControlFlow::Break };
        if label.parent().is_none() {
            return glib::ControlFlow::Break;
        }
        if asking.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return glib::ControlFlow::Continue;
        }
        let (context, machine, asking) = (context.clone(), machine.clone(), asking.clone());
        let weak = label.downgrade();
        glib::spawn_future_local(async move {
            let only = gio::spawn_blocking(move || {
                remote_view::allowed(&context, &machine)
                    .then(|| remote_view::info(&context, &machine).viewing_only().map(str::to_owned))
                    .flatten()
            })
            .await
            .ok()
            .flatten();
            asking.store(false, std::sync::atomic::Ordering::SeqCst);
            if let Some(label) = weak.upgrade() {
                if let Some(name) = &only {
                    label.set_label(&only_notice(name));
                }
                label.set_visible(only.is_some());
            }
        });
        glib::ControlFlow::Continue
    });
}
