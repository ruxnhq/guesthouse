//! What comes after Send or Save: the report's ID and the offer of an issue, or a way not to lose a
//! report that wasn't sent.

use crate::common::chosen_path;
use crate::window::Window;
use adw::prelude::*;
use gtk::gio;
use guesthouse_app_core::report::Bundle;
use std::fs;

const SPACING: i32 = 6;

/// Shows the ID with a Copy button, and opens the browser on the issue only if the tester says so.
pub fn sent(window: &Window, id: &str, link: &str) {
    let body = "Mention its ID wherever you talk about this problem.\n\nAlso open a GitHub issue? Your browser \
                opens on one filled in with the ID, the report's facts, what you wrote and a few lines \
                of the app's own log, for you to check before you post it.";
    let dialog = adw::AlertDialog::new(Some("Report Sent"), Some(body));
    let id_label = gtk::Label::builder().label(id).selectable(true).build();
    id_label.add_css_class("monospace");
    id_label.add_css_class("title-3");
    let copy = gtk::Button::with_label("Copy");
    let row = gtk::Box::builder().spacing(SPACING).halign(gtk::Align::Center).build();
    row.append(&id_label);
    row.append(&copy);
    dialog.set_extra_child(Some(&row));
    let copied = id.to_owned();
    let handle = window.clone();
    copy.connect_clicked(move |button| {
        button.clipboard().set_text(&copied);
        handle.toast("The report's ID is copied.", adw::ToastPriority::Normal);
    });
    dialog.add_response("no", "No");
    dialog.add_response("issue", "Open an Issue");
    dialog.set_response_appearance("issue", adw::ResponseAppearance::Suggested);
    dialog.set_close_response("no");
    let link = link.to_owned();
    let parent = window.widget().clone();
    dialog.connect_response(Some("issue"), move |_, _| {
        gtk::UriLauncher::new(&link).launch(Some(&parent), gio::Cancellable::NONE, |_| {});
    });
    dialog.present(Some(window.widget()));
}

/// Says why, and offers to save the report so it isn't lost.
pub fn not_sent(window: &Window, parent: &adw::Dialog, reason: &str, bundle: Bundle) {
    let body = format!(
        "{reason}\n\nSave the report to a file so it isn't lost. You can attach it to an issue, or send it \
         later with guesthouse-report."
    );
    let dialog = adw::AlertDialog::new(Some("The Report Wasn't Sent"), Some(&body));
    dialog.add_response("close", "Close");
    dialog.add_response("save", "Save to File…");
    dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
    dialog.set_close_response("close");
    let handle = window.clone();
    let report = parent.clone();
    dialog.connect_response(Some("save"), move |_, _| save_as(&handle, &report, bundle.clone()));
    dialog.present(Some(parent));
}

/// Asks where, and copies the written report there.
pub fn save_as(window: &Window, parent: &adw::Dialog, bundle: Bundle) {
    let name = bundle.path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let chooser = gtk::FileDialog::builder().title("Save Report").initial_name(name).build();
    let handle = window.clone();
    let report = parent.clone();
    chooser.save(Some(window.widget()), gio::Cancellable::NONE, move |result| {
        let Some(path) = chosen_path(result, &|text| handle.problem(&text)) else { return };
        match fs::copy(&bundle.path, &path) {
            Ok(_) => {
                report.close();
                handle.toast(&format!("Saved to {}", path.display()), adw::ToastPriority::Normal);
            }
            Err(error) => handle.problem(&format!("The report couldn't be saved there: {error}")),
        }
    });
}
