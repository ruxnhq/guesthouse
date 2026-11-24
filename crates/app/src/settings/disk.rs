//! The disk's size, on the Overview, and what is in the drive, on the Storage page. A draft has neither
//! yet: New Machine sets its size and its disc.

use super::{Shared, is_off};
use crate::common::in_background;
use crate::rows::{boxed_rows, number_row, on_yes, question, row_button};
use adw::prelude::*;
use guesthouse_app_core::disk_image::disk_size_gib;
use guesthouse_app_core::library::{MAX_DISK_GIB, eject};
use guesthouse_app_core::machine::Kind;
use guesthouse_app_core::tasks::Status;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

struct Parts {
    shared: Shared,
    disk: adw::ActionRow,
    grow: gtk::Button,
    drive: adw::ActionRow,
    eject: gtk::Button,
    size: Cell<Option<f64>>,
    busy: Cell<bool>,
    status: RefCell<Status>,
}

#[derive(Clone)]
pub struct Disk(Rc<Parts>);

impl Disk {
    /// The size goes in `figures`, and the drive in a group of its own on `storage`.
    pub fn new(shared: &Shared, figures: &adw::PreferencesGroup, storage: &adw::PreferencesPage) -> Self {
        let draft = shared.is_draft();
        let android = shared.machine.borrow().android();
        // An Android machine's size is its data disk's, where its apps go.
        let title = if android { "Data Disk" } else { "Disk" };
        let disk =
            adw::ActionRow::builder().title(title).subtitle("Reading the disk").visible(!draft).build();
        let grow = row_button("Grow");
        grow.set_sensitive(false);
        disk.add_suffix(&grow);
        figures.add(&disk);
        // An Android machine has no drive for a disc: it boots only from its own disk.
        let plain = shared.machine.borrow().kind == Kind::Plain && !android && !draft;
        let macos = shared.machine.borrow().macos();
        let title = if macos { "Installer" } else { "ISO" };
        let group = adw::PreferencesGroup::builder().title("Disc").visible(plain).build();
        storage.add(&group);
        let drive = adw::ActionRow::builder().title(title).subtitle_lines(1).visible(plain).build();
        let eject = row_button(if macos { "Remove" } else { "Eject" });
        drive.add_suffix(&eject);
        group.add(&drive);
        let this = Self(Rc::new(Parts {
            shared: shared.clone(),
            disk,
            grow,
            drive,
            eject,
            size: Cell::new(None),
            busy: Cell::new(false),
            status: RefCell::default(),
        }));
        let handle = this.clone();
        this.0.grow.connect_clicked(move |_| handle.on_grow());
        let handle = this.clone();
        this.0.eject.connect_clicked(move |_| handle.on_eject());
        this.show_drive();
        this.read_disk();
        this
    }

    fn show_drive(&self) {
        let iso = self.0.shared.machine.borrow().iso.clone();
        let macos = self.0.shared.machine.borrow().macos();
        let text = match (iso.is_empty(), macos) {
            (true, true) => "Removed, so the Mac boots from its disk",
            (true, false) => "Empty, so the machine boots from its disk",
            (false, true) => "Apple's installer, to remove once macOS is installed",
            (false, false) => iso.as_str(),
        };
        self.0.drive.set_subtitle(&gtk::glib::markup_escape_text(text));
        self.0.eject.set_sensitive(!iso.is_empty());
    }

    fn on_eject(&self) {
        let shared = &self.0.shared;
        shared.change(eject);
        self.show_drive();
    }

    fn read_disk(&self) {
        if self.0.shared.is_draft() {
            return;
        }
        let context = self.0.shared.window.context();
        // An Android machine's size is its data disk's, where its apps go.
        let disk = {
            let machine = self.0.shared.machine.borrow();
            if machine.android() { machine.android_data.clone() } else { machine.disk.clone() }
        };
        let this = self.clone();
        in_background(
            &context,
            move || disk_size_gib(&disk),
            move |result| match result {
                Ok(size) => {
                    this.0.size.set(Some(size));
                    this.show_size();
                }
                Err(text) => this.0.disk.set_subtitle(&gtk::glib::markup_escape_text(&text)),
            },
        );
    }

    fn show_size(&self) {
        let off = is_off(&self.0.status.borrow());
        // Android's data partition is sized when Android formats it, and isn't grown after.
        self.0.grow.set_visible(!self.0.shared.machine.borrow().android());
        self.0.grow.set_sensitive(off && self.0.size.get().is_some());
        self.0.grow.set_tooltip_text((!off).then_some("It can only grow while the VM is off."));
        if let Some(size) = self.0.size.get() {
            let machine = self.0.shared.machine.borrow();
            let place = if machine.disk_folder.is_empty() {
                String::new()
            } else {
                format!(" in {}", gtk::glib::markup_escape_text(&machine.disk_folder))
            };
            self.0.disk.set_subtitle(&format!("{size} GiB{place}"));
        }
    }

    fn on_grow(&self) {
        let Some(current) = self.0.size.get() else { return };
        let smallest = current.trunc() + 1.0;
        let size = number_row("New Size", smallest, smallest, MAX_DISK_GIB.into());
        size.set_subtitle("In GiB");
        let dialog = question(
            "Grow the Disk?",
            "The guest will not use the new space until you extend its volume, which on Windows is in Disk \
             Management. The disk cannot shrink again.",
            "Grow",
            adw::ResponseAppearance::Destructive,
        );
        dialog.set_extra_child(Some(&boxed_rows(&[size.upcast_ref()])));
        let shared = self.0.shared.clone();
        on_yes(&dialog, move || {
            // A size typed and not yet entered is the one meant.
            size.update();
            let gib = (size.value() as u32).to_string();
            shared.window.launch("grow", &shared.machine.borrow(), &[gib], None);
        });
        dialog.present(Some(&self.0.shared.dialog));
    }

    pub fn set_status(&self, status: &Status) {
        *self.0.status.borrow_mut() = status.clone();
        self.show_drive();
        self.show_size();
        if self.0.busy.replace(status.busy) != status.busy {
            self.read_disk();
        }
    }
}
