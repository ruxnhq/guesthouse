//! What the app itself can be told, as opposed to a machine: for now, its colours.

use crate::rows::choice_row;
use crate::theme;
use crate::window::Window;
use adw::prelude::*;
use guesthouse_app_core::settings::read_prefs;
use serde_json::{Value, json};

/// The colours to choose from, and what the preference holds for each. The first follows the
/// desktop, and is what anything else in the preference means too.
const SCHEMES: [(&str, &str); 3] = [("Follow the desktop", ""), ("Dark", "dark"), ("Light", "light")];
const WIDTH: i32 = 520;

pub fn show(window: &Window) {
    let saved = read_prefs(&window.context()).get(theme::SCHEME).and_then(Value::as_str).map(str::to_owned);
    let names: Vec<&str> = SCHEMES.iter().map(|(name, _)| *name).collect();
    let colours = choice_row("Colours", &names);
    let chosen = SCHEMES.iter().position(|(_, value)| Some(*value) == saved.as_deref()).unwrap_or(0);
    colours.set_selected(chosen as u32);
    let remembering = window.clone();
    colours.connect_selected_notify(move |row| {
        let value = SCHEMES.get(row.selected() as usize).map_or("", |(_, value)| *value);
        remembering.remember(json!({ theme::SCHEME: value }));
        theme::set_scheme(&read_prefs(&remembering.context()));
    });
    let look = adw::PreferencesGroup::builder().title("Look").build();
    look.add(&colours);
    let page = adw::PreferencesPage::new();
    page.add(&look);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&page));
    let dialog = adw::Dialog::builder().title("Preferences").child(&view).content_width(WIDTH).build();
    dialog.present(Some(window.widget()));
}
