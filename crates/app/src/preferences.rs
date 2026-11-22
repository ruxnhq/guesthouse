//! What the app itself can be told, as opposed to a machine: its colours, whether a login starts it,
//! and where the desktop's shortcuts go from a machine's screen.

use crate::login;
use crate::notify;
use crate::rows::choice_row;
use crate::theme;
use crate::viewer::{self, CAPTURE_KEYS};
use crate::window::{TRAY, Window};
use adw::prelude::*;
use guesthouse_app_core::settings::read_prefs;
use serde_json::{Value, json};

/// The colours to choose from, and what the preference holds for each. The first follows the
/// desktop, and is what anything else in the preference means too.
const SCHEMES: [(&str, &str); 3] = [("Follow the desktop", ""), ("Dark", "dark"), ("Light", "light")];
/// Whose look the app has: the desktop's GTK theme, or its own.
const LOOKS: [&str; 2] = ["The desktop's", "Guesthouse"];
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
    let desktops_look = theme::desktops_look(&prefs);
    let whose = choice_row("Theme", &LOOKS);
    whose.set_subtitle("Guesthouse's own is the same on every desktop.");
    whose.set_selected(u32::from(!desktops_look));
    // The accent is the theme's when the theme is the desktop's.
    accent.set_sensitive(!desktops_look);
    let (remembering, accent_row) = (window.clone(), accent.clone());
    whose.connect_selected_notify(move |row| {
        let desktops = row.selected() == 0;
        let value = if desktops { theme::DESKTOP_LOOK } else { theme::OWN_LOOK };
        remembering.remember(json!({ theme::LOOK_OF: value }));
        accent_row.set_sensitive(!desktops);
        theme::follow(&read_prefs(&remembering.context()));
    });
    let look = adw::PreferencesGroup::builder().title("Look").build();
    look.add(&whose);
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
    let in_tray = adw::SwitchRow::builder()
        .title("Keep running in the tray")
        .subtitle("Closing the window leaves the app running, with its icon in the tray to call it back.")
        .active(window.has_tray())
        .build();
    let asking = window.clone();
    in_tray.connect_active_notify(move |row| {
        let wanted = row.is_active();
        let there = asking.set_tray(wanted, false);
        asking.remember(json!({ TRAY: there }));
        if wanted && !there {
            asking.problem("This desktop has no tray for the icon to go in.");
            row.set_active(false);
        }
    });
    let notifications = adw::SwitchRow::builder()
        .title("Notifications")
        .subtitle("While the window isn't in front: a machine that has started, been installed or stopped by itself, and a new version.")
        .active(notify::wanted(window))
        .build();
    let remembering = window.clone();
    notifications.connect_active_notify(move |row| {
        remembering.remember(json!({ notify::NOTIFICATIONS: row.is_active() }))
    });
    let background = adw::PreferencesGroup::builder().title("In the Background").build();
    background.add(&in_tray);
    background.add(&notifications);
    let shortcuts = adw::SwitchRow::builder()
        .title("Send the desktop's shortcuts to the guest")
        .subtitle(format!(
            "Keys such as Alt+Tab and Super, whenever a machine's screen has the keyboard. Turned off, your \
             desktop keeps them, as a window manager run by its shortcuts, such as Hyprland or Sway, needs. \
             Typing still goes to the guest, and {CAPTURE_KEYS} hands it the shortcuts with the mouse until \
             it's pressed again."
        ))
        .active(viewer::shortcuts_on_focus(&prefs))
        .build();
    let remembering = window.clone();
    shortcuts.connect_active_notify(move |row| {
        remembering.remember(json!({ viewer::SHORTCUTS_ON_FOCUS: row.is_active() }))
    });
    let screens = adw::PreferencesGroup::builder().title("Machines' Screens").build();
    screens.add(&shortcuts);
    let page = adw::PreferencesPage::new();
    page.add(&look);
    page.add(&starting);
    page.add(&background);
    page.add(&screens);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&page));
    let dialog = adw::Dialog::builder().title("Preferences").child(&view).content_width(WIDTH).build();
    dialog.present(Some(window.widget()));
}
