//! The application: one window, which no VM depends on.

use crate::gpu_reset::{self, Restart};
use crate::window::Window;
use adw::prelude::*;
use gtk::glib;
use guesthouse_app_core::context::Context;
use guesthouse_app_core::log::log_panics;
use std::cell::RefCell;
use std::fs;
use std::io;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

pub const APP_ID: &str = "io.github.xnixy.Guesthouse";
/// Built in, so the app has its icon however it was started. GTK only reads icons from a folder.
const ICONS: [(&str, &[u8]); 2] = [
    (
        "hicolor/scalable/apps/io.github.xnixy.Guesthouse.svg",
        include_bytes!("../data/icons/hicolor/scalable/apps/io.github.xnixy.Guesthouse.svg"),
    ),
    (
        "hicolor/symbolic/apps/io.github.xnixy.Guesthouse-symbolic.svg",
        include_bytes!("../data/icons/hicolor/symbolic/apps/io.github.xnixy.Guesthouse-symbolic.svg"),
    ),
];

fn write_icons(folder: &Path) -> io::Result<()> {
    for (name, data) in ICONS {
        let path = folder.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, data)?;
    }
    Ok(())
}

/// Builds the application around this context, and calls `started` with its window. `restart` is the
/// note of a process that lost the GPU and started this one over.
pub fn application(
    context: Arc<Context>,
    restart: Option<Restart>,
    started: impl Fn(&Window) + 'static,
) -> adw::Application {
    let application = adw::Application::builder().application_id(APP_ID).build();
    let icons = context.paths.runtime.join("icons");
    let window: Rc<RefCell<Option<Window>>> = Rc::default();
    application.connect_activate(move |application| {
        let mut slot = window.borrow_mut();
        let window = slot.get_or_insert_with(|| {
            let window = Window::new(application, context.clone());
            // A run that a login started stays out of the way when there's an icon to call it by.
            if !(crate::login::started_at_login() && window.has_tray()) {
                window.present();
            }
            window.note_screen_hdr();
            window.offer_setup();
            window.say_if_there_is_an_update();
            if let Some(note) = &restart {
                window.reopen(note);
            }
            let lost = window.clone();
            let earlier = restart.clone();
            gpu_reset::when_lost(move || lost.start_over(earlier.as_ref()));
            started(&window);
            window
        });
        // Started again while it runs: that's someone asking for the window.
        window.present();
    });
    application.connect_startup(move |_| use_built_in_icons(&icons));
    application
}

/// So the icon shows before anything has installed it: written to `icons`, and looked for there.
pub fn use_built_in_icons(icons: &Path) {
    if write_icons(icons).is_ok()
        && let Some(display) = gtk::gdk::Display::default()
    {
        gtk::IconTheme::for_display(&display).add_search_path(icons);
    }
    gtk::Window::set_default_icon_name(APP_ID);
}

/// `restart` is the note of a process that lost the GPU and started this one over.
pub fn run(restart: Option<Restart>) -> glib::ExitCode {
    let context = match Context::from_process() {
        Ok(context) => Arc::new(context),
        Err(error) => {
            eprintln!("{error}");
            return glib::ExitCode::FAILURE;
        }
    };
    log_panics(&context);
    gpu_reset::watch();
    if restart.is_some() {
        gpu_reset::wait_for_the_name();
    }
    let at_login = crate::login::started_at_login();
    let application = application(context, restart, move |window| {
        if at_login {
            window.start_machines_for_login();
        }
    });
    // GTK would refuse the autostart file's own argument, and the app takes no others.
    application.run_with_args::<&str>(&[])
}
