//! What KDE Plasma says of its look, read from its own files: the colour scheme in `kdeglobals` and
//! the order of the window's buttons in `kwinrc`. The settings portal says only the accent and
//! dark or light, so the rest comes from where Plasma keeps it. Without them, Breeze's colours and
//! button order stand.

use std::env;
use std::fs;
use std::path::PathBuf;

const COLOURS_FILE: &str = "kdeglobals";
const KWIN_FILE: &str = "kwinrc";
const DECORATION_GROUP: &str = "org.kde.kdecoration2";
/// Breeze's buttons: the window's menu on the left; minimise, maximise and close on the right.
const BREEZE_LEFT: &str = "MS";
const BREEZE_RIGHT: &str = "HIAX";
/// Below this lightness a window's background is a dark scheme's.
const DARK_BELOW: f64 = 0.5;

/// A colour as KDE writes it: red, green and blue from 0 to 255.
type Rgb = (u8, u8, u8);

/// Where KDE's colours go, by libadwaita's names and the app's. Each is a group and key of
/// `kdeglobals`.
const NAMED: [(&str, &str, &str); 19] = [
    ("window_bg_color", "Colors:Window", "BackgroundNormal"),
    ("window_fg_color", "Colors:Window", "ForegroundNormal"),
    ("sidebar_bg_color", "Colors:Window", "BackgroundNormal"),
    ("sidebar_backdrop_color", "Colors:Window", "BackgroundNormal"),
    ("sidebar_fg_color", "Colors:Window", "ForegroundNormal"),
    ("headerbar_bg_color", "Colors:Header", "BackgroundNormal"),
    ("headerbar_backdrop_color", "Colors:Header", "BackgroundNormal"),
    ("headerbar_fg_color", "Colors:Header", "ForegroundNormal"),
    ("view_bg_color", "Colors:View", "BackgroundNormal"),
    ("view_fg_color", "Colors:View", "ForegroundNormal"),
    ("card_bg_color", "Colors:View", "BackgroundNormal"),
    ("card_fg_color", "Colors:View", "ForegroundNormal"),
    ("dialog_bg_color", "Colors:Window", "BackgroundNormal"),
    ("dialog_fg_color", "Colors:Window", "ForegroundNormal"),
    ("popover_bg_color", "Colors:View", "BackgroundNormal"),
    ("popover_fg_color", "Colors:View", "ForegroundNormal"),
    ("field_bg_color", "Colors:Button", "BackgroundNormal"),
    ("destructive_color", "Colors:View", "ForegroundNegative"),
    ("error_color", "Colors:View", "ForegroundNegative"),
];
/// The accent when the portal has none: what Plasma selects with.
const SELECTION: (&str, &str) = ("Colors:Selection", "BackgroundNormal");

fn config_folder() -> Option<PathBuf> {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
}

fn read_config(name: &str) -> String {
    config_folder().and_then(|folder| fs::read_to_string(folder.join(name)).ok()).unwrap_or_default()
}

/// The value of `key` in `[group]` of an INI file as KDE writes them.
fn value<'a>(text: &'a str, group: &str, key: &str) -> Option<&'a str> {
    let mut in_group = false;
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|line| line.strip_suffix(']')) {
            in_group = name == group;
        } else if in_group
            && let Some((found, value)) = line.split_once('=')
            && found.trim() == key
        {
            return Some(value.trim());
        }
    }
    None
}

fn rgb(text: &str) -> Option<Rgb> {
    let mut parts = text.split(',').map(|part| part.trim().parse::<u8>().ok());
    let colour = (parts.next()??, parts.next()??, parts.next()??);
    Some(colour)
}

fn lightness((red, green, blue): Rgb) -> f64 {
    (0.2126 * f64::from(red) + 0.7152 * f64::from(green) + 0.0722 * f64::from(blue)) / 255.0
}

