//! The app's own look: its colours, in dark and in light, and the shapes of its widgets.
//!
//! libadwaita's look is GNOME's, and most people who run this aren't on GNOME. So the app carries its
//! look with it and is the same on every desktop. The colours go in under the names libadwaita reads
//! its own from, which is what lets one set of shapes serve both.
//!
//! Someone with a theme of their own may want the app in it, as it was before it had a look. The
//! preferences can ask for that: then the theme draws everything it knows, and only the widgets
//! that are the app's own are styled here, in the theme's colours.

use adw::prelude::*;
use gtk::gio;
use guesthouse_app_core::settings::Fields;
use serde_json::Value;
use std::cell::RefCell;

const DARK: &str = include_str!("../data/style/dark.css");
const LIGHT: &str = include_str!("../data/style/light.css");
const SHAPES: &str = include_str!("../data/style/shapes.css");
const OWN_WIDGETS: &str = include_str!("../data/style/own.css");
const DESKTOP_NAMES: &str = include_str!("../data/style/desktop.css");
/// Above USER, as a desktop can write colours of its own into the user's gtk.css.
const PRIORITY: u32 = gtk::STYLE_PROVIDER_PRIORITY_USER + 1;
/// Where an app's own style usually goes, under the desktop's theme and the user's gtk.css. It's
/// where the app's own widgets go when the desktop's theme is the one wanted.
const UNDER_THE_THEME: u32 = gtk::STYLE_PROVIDER_PRIORITY_APPLICATION;
/// The preference that says whose look to use, and what it holds for the desktop's theme.
pub const LOOK_OF: &str = "look";
pub const DESKTOP_LOOK: &str = "desktop";
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

/// The stylesheet in use and what decides its text.
struct Look {
    provider: gtk::CssProvider,
    display: gtk::gdk::Display,
    /// The desktop's theme draws the stock widgets, and only the app's own widgets are styled here.
    desktops: bool,
    /// The desktop's accent, if that's the one wanted.
    accent: Option<Rgb>,
}

thread_local! {
    static LOOK: RefCell<Option<Look>> = const { RefCell::new(None) };
}

fn desktops_look(prefs: &Fields) -> bool {
    prefs.get(LOOK_OF).and_then(Value::as_str) == Some(DESKTOP_LOOK)
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
    format!("{colours}\n{accent}\n{SHAPES}\n{OWN_WIDGETS}")
}

/// Only the app's own widgets, in the theme's colours.
fn style_under_the_theme() -> String {
    format!("{DESKTOP_NAMES}\n{OWN_WIDGETS}")
}

fn load() {
    LOOK.with_borrow(|look| {
        let Some(look) = look else { return };
        let text = if look.desktops {
            style_under_the_theme()
        } else {
            style(adw::StyleManager::default().is_dark(), look.accent)
        };
        look.provider.load_from_string(&text);
    });
}

/// Puts the stylesheet over the desktop's theme or under it, by which look is wanted.
fn place(look: &Look) {
    gtk::style_context_remove_provider_for_display(&look.display, &look.provider);
    let priority = if look.desktops { UNDER_THE_THEME } else { PRIORITY };
    gtk::style_context_add_provider_for_display(&look.display, &look.provider, priority);
}

/// Gives every window on this display the look the preferences ask for, and keeps it in step with
/// dark and light.
pub fn install(display: &gtk::gdk::Display, prefs: &Fields) {
    let manager = adw::StyleManager::default();
    manager.set_color_scheme(scheme(prefs));
    let desktops = desktops_look(prefs);
    let look = Look {
        provider: gtk::CssProvider::new(),
        display: display.clone(),
        desktops,
        accent: if desktops { None } else { wanted_accent(prefs) },
    };
    place(&look);
    LOOK.set(Some(look));
    load();
    manager.connect_dark_notify(|_| load());
}

/// Takes in changed preferences while the app runs.
pub fn follow(prefs: &Fields) {
    adw::StyleManager::default().set_color_scheme(scheme(prefs));
    let desktops = desktops_look(prefs);
    let accent = if desktops { None } else { wanted_accent(prefs) };
    LOOK.with_borrow_mut(|look| {
        if let Some(look) = look {
            look.desktops = desktops;
            look.accent = accent;
            place(look);
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

    #[test]
    fn under_the_desktops_theme_only_the_apps_own_widgets_are_styled() {
        let text = style_under_the_theme();
        assert!(text.contains(".hero") && text.contains(".viewer-pill"));
        assert!(!text.contains("headerbar {") && !text.contains("switch {"));
        assert!(!text.contains("window_bg_color #"), "the theme's colours have to stay the theme's");
    }
}
