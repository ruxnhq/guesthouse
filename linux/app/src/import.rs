//! Import Machine, from the main menu: a machine from a file exported on another PC.

use crate::common::{choose_file, in_background};
use crate::window::Window;
use guesthouse_app_core::export::EXTENSION;
use guesthouse_app_core::import::import;
use std::path::PathBuf;

/// Asks for the file, and makes the machine on a thread, as a disk takes a while to read. If the app
/// closes first, nothing of it is left, and the next import or duplicate clears its folder.
pub fn ask(window: &Window) {
    let chosen = window.clone();
    let failed = window.clone();
    choose_file(
        window.widget(),
        "Import Machine",
        "Exported machines",
        EXTENSION,
        move |path| make(&chosen, PathBuf::from(path)),
        move |text| failed.problem(&text),
    );
}

fn make(window: &Window, file: PathBuf) {
    window.toast("Importing the machine. It shows up here once it's whole.", adw::ToastPriority::Normal);
    let context = window.context();
    let working = context.clone();
    let window = window.clone();
    in_background(
        &context,
        move || import(&working, &file, None, &mut |_| {}),
        move |made| match made {
            Ok(made) => {
                window.add_machine(made.machine.clone());
                let mut text = format!("{} is imported.", made.machine.name);
                for note in &made.notes {
                    text = format!("{text} {note}");
                }
                window.toast(&text, adw::ToastPriority::High);
            }
            Err(problem) => window.problem(&problem),
        },
    );
}