/// The user's KDE colours as `@define-color` lines, when the scheme is as dark as `dark` says. A
/// light scheme under a dark app, or no scheme, gives nothing, and Breeze's colours stand.
pub fn colours(dark: bool) -> String {
    colours_from(&read_config(COLOURS_FILE), dark)
}

fn colours_from(text: &str, dark: bool) -> String {
    let window = value(text, "Colors:Window", "BackgroundNormal").and_then(rgb);
    if window.is_none_or(|window| (lightness(window) < DARK_BELOW) != dark) {
        return String::new();
    }
    NAMED
        .iter()
        .filter_map(|(name, group, key)| {
            let (red, green, blue) = value(text, group, key).and_then(rgb)?;
            Some(format!("@define-color {name} rgb({red}, {green}, {blue});\n"))
        })
        .collect()
}

/// The colour Plasma selects with, from 0 to 1, for when the portal says no accent.
pub fn selection() -> Option<(f64, f64, f64)> {
    let (red, green, blue) = value(&read_config(COLOURS_FILE), SELECTION.0, SELECTION.1).and_then(rgb)?;
    let part = |byte: u8| f64::from(byte) / 255.0;
    Some((part(red), part(green), part(blue)))
}

/// KWin's buttons as a GTK decoration layout, like `icon:minimize,maximize,close`.
pub fn button_layout() -> String {
    let text = read_config(KWIN_FILE);
    let left = value(&text, DECORATION_GROUP, "ButtonsOnLeft").unwrap_or(BREEZE_LEFT);
    let right = value(&text, DECORATION_GROUP, "ButtonsOnRight").unwrap_or(BREEZE_RIGHT);
    format!("{}:{}", gtk_buttons(left), gtk_buttons(right))
}

/// KWin's letters for the buttons GTK has. The rest (on all desktops, help, keep above, shade)
/// have no GTK button, and are left out.
fn gtk_buttons(letters: &str) -> String {
    let name = |letter: char| match letter {
        'M' | 'N' => Some("icon"),
        'I' => Some("minimize"),
        'A' => Some("maximize"),
        'X' => Some("close"),
        _ => None,
    };
    letters.chars().filter_map(name).collect::<Vec<_>>().join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    const BREEZE_DARK: &str = "[General]\nColorScheme=BreezeDark\n\n[Colors:Window]\nBackgroundNormal=32,35,38\n\
                               ForegroundNormal=252,252,252\n\n[Colors:View]\nBackgroundNormal=20,22,24\n";

    #[test]
    fn a_dark_scheme_gives_its_colours_to_a_dark_app_only() {
        let dark = colours_from(BREEZE_DARK, true);
        assert!(dark.contains("@define-color window_bg_color rgb(32, 35, 38);"));
        assert!(dark.contains("@define-color card_bg_color rgb(20, 22, 24);"));
        assert!(!dark.contains("headerbar_bg_color"), "a colour the scheme lacks stays Breeze's");
        assert_eq!(colours_from(BREEZE_DARK, false), "");
        assert_eq!(colours_from("", true), "");
    }

    #[test]
    fn kwins_buttons_become_gtks() {
        assert_eq!(
            format!("{}:{}", gtk_buttons(BREEZE_LEFT), gtk_buttons(BREEZE_RIGHT)),
            "icon:minimize,maximize,close"
        );
        assert_eq!(gtk_buttons("XAI"), "close,maximize,minimize");
        assert_eq!(gtk_buttons("SF"), "");
    }

    #[test]
    fn a_key_is_read_from_its_own_group_only() {
        let text = "[Colors:View]\nBackgroundNormal=1,2,3\n[Colors:Window]\nBackgroundNormal=4,5,6\n";
        assert_eq!(value(text, "Colors:Window", "BackgroundNormal"), Some("4,5,6"));
        assert_eq!(rgb("4,5,6"), Some((4, 5, 6)));
        assert_eq!(rgb("4,5"), None);
    }
}
