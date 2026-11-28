//! The Look preference: which of the app's looks draws it. Automatic picks one by what the desktop
//! says it is, while the app runs; the others are asked for by name.

use super::desktop::{Desktop, Family};
use guesthouse_app_core::settings::Fields;
use serde_json::Value;

/// The preference, and what it holds for each look.
pub const LOOK_OF: &str = "look";
const AUTOMATIC: &str = "automatic";
const TIDE: &str = "tide";
const KDE: &str = "kde";
const SYSTEM: &str = "system";
/// What the preference held before there were more looks than two: the desktop's theme, and the
/// app's own look, which Tide grew out of.
const OLD_DESKTOP: &str = "desktop";
const OLD_OWN: &str = "own";

/// The look the preference asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    Automatic,
    Tide,
    Kde,
    System,
}

/// The look that draws the app, once Automatic has picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drawn {
    /// Compact, for tiling window managers: no bar of its own when tiled, glass where the
    /// compositor blurs, and machines that show their state by how they move.
    Tide,
    /// KDE's colours, spacing and button order.
    Kde,
    /// The desktop's GTK theme, with only the app's own widgets styled.
    System,
}

impl Look {
    /// In the order the preferences list them.
    pub const ALL: [Look; 4] = [Look::Automatic, Look::Tide, Look::Kde, Look::System];

    /// A value from before is read as what it meant: the desktop's theme is System, and the app's
    /// own look is Tide. Nothing, or anything unknown, is Automatic.
    pub fn from_prefs(prefs: &Fields) -> Self {
        match prefs.get(LOOK_OF).and_then(Value::as_str) {
            Some(TIDE | OLD_OWN) => Self::Tide,
            Some(KDE) => Self::Kde,
            Some(SYSTEM | OLD_DESKTOP) => Self::System,
            _ => Self::Automatic,
        }
    }

    /// What the preference holds for this look.
    pub fn value(self) -> &'static str {
        match self {
            Self::Automatic => AUTOMATIC,
            Self::Tide => TIDE,
            Self::Kde => KDE,
            Self::System => SYSTEM,
        }
    }

    /// Its name in the preferences.
    pub fn name(self) -> &'static str {
        match self {
            Self::Automatic => "Automatic",
            Self::Tide => "Tide",
            Self::Kde => "KDE",
            Self::System => "System",
        }
    }

    /// What draws the app on this desktop.
    pub fn drawn_on(self, desktop: &Desktop) -> Drawn {
        match self {
            Self::Tide => Drawn::Tide,
            Self::Kde => Drawn::Kde,
            Self::System => Drawn::System,
            Self::Automatic => match desktop.family() {
                Family::Tiling => Drawn::Tide,
                Family::Kde => Drawn::Kde,
                Family::Other => Drawn::System,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefs(text: &str) -> Fields {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn a_value_from_before_keeps_what_it_meant() {
        assert_eq!(Look::from_prefs(&prefs("{}")), Look::Automatic);
        assert_eq!(Look::from_prefs(&prefs(r#"{"look":""}"#)), Look::Automatic);
        assert_eq!(Look::from_prefs(&prefs(r#"{"look":"desktop"}"#)), Look::System);
        assert_eq!(Look::from_prefs(&prefs(r#"{"look":"own"}"#)), Look::Tide);
        assert_eq!(Look::from_prefs(&prefs(r#"{"look":"later"}"#)), Look::Automatic);
    }

    #[test]
    fn each_look_reads_back_as_itself() {
        for look in Look::ALL {
            let saved = prefs(&format!(r#"{{"look":"{}"}}"#, look.value()));
            assert_eq!(Look::from_prefs(&saved), look);
        }
    }

    #[test]
    fn automatic_follows_the_desktop_and_the_rest_do_not() {
        let sway = Desktop::from_names("sway", false);
        let plasma = Desktop::from_names("KDE", false);
        let gnome = Desktop::from_names("GNOME", false);
        assert_eq!(Look::Automatic.drawn_on(&sway), Drawn::Tide);
        assert_eq!(Look::Automatic.drawn_on(&plasma), Drawn::Kde);
        assert_eq!(Look::Automatic.drawn_on(&gnome), Drawn::System);
        assert_eq!(Look::Tide.drawn_on(&gnome), Drawn::Tide);
        assert_eq!(Look::System.drawn_on(&sway), Drawn::System);
    }
}
