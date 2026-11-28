//! The text of each look's stylesheet: its colours first, as the shapes name them, then the shapes,
//! then the app's own widgets and what the look changes of them.

use super::accent::{Rgb, accent_style};
use super::look::Drawn;

const DARK: &str = include_str!("../../data/style/dark.css");
const LIGHT: &str = include_str!("../../data/style/light.css");
const SHAPES: &str = include_str!("../../data/style/shapes.css");
const OWN_WIDGETS: &str = include_str!("../../data/style/own.css");
const DESKTOP_NAMES: &str = include_str!("../../data/style/desktop.css");
const TIDE: &str = include_str!("../../data/style/tide.css");
const TIDE_DARK: &str = include_str!("../../data/style/tide-dark.css");
const TIDE_LIGHT: &str = include_str!("../../data/style/tide-light.css");
const GLASS: &str = include_str!("../../data/style/glass.css");
const KDE: &str = include_str!("../../data/style/kde.css");
const BREEZE_DARK: &str = include_str!("../../data/style/breeze-dark.css");
const BREEZE_LIGHT: &str = include_str!("../../data/style/breeze-light.css");

/// What decides a stylesheet's text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Wanted {
    pub dark: bool,
    pub accent: Option<Rgb>,
    /// The compositor blurs behind the window.
    pub glass: bool,
    /// The user's KDE colours, as `@define-color` lines, when the KDE look has any.
    pub kde_colours: String,
}

pub fn text(drawn: Drawn, wanted: &Wanted) -> String {
    let accent = wanted.accent.map(accent_style).unwrap_or_default();
    match drawn {
        // Only the app's own widgets, in the theme's colours.
        Drawn::System => format!("{DESKTOP_NAMES}\n{accent}\n{OWN_WIDGETS}"),
        Drawn::Tide => {
            let (colours, tide_colours) = if wanted.dark { (DARK, TIDE_DARK) } else { (LIGHT, TIDE_LIGHT) };
            let glass = if wanted.glass { GLASS } else { "" };
            format!("{colours}\n{tide_colours}\n{accent}\n{SHAPES}\n{OWN_WIDGETS}\n{TIDE}\n{glass}")
        }
        Drawn::Kde => {
            let breeze = if wanted.dark { BREEZE_DARK } else { BREEZE_LIGHT };
            let own = &wanted.kde_colours;
            format!("{breeze}\n{own}\n{accent}\n{SHAPES}\n{OWN_WIDGETS}\n{KDE}")
        }
    }
}

/// Every look's text, for the checks that GTK reads all of it.
pub fn every_look() -> Vec<(String, String)> {
    let mut all = Vec::new();
    for drawn in [Drawn::Tide, Drawn::Kde, Drawn::System] {
        for dark in [true, false] {
            for glass in [true, false] {
                let wanted =
                    Wanted { dark, accent: Some((0.2, 0.4, 0.8)), glass, kde_colours: String::new() };
                all.push((format!("{drawn:?}, dark {dark}, glass {glass}"), text(drawn, &wanted)));
            }
        }
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wanted(dark: bool, glass: bool) -> Wanted {
        Wanted { dark, accent: None, glass, kde_colours: String::new() }
    }

    #[test]
    fn the_accent_comes_after_the_palette_and_before_the_shapes() {
        let text = text(Drawn::Tide, &Wanted { accent: Some((0.0, 1.0, 0.0)), ..wanted(true, false) });
        let own = text.find("rgb(0, 255, 0)").unwrap();
        assert!(text.find("#8fb4ff").unwrap() < own, "the palette's accent has to come first to lose");
        assert!(own < text.find("headerbar {").unwrap());
    }

    #[test]
    fn under_the_desktops_theme_only_the_apps_own_widgets_are_styled() {
        let text = text(Drawn::System, &wanted(true, false));
        assert!(text.contains(".hero") && text.contains(".viewer-pill"));
        assert!(!text.contains("headerbar {") && !text.contains("switch {"));
        assert!(!text.contains("window_bg_color #"), "the theme's colours have to stay the theme's");
    }

    #[test]
    fn tide_lets_the_compositor_through_only_where_it_blurs() {
        assert!(text(Drawn::Tide, &wanted(true, true)).contains("alpha(@tide_frame_color"));
        assert!(!text(Drawn::Tide, &wanted(true, false)).contains("alpha(@tide_frame_color"));
        assert!(text(Drawn::Tide, &wanted(false, false)).contains("#e9ebef"));
    }

    #[test]
    fn kdes_own_colours_go_over_breezes() {
        let own = "@define-color window_bg_color rgb(1, 2, 3);\n".to_owned();
        let text = text(Drawn::Kde, &Wanted { kde_colours: own, ..wanted(true, false) });
        assert!(text.find("#202326").unwrap() < text.find("rgb(1, 2, 3)").unwrap());
    }
}
