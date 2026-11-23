//! Snapshots: taking one, going back, and deleting.

use super::{Shared, is_off};
use crate::common::in_background;
use crate::rows::{boxed_rows, confirm, icon_button, on_yes, question, row_button};
use adw::prelude::*;
use guesthouse_app_core::auto_snapshot::{NOT_FOR_THROWAWAY, disk_can_hold};
use guesthouse_app_core::machine::Kind;
use guesthouse_app_core::snapshots::{Snapshot, list_snapshots};
use guesthouse_app_core::tasks::Status;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const NOT_INSTALLED: &str = "Windows has not finished installing.";
const SHUT_DOWN_FIRST: &str = "Shut the VM down first.";
const AUTO_TITLE: &str = "Snapshot Before Risky Changes";
const AUTO_NOTE: &str = "Before a start that changes Windows in a way it might not come back from, Guesthouse \
                         takes a snapshot of its own. It replaces the last one, and yours stay. If that start \
                         fails, you can go back to it.";
const AUTO_DISK: &str = "This disk can't hold snapshots, as only a qcow2 disk can.";
const AUTO_BY: &str = "Taken by Guesthouse before a start";
const AUTO_ICON: &str = "document-save-symbolic";

fn taken_text(snapshot: &Snapshot) -> String {
    let time = Timestamp::from_second(snapshot.taken).ok();
    time.map_or_else(String::new, |time| {
        time.to_zoned_datetime(TimeZone::system()).strftime("%-d %B %Y, %H:%M").to_string()
    })
}

struct Parts {
    shared: Shared,
    group: adw::PreferencesGroup,
    /// Windows machines only: the other kinds have no change for it to be before.
    auto: Option<adw::SwitchRow>,
    take: gtk::Button,
    rows: RefCell<Vec<adw::ActionRow>>,
    can_change: Cell<bool>,
    busy: Cell<bool>,
    /// Whether the disk can hold a snapshot, once it has been read.
    disk_can_hold: Cell<bool>,
}

#[derive(Clone)]
pub struct Snapshots(Rc<Parts>);

impl Snapshots {
    pub fn new(shared: &Shared, page: &adw::PreferencesPage) -> Self {
        let group = adw::PreferencesGroup::builder()
            .title("Snapshots")
            .description("They live inside the disk file, which grows as the guest changes what they hold.")
            .build();
        // Ubuntu's are left out for now: a snapshot here keeps a firmware file, and it has none.
        group.set_visible(shared.machine.borrow().kind != Kind::Linux);
        page.add(&group);
        let take = row_button("Take Snapshot");
        take.set_sensitive(false);
        group.set_header_suffix(Some(&take));
        let auto = (shared.machine.borrow().kind == Kind::Windows).then(|| Self::auto_row(shared, &group));
        let this = Self(Rc::new(Parts {
            shared: shared.clone(),
            group,
            auto,
            take,
            rows: RefCell::default(),
            can_change: Cell::new(false),
            busy: Cell::new(false),
            disk_can_hold: Cell::new(true),
        }));
        let handle = this.clone();
        this.0.take.connect_clicked(move |_| handle.on_take());
        this.read();
        this
    }

    fn auto_row(shared: &Shared, group: &adw::PreferencesGroup) -> adw::SwitchRow {
        let on = shared.machine.borrow().auto_snapshot_on();
        let row = adw::SwitchRow::builder().title(AUTO_TITLE).subtitle(AUTO_NOTE).active(on).build();
        let saving = shared.clone();
        row.connect_active_notify(move |row| {
            saving.machine.borrow_mut().auto_snapshot = Some(row.is_active());
            saving.save();
        });
        group.add(&row);
        row
    }

    fn read(&self) {
        let context = self.0.shared.window.context();
        let machine = self.0.shared.machine.borrow().clone();
        let reading = context.clone();
        let this = self.clone();
        in_background(
            &context,
            move || Ok((list_snapshots(&reading, &machine), disk_can_hold(&machine))),
            move |result| match result {
                Ok((found, can_hold)) => {
                    this.show(&found);
                    this.show_auto_disk(can_hold);
                }
                Err(text) => this.0.shared.window.problem(&text),
            },
        );
    }

    fn show_auto_disk(&self, can_hold: bool) {
        self.0.disk_can_hold.set(can_hold);
        self.show_auto();
    }

