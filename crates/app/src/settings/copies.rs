//! Copies of a machine: a duplicate with a disk of its own, a throwaway that starts clean, and an
//! export for another PC.

use super::{Shared, is_off};
use crate::common::{chosen_path, in_background};
use crate::rows::{boxed_rows, on_yes, question, row_button};
use adw::prelude::*;
use gtk::gio;
use guesthouse_app_core::duplicate::{
    DELETE_WORD, DUPLICATE_TASK, THROWAWAY_WORD, copy_name, plan_duplicate,
};
use guesthouse_app_core::export::{EXPORT_TASK, EXTENSION, plan_export};
use guesthouse_app_core::tasks::Status;
use guesthouse_app_core::throwaway::set_throwaway;

const NOT_INSTALLED: &str = "It has not finished installing.";
const SHUT_DOWN_FIRST: &str = "Shut the VM down first.";

const THROWAWAY_TEXT: &str = "Starts clean every time, and what it writes to its disk is gone when it shuts down. \
                              Its shared folder and host drives keep what is written to them.";

pub struct Copies {
    shared: Shared,
    duplicate: gtk::Button,
    export: gtk::Button,
    throwaway: adw::SwitchRow,
}

impl Copies {
    pub fn new(shared: &Shared, page: &adw::PreferencesPage) -> Self {
        let group = adw::PreferencesGroup::builder().title("Copies").build();
        page.add(&group);
        let row = adw::ActionRow::builder()
            .title("Duplicate")
            .subtitle("A copy with a disk of its own. This machine stays as it is.")
            .build();
        let duplicate = row_button("Duplicate…");
        duplicate.set_sensitive(false);
        let asking = shared.clone();
        duplicate.connect_clicked(move |_| ask(&asking));
        row.add_suffix(&duplicate);
        group.add(&row);
        let throwaway = adw::SwitchRow::builder()
            .title("Throwaway")
            .subtitle(THROWAWAY_TEXT)
            .active(shared.machine.borrow().throwaway)
            .build();
        let changing = shared.clone();
        throwaway.connect_active_notify(move |row| {
            let on = row.is_active();
            if changing.machine.borrow().throwaway == on {
                return;
            }
            let context = changing.window.context();
            changing.change(|machine| set_throwaway(&context, machine, on));
            row.set_active(changing.machine.borrow().throwaway);
        });
        group.add(&throwaway);
        let row = adw::ActionRow::builder()
            .title("Export")
            .subtitle(
                "One file with its disk and settings, for Guesthouse on another PC. It holds the guest's \
                       password and agent token, so keep it as safe as the machine.",
            )
            .build();
        let export = row_button("Export…");
        export.set_sensitive(false);
        let asking = shared.clone();
        export.connect_clicked(move |_| ask_where(&asking));
        row.add_suffix(&export);
        group.add(&row);
        Self { shared: shared.clone(), duplicate, export, throwaway }
    }

    pub fn set_status(&self, status: &Status) {
        let installing = self.shared.machine.borrow().installing;
        let can = is_off(status) && !installing;
        self.duplicate.set_sensitive(can);
        let why = if installing { NOT_INSTALLED } else { SHUT_DOWN_FIRST };
        self.duplicate.set_tooltip_text((!can).then_some(why));
        self.throwaway.set_sensitive(can);
        self.export.set_sensitive(can);
        self.export.set_tooltip_text((!can).then_some(why));
    }
}

/// Works out what the copy costs first, as that is what the question says.
fn ask(shared: &Shared) {
    let context = shared.window.context();
    let machine = shared.machine.borrow().clone();
    let name = copy_name(&machine);
    let planning = context.clone();
    let shared = shared.clone();
    in_background(
        &context,
        move || plan_duplicate(&planning, &machine, &name, false),
        move |planned| match planned {
            Ok(plan) => show_question(&shared, &plan.copy.name, &plan.cost_text()),
            Err(problem) => shared.window.problem(&problem),
        },
    );
}

fn show_question(shared: &Shared, suggested: &str, cost: &str) {
    let name = adw::EntryRow::builder().title("Name").text(suggested).build();
    let throwaway = adw::SwitchRow::builder().title("Throwaway").subtitle(THROWAWAY_TEXT).build();
    throwaway.set_active(shared.machine.borrow().throwaway);
    throwaway.set_sensitive(!shared.machine.borrow().throwaway);
    let body = format!(
        "{cost} The copy has its own network address. Its guest and this one each get a new agent token at \
         their next start."
    );
    let dialog = question("Duplicate", &body, "Duplicate", adw::ResponseAppearance::Suggested);
    let delete = adw::SwitchRow::builder()
        .title("Delete It Once It Has Stopped")
        .subtitle("After its first run, the whole copy goes, when the app or guesthouse-vm next looks.")
        .sensitive(throwaway.is_active())
        .build();
    let following = delete.clone();
    throwaway.connect_active_notify(move |row| {
        following.set_sensitive(row.is_active());
        if !row.is_active() {
            following.set_active(false);
        }
    });
    let rows = [name.upcast_ref(), throwaway.upcast_ref(), delete.upcast_ref()];
    dialog.set_extra_child(Some(&boxed_rows(&rows)));
    let enabling = dialog.clone();
    name.connect_changed(move |row| enabling.set_response_enabled("yes", !row.text().trim().is_empty()));
    let shared = shared.clone();
    let parent = shared.dialog.clone();
    on_yes(&dialog, move || {
        let mut words = vec![name.text().trim().to_owned()];
        if throwaway.is_active() {
            words.push(THROWAWAY_WORD.to_owned());
        }
        if throwaway.is_active() && delete.is_active() {
            words.push(DELETE_WORD.to_owned());
        }
        shared.window.launch(DUPLICATE_TASK, &shared.machine.borrow(), &words, None);
    });
    dialog.present(Some(&parent));
}

/// Asks where the file goes, then checks it fits before the task writes it.
fn ask_where(shared: &Shared) {
    let machine = shared.machine.borrow().clone();
    let name = format!("{}.{EXTENSION}", machine.id);
    let chooser = gtk::FileDialog::builder().title("Export").initial_name(name).build();
    let shared = shared.clone();
    let parent = shared.window.widget().clone();
    chooser.save(Some(&parent), gio::Cancellable::NONE, move |result| {
        let Some(path) = chosen_path(result, &|text| shared.window.problem(&text)) else { return };
        let context = shared.window.context();
        let planning = context.clone();
        let machine = shared.machine.borrow().clone();
        let to = path.clone();
        let shared = shared.clone();
        in_background(
            &context,
            move || plan_export(&planning, &machine, &to),
            move |planned| match planned {
                Ok(_) => {
                    let words = [path.to_string_lossy().into_owned()];
                    shared.window.launch(EXPORT_TASK, &shared.machine.borrow(), &words, None);
                }
                Err(problem) => shared.window.problem(&problem),
            },
        );
    });
}
