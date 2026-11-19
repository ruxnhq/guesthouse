//! The app's own look: its colours, in dark and in light, and the shapes of its widgets.
//!
//! libadwaita's look is GNOME's, and most people who run this aren't on GNOME. So the app carries its
//! look with it and is the same on every desktop. The colours go in under the names libadwaita reads
//! its own from, which is what lets one set of shapes serve both.

use guesthouse_app_core::settings::Fields;
use serde_json::Value;

const DARK: &str = include_str!("../data/style/dark.css");
const LIGHT: &str = include_str!("../data/style/light.css");
const SHAPES: &str = include_str!("../data/style/shapes.css");
/// Above USER, as a desktop can write colours of its own into the user's gtk.css.
const PRIORITY: u32 = gtk::STYLE_PROVIDER_PRIORITY_USER + 1;
/// The preference that says which colours to use.
pub const SCHEME: &str = "scheme";

/// What the preference asks for. Anything else follows the desktop, and is dark where the desktop
/// doesn't say.
fn scheme(prefs: &Fields) -> adw::ColorScheme {
    match prefs.get(SCHEME).and_then(Value::as_str) {
        Some("dark") => adw::ColorScheme::ForceDark,
        Some("light") => adw::ColorScheme::ForceLight,
        _ => adw::ColorScheme::PreferDark,
    }
}

/// The colours first, as the shapes name them.
fn style(dark: bool) -> String {
    let colours = if dark { DARK } else { LIGHT };
    format!("{colours}\n{SHAPES}")
}

/// Gives every window on this display the app's look, and keeps it in step with dark and light.
pub fn install(display: &gtk::gdk::Display, prefs: &Fields) {
    let manager = adw::StyleManager::default();
    manager.set_color_scheme(scheme(prefs));
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&style(manager.is_dark()));
    gtk::style_context_add_provider_for_display(display, &provider, PRIORITY);
    manager.connect_dark_notify(move |manager| provider.load_from_string(&style(manager.is_dark())));
}

/// Changes the colours while the app runs.
pub fn set_scheme(prefs: &Fields) {
    adw::StyleManager::default().set_color_scheme(scheme(prefs));
}
