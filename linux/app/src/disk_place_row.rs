//! Where a new machine's disk goes: with the machine, or in a folder on another drive.

use crate::common::choose_folder;
use crate::rows::{icon_button, row_button};
use crate::window::Window;
use adw::prelude::*;
use guesthouse_app_core::host::free_gib;
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

const TITLE: &str = "Disk Location";
const WITH_THE_MACHINE: &str = "With the machine";

struct Parts {
    window: Window,
    row: adw::ActionRow,
    back: gtk::Button,
    /// The folder the user chose, or empty for the machine's own.
    chosen: RefCell<String>,
}

#[derive(Clone)]
pub struct DiskPlaceRow(Rc<Parts>);

/// The folder and the room its drive has left, which is what the disk grows into.
fn place_text(folder: &Path, chosen: bool) -> String {
    let place = if chosen { folder.display().to_string() } else { WITH_THE_MACHINE.to_owned() };
    // A store with no machine yet has no folder for them either, and its drive is the same.
    let there = folder.ancestors().find(|place| place.exists()).unwrap_or(folder);
    match free_gib(there) {
        Ok(room) => format!("{place}, {room:.0} GiB free"),
        Err(_) => place,
    }
}

impl DiskPlaceRow {
    pub fn new(window: &Window, group: &adw::PreferencesGroup) -> Self {
        let row = adw::ActionRow::builder().title(TITLE).subtitle_lines(2).use_markup(false).build();
        let back = icon_button("edit-undo-symbolic", "Keep the Disk With the Machine");
        back.set_visible(false);
        let choose = row_button("Choose");
        row.add_suffix(&back);
        row.add_suffix(&choose);
        group.add(&row);
        let this = Self(Rc::new(Parts { window: window.clone(), row, back, chosen: RefCell::default() }));
        let handle = this.clone();
        choose.connect_clicked(move |_| handle.on_choose());
        let handle = this.clone();
        this.0.back.connect_clicked(move |_| handle.set(String::new()));
        this.show();
        this
    }

    /// The folder the user chose, or empty for the machine's own.
    pub fn chosen(&self) -> String {
        self.0.chosen.borrow().clone()
    }

    fn on_choose(&self) {
        let window = self.0.window.clone();
        let failing = window.clone();
        let this = self.clone();
        choose_folder(
            window.widget(),
            "Choose a Folder for the Disk",
            move |path| this.set(path),
            move |text| failing.problem(&text),
        );
    }

    fn set(&self, folder: String) {
        *self.0.chosen.borrow_mut() = folder;
        self.show();
    }

    fn show(&self) {
        let chosen = self.0.chosen.borrow();
        let text = if chosen.is_empty() {
            place_text(&self.0.window.context().paths.machines, false)
        } else {
            place_text(Path::new(chosen.as_str()), true)
        };
        self.0.row.set_subtitle(&text);
        self.0.back.set_visible(!chosen.is_empty());
    }
}