    /// A disk that can't hold a snapshot, or a throwaway, gets none by itself, and the switch says why.
    fn show_auto(&self) {
        let Some(row) = &self.0.auto else { return };
        let why = if self.0.shared.machine.borrow().throwaway {
            Some(NOT_FOR_THROWAWAY)
        } else {
            (!self.0.disk_can_hold.get()).then_some(AUTO_DISK)
        };
        if row.is_sensitive() != why.is_none() || row.subtitle().as_deref() != Some(why.unwrap_or(AUTO_NOTE))
        {
            row.set_sensitive(why.is_none());
            row.set_subtitle(why.unwrap_or(AUTO_NOTE));
        }
    }

    fn row_for(&self, snapshot: &Snapshot) -> adw::ActionRow {
        let when = taken_text(snapshot);
        let subtitle = if snapshot.automatic { format!("{AUTO_BY}, {when}") } else { when };
        let row =
            adw::ActionRow::builder().title(&snapshot.name).use_markup(false).subtitle(subtitle).build();
        if snapshot.automatic {
            row.add_prefix(&gtk::Image::from_icon_name(AUTO_ICON));
        }
        let back = row_button("Go Back");
        let this = self.clone();
        let chosen = snapshot.clone();
        back.connect_clicked(move |_| this.on_go_back(&chosen));
        row.add_suffix(&back);
        let delete = icon_button("user-trash-symbolic", "Delete Snapshot");
        let this = self.clone();
        let chosen = snapshot.clone();
        delete.connect_clicked(move |_| this.on_delete(&chosen));
        row.add_suffix(&delete);
        row
    }

    fn show(&self, snapshots: &[Snapshot]) {
        let parts = &self.0;
        for row in parts.rows.borrow_mut().drain(..) {
            parts.group.remove(&row);
        }
        let mut rows: Vec<adw::ActionRow> = snapshots.iter().map(|snapshot| self.row_for(snapshot)).collect();
        if rows.is_empty() {
            rows.push(adw::ActionRow::builder().title("None yet").build());
        }
        for row in &rows {
            row.set_sensitive(parts.can_change.get());
            parts.group.add(row);
        }
        *parts.rows.borrow_mut() = rows;
    }

    fn on_take(&self) {
        let name = adw::EntryRow::builder().title("Name").build();
        let dialog = question(
            "Take a Snapshot",
            "This keeps the disk and the firmware settings as they are now.",
            "Take Snapshot",
            adw::ResponseAppearance::Suggested,
        );
        dialog.set_extra_child(Some(&boxed_rows(&[name.upcast_ref()])));
        dialog.set_response_enabled("yes", false);
        let enabling = dialog.clone();
        name.connect_changed(move |row| enabling.set_response_enabled("yes", !row.text().trim().is_empty()));
        let shared = self.0.shared.clone();
        on_yes(&dialog, move || {
            shared.window.launch("take_snapshot", &shared.machine.borrow(), &[name.text().to_string()], None);
        });
        dialog.present(Some(&self.0.shared.dialog));
    }

    fn on_go_back(&self, snapshot: &Snapshot) {
        let shared = self.0.shared.clone();
        let tag = snapshot.tag.clone();
        let body = format!(
            "Everything done on {} since {} is lost: files, programs and changes to the guest. If the disk \
             grew since then, it shrinks back. Other snapshots stay. This cannot be undone.",
            shared.machine.borrow().name,
            taken_text(snapshot)
        );
        let parent = shared.dialog.clone();
        confirm(&parent, &format!("Go Back to {}?", snapshot.name), &body, "Go Back", move || {
            shared.window.launch("go_back", &shared.machine.borrow(), std::slice::from_ref(&tag), None);
        });
    }

    fn on_delete(&self, snapshot: &Snapshot) {
        let shared = self.0.shared.clone();
        let tag = snapshot.tag.clone();
        let body = "You can no longer go back to it. The machine stays as it is now.";
        let parent = shared.dialog.clone();
        confirm(&parent, &format!("Delete {}?", snapshot.name), body, "Delete", move || {
            shared.window.launch(
                "delete_snapshot",
                &shared.machine.borrow(),
                std::slice::from_ref(&tag),
                None,
            );
        });
    }

    pub fn set_status(&self, status: &Status) {
        let parts = &self.0;
        let installing = parts.shared.machine.borrow().installing;
        let can_change = is_off(status) && !installing;
        parts.can_change.set(can_change);
        parts.take.set_sensitive(can_change);
        let why = if installing { NOT_INSTALLED } else { SHUT_DOWN_FIRST };
        parts.take.set_tooltip_text((!can_change).then_some(why));
        for row in parts.rows.borrow().iter() {
            row.set_sensitive(can_change);
        }
        // Throwaway is switched on the General page, and this follows it.
        self.show_auto();
        if parts.busy.replace(status.busy) != status.busy {
            self.read();
        }
    }
}
