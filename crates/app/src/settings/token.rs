//! For a Windows machine installed before each machine had its own token: the app can't reach its
//! guest until the user points at the old token file, and only the user knows where that is.

use super::Shared;
use crate::common::choose_file;
use crate::rows::{explain, row_button};
use adw::prelude::*;
use guesthouse_app_core::agent::import_token;
use guesthouse_app_core::machine::Kind;
use std::path::Path;

pub struct Token;

impl Token {
    /// The group goes first on `page`, as the app can't reach the guest until it's done.
    pub fn new(shared: &Shared, page: &adw::PreferencesPage) -> Self {
        let machine = shared.machine.borrow().clone();
        let context = shared.window.context();
        let needed = !shared.is_draft()
            && machine.kind == Kind::Windows
            && !machine.installing
            && !machine.token_file(&context).exists();
        if !needed {
            return Self;
        }
        let group = adw::PreferencesGroup::builder().title("Guest Token").build();
        page.add(&group);
        let row = adw::ActionRow::builder()
            .title("Token")
            .subtitle("None, so the app can't reach the guest")
            .build();
        explain(
            &row,
            "This machine was installed before each machine had its own token. Choose the token file it was installed with.",
        );
        let choose = row_button("Choose");
        row.add_suffix(&choose);
        group.add(&row);
        let shared = shared.clone();
        choose.connect_clicked(move |_| ask(&shared, &group));
        Self
    }
}

fn ask(shared: &Shared, group: &adw::PreferencesGroup) {
    let (window, failing) = (shared.window.clone(), shared.window.clone());
    let (shared, group) = (shared.clone(), group.clone());
    choose_file(
        window.widget(),
        "Choose the Guest Token File",
        "Token files",
        "token",
        move |path| match import_token(&shared.window.context(), &shared.machine.borrow(), Path::new(&path)) {
            Ok(()) => group.set_visible(false),
            Err(failure) => shared.window.problem(&failure.0),
        },
        move |text| failing.problem(&text),
    );
}
