//! The clipboard, dragged files and the shared folder.

use super::Shared;
use crate::common::choose_folder;
use crate::rows::{explain, info_button, row_button};
use adw::prelude::*;
use guesthouse_app_core::agent::{share_address, share_key};
use guesthouse_app_core::machine::Kind;
use guesthouse_app_core::start_checks::share_problem;
use guesthouse_app_core::tasks::{State, Status};
use std::cell::Cell;
use std::rc::Rc;

struct Parts {
    shared: Shared,
    folder: adw::ActionRow,
    clear: gtk::Button,
    read_only: adw::SwitchRow,
    off: Cell<bool>,
}

#[derive(Clone)]
pub struct Sharing(Rc<Parts>);

/// Where the guest finds the shared folder. Its address holds the key the folder is served under,
/// which a machine not made yet doesn't have.
fn place(machine: &guesthouse_app_core::machine::Machine, address: Option<&str>) -> String {
    if machine.kind == Kind::Windows {
        return "as a drive, from its next start, for files up to 4 GiB".to_owned();
    }
    let Some(address) = address else {
        return "at an address of its own, shown here once the machine is made".to_owned();
    };
    if machine.macos() {
        format!("in Finder, with Go, Connect to Server and {address}")
    } else {
        format!("at {address}, through a WebDAV client")
    }
}

impl Sharing {
    pub fn new(shared: &Shared, page: &adw::PreferencesPage) -> Self {
        let machine = shared.machine.borrow().clone();
        // The shared folder and the clipboard both reach a guest through what Ubuntu doesn't have yet.
        let group =
            adw::PreferencesGroup::builder().title("Sharing").visible(machine.kind != Kind::Linux).build();
        page.add(&group);
        let clipboard = adw::SwitchRow::builder()
            .title("Clipboard")
            .active(machine.clipboard)
            .visible(machine.kind == Kind::Windows)
            .subtitle("Copied text, both ways")
            .build();
        explain(
            &clipboard,
            "Shares copied text with the guest, both ways, while its screen is open and its window has focus. \
             Takes effect when the screen opens.",
        );
        let saving = shared.clone();
        clipboard.connect_active_notify(move |row| {
            saving.machine.borrow_mut().clipboard = row.is_active();
            saving.save();
        });
        group.add(&clipboard);
        let files = adw::SwitchRow::builder()
            .title("Drag Files")
            .active(!machine.file_drop_off)
            .visible(machine.kind == Kind::Windows)
            .subtitle("Both ways, as copies")
            .build();
        explain(
            &files,
            "Files dropped on the guest's screen land where its pointer is, and files dragged off the edge of \
             the screen come to this PC as a copy. Takes effect when the screen opens.",
        );
        let saving = shared.clone();
        files.connect_active_notify(move |row| {
            saving.machine.borrow_mut().file_drop_off = !row.is_active();
            saving.save();
        });
        group.add(&files);
        let folder = adw::ActionRow::builder().title("Shared Folder").subtitle_lines(2).build();
        let key = (!shared.is_draft()).then(|| share_key(&shared.window.context(), &machine).ok()).flatten();
        let address = key.as_deref().map(share_address);
        folder
            .add_suffix(&info_button(&format!("The guest sees it {}.", place(&machine, address.as_deref()))));
        let choose = row_button("Choose");
        let clear = row_button("Clear");
        folder.add_suffix(&choose);
        folder.add_suffix(&clear);
        group.add(&folder);
        let read_only = adw::SwitchRow::builder()
            .title("Read Only")
            .active(machine.share_read_only)
            .subtitle("The guest can open the folder's files and not change them")
            .build();
        explain(
            &read_only,
            "The guest can read the shared folder and can't change, add or remove anything in it.",
        );
        let saving = shared.clone();
        read_only.connect_active_notify(move |row| {
            saving.machine.borrow_mut().share_read_only = row.is_active();
            saving.save();
        });
        group.add(&read_only);
        let this =
            Self(Rc::new(Parts { shared: shared.clone(), folder, clear, read_only, off: Cell::new(true) }));
        let handle = this.clone();
        this.0.clear.connect_clicked(move |_| handle.set_folder(String::new()));
        let handle = this.clone();
        choose.connect_clicked(move |_| handle.on_choose());
        this.show_folder();
        this
    }

    fn show_folder(&self) {
        let machine = self.0.shared.machine.borrow();
        let share = &machine.share;
        let shown = if share.is_empty() { "None" } else { share.as_str() };
        let text = if self.0.off.get() {
            shown.to_owned()
        } else {
            format!("{shown}. The VM must be off to change this.")
        };
        self.0.folder.set_subtitle(&gtk::glib::markup_escape_text(&text));
        self.0.clear.set_visible(!share.is_empty());
        self.0.read_only.set_visible(!share.is_empty());
    }

    fn on_choose(&self) {
        let window = self.0.shared.window.clone();
        let failing = window.clone();
        let this = self.clone();
        choose_folder(
            window.widget(),
            "Choose a Folder to Share",
            move |path| this.set_folder(path),
            move |text| failing.problem(&text),
        );
    }

    fn set_folder(&self, path: String) {
        if let Some(problem) = share_problem(&self.0.shared.window.context(), &path) {
            self.0.shared.window.problem(&problem);
            return;
        }
        self.0.shared.machine.borrow_mut().share = path;
        self.0.shared.save();
        self.show_folder();
    }

    pub fn set_status(&self, status: &Status) {
        self.0.off.set(status.state == State::Off);
        self.0.folder.set_sensitive(super::is_off(status));
        self.0.read_only.set_sensitive(super::is_off(status));
        self.show_folder();
    }
}
