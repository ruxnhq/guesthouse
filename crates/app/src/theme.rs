//! The app's own look: its colours, in dark and in light, and the shapes of its widgets.
//!
//! libadwaita's look is GNOME's, and most people who run this aren't on GNOME. So the app carries its
//! look with it and is the same on every desktop. The colours go in under the names libadwaita reads
//! its own from, which is what lets one set of shapes serve both.

use adw::prelude::*;
use gtk::gio;
use guesthouse_app_core::settings::Fields;
use serde_json::Value;
use std::cell::RefCell;

const DARK: &str = include_str!("../data/style/dark.css");
const LIGHT: &str = include_str!("../data/style/light.css");
const SHAPES: &str = include_str!("../data/style/shapes.css");
/// Above USER, as a desktop can write colours of its own into the user's gtk.css.
const PRIORITY: u32 = gtk::STYLE_PROVIDER_PRIORITY_USER + 1;
/// The preference that says which colours to use.
pub const SCHEME: &str = "scheme";
/// The preference that says whose accent colour to use, and what it holds for the desktop's.
pub const ACCENT: &str = "accent";
pub const DESKTOP_ACCENT: &str = "desktop";
/// How long the desktop gets to say its accent colour. It's asked while the app starts.
const PORTAL_WAIT_MS: i32 = 500;
/// Above this lightness an accent takes dark text on it, and below it light.
const LIGHT_ACCENT: f64 = 0.55;

/// Red, green and blue, each from 0 to 1.
type Rgb = (f64, f64, f64);

thread_local! {
    /// The stylesheet in use, and the desktop's accent if that's the one wanted.
    static LOOK: RefCell<Option<(gtk::CssProvider, Option<Rgb>)>> = const { RefCell::new(None) };
}

/// What the preference asks for. Anything else follows the desktop, and is dark where the desktop
/// doesn't say.
fn scheme(prefs: &Fields) -> adw::ColorScheme {
    match prefs.get(SCHEME).and_then(Value::as_str) {
        Some("dark") => adw::ColorScheme::ForceDark,
        Some("light") => adw::ColorScheme::ForceLight,
        _ => adw::ColorScheme::PreferDark,
    }
}

/// The desktop's accent colour, where it has one and a settings portal to say it through.
fn desktop_accent() -> Option<Rgb> {
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).ok()?;
    let asked = ("org.freedesktop.appearance", "accent-color").to_variant();
    let reply = bus
        .call_sync(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.Settings",
            "ReadOne",
            Some(&asked),
            None,
            gio::DBusCallFlags::NONE,
            PORTAL_WAIT_MS,
            gio::Cancellable::NONE,
        )
        .ok()?;
    let colour = reply.child_value(0).as_variant()?.get::<Rgb>()?;
    // A desktop with no accent set answers with numbers outside the range.
    let in_range = |part: f64| (0.0..=1.0).contains(&part);
    (in_range(colour.0) && in_range(colour.1) && in_range(colour.2)).then_some(colour)
}

fn wanted_accent(prefs: &Fields) -> Option<Rgb> {
    let desktop = prefs.get(ACCENT).and_then(Value::as_str) == Some(DESKTOP_ACCENT);
    desktop.then(desktop_accent).flatten()
}

/// The accent's colours under libadwaita's names, to go after the palette's own.
fn accent_style((red, green, blue): Rgb) -> String {
    let byte = |part: f64| (part * 255.0).round() as u8;
    let colour = format!("rgb({}, {}, {})", byte(red), byte(green), byte(blue));
    // How light the eye takes it to be, which decides what can be read on it.
    let lightness = 0.2126 * red + 0.7152 * green + 0.0722 * blue;
    let on_it = if lightness > LIGHT_ACCENT { "#07080a" } else { "#ffffff" };
    format!(
        "@define-color accent_color {colour};\n@define-color accent_bg_color {colour};\n\
         @define-color accent_fg_color {on_it};\n"
    )
}

/// The colours first, as the shapes name them.
fn style(dark: bool, accent: Option<Rgb>) -> String {
    let colours = if dark { DARK } else { LIGHT };
    let accent = accent.map(accent_style).unwrap_or_default();
    format!("{colours}\n{accent}\n{SHAPES}")
}

fn load() {
    LOOK.with_borrow(|look| {
        if let Some((provider, accent)) = look {
            provider.load_from_string(&style(adw::StyleManager::default().is_dark(), *accent));
        }
    });
}

/// Gives every window on this display the app's look, and keeps it in step with dark and light.
pub fn install(display: &gtk::gdk::Display, prefs: &Fields) {
    let manager = adw::StyleManager::default();
    manager.set_color_scheme(scheme(prefs));
    let provider = gtk::CssProvider::new();
    gtk::style_context_add_provider_for_display(display, &provider, PRIORITY);
    LOOK.set(Some((provider, wanted_accent(prefs))));
    load();
    manager.connect_dark_notify(|_| load());
}

/// Takes in changed preferences while the app runs.
pub fn follow(prefs: &Fields) {
    adw::StyleManager::default().set_color_scheme(scheme(prefs));
    let accent = wanted_accent(prefs);
    LOOK.with_borrow_mut(|look| {
        if let Some((_, old)) = look {
            *old = accent;
        }
    });
    load();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_accent_takes_text_that_can_be_read_on_it() {
        assert!(accent_style((0.95, 0.85, 0.3)).contains("accent_fg_color #07080a"));
        assert!(accent_style((0.1, 0.2, 0.6)).contains("accent_fg_color #ffffff"));
        assert!(accent_style((1.0, 0.0, 0.5)).contains("accent_bg_color rgb(255, 0, 128)"));
    }

    #[test]
    fn the_accent_comes_after_the_palette_and_before_the_shapes() {
        let text = style(true, Some((0.0, 1.0, 0.0)));
        let own = text.find("rgb(0, 255, 0)").unwrap();
        assert!(text.find("#8fb4ff").unwrap() < own, "the palette's accent has to come first to lose");
        assert!(own < text.find("headerbar {").unwrap());
        assert!(!style(true, None).contains("accent_bg_color rgb("));
    }
}
