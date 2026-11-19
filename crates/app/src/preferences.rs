//! What the app itself can be told, as opposed to a machine: its colours, and whether a login
//! starts it.

use crate::login;
use crate::rows::choice_row;
use crate::theme;
use crate::window::Window;
use adw::prelude::*;
use guesthouse_app_core::settings::read_prefs;
use serde_json::{Value, json};

/// The colours to choose from, and what the preference holds for each. The first follows the
/// desktop, and is what anything else in the preference means too.
const SCHEMES: [(&str, &str); 3] = [("Follow the desktop", ""), ("Dark", "dark"), ("Light", "light")];
/// The second is the desktop's own, where it has one.
const ACCENTS: [&str; 2] = ["Blue", "Follow the desktop"];
const WIDTH: i32 = 520;

pub fn show(window: &Window) {
    let prefs = read_prefs(&window.context());
    let saved = prefs.get(theme::SCHEME).and_then(Value::as_str).map(str::to_owned);
    let names: Vec<&str> = SCHEMES.iter().map(|(name, _)| *name).collect();
    let colours = choice_row("Colours", &names);
    let chosen = SCHEMES.iter().position(|(_, value)| Some(*value) == saved.as_deref()).unwrap_or(0);
    colours.set_selected(chosen as u32);
    let remembering = window.clone();
    colours.connect_selected_notify(move |row| {
        let value = SCHEMES.get(row.selected() as usize).map_or("", |(_, value)| *value);
        remembering.remember(json!({ theme::SCHEME: value }));
        theme::follow(&read_prefs(&remembering.context()));
    });
    let desktops = prefs.get(theme::ACCENT).and_then(Value::as_str) == Some(theme::DESKTOP_ACCENT);
    let accent = choice_row("Accent", &ACCENTS);
    accent.set_subtitle("The desktop's is read when the app starts, and when this is changed.");
    accent.set_selected(u32::from(desktops));
    let remembering = window.clone();
    accent.connect_selected_notify(move |row| {
        let value = if row.selected() == 1 { theme::DESKTOP_ACCENT } else { "" };
        remembering.remember(json!({ theme::ACCENT: value }));
        theme::follow(&read_prefs(&remembering.context()));
    });
    let look = adw::PreferencesGroup::builder().title("Look").build();
    look.add(&colours);
    look.add(&accent);
    let at_login = adw::SwitchRow::builder()
        .title("Start Guesthouse at login")
        .subtitle("Machines set to start with this PC start then.")
        .active(login::starts_at_login())
        .build();
    let telling = window.clone();
    at_login.connect_active_notify(move |row| {
        if let Err(failure) = login::set_starts_at_login(row.is_active()) {
            telling.problem(&format!("Could not change what starts at login: {failure}"));
        }
    });
    let starting = adw::PreferencesGroup::builder().title("Starting").build();
    starting.add(&at_login);
    let page = adw::PreferencesPage::new();
    page.add(&look);
    page.add(&starting);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&page));
    let dialog = adw::Dialog::builder().title("Preferences").child(&view).content_width(WIDTH).build();
    dialog.present(Some(window.widget()));
}
