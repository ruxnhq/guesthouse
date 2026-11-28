//! What the desktop says it is, as the app finds it when it starts: the names in
//! `XDG_CURRENT_DESKTOP` and the session's, and the IPC socket a tiling compositor leaves in the
//! environment of what it starts.

use std::env;

/// The variables a desktop names itself in. `XDG_CURRENT_DESKTOP` may hold several, split by `:`.
const NAME_VARIABLES: [&str; 3] = ["XDG_CURRENT_DESKTOP", "XDG_SESSION_DESKTOP", "DESKTOP_SESSION"];
/// The sockets tiling compositors tell their clients about.
const TILING_SOCKETS: [&str; 4] = ["HYPRLAND_INSTANCE_SIGNATURE", "SWAYSOCK", "I3SOCK", "NIRI_SOCKET"];
/// Compositors and window managers that tile, by the name they give, in lower case.
const TILING_NAMES: [&str; 17] = [
    "hyprland",
    "sway",
    "swayfx",
    "i3",
    "niri",
    "river",
    "dwl",
    "qtile",
    "bspwm",
    "herbstluftwm",
    "xmonad",
    "dwm",
    "awesome",
    "leftwm",
    "spectrwm",
    "mango",
    "jay",
];
/// What KDE Plasma calls itself.
const KDE_NAMES: [&str; 2] = ["kde", "plasma"];

/// The kind of desktop, as far as the look goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    /// A compositor that tiles: windows are placed, moved and closed by it, from the keyboard.
    Tiling,
    Kde,
    /// GNOME and every other desktop that draws floating windows.
    Other,
}

#[derive(Clone, Debug, Default)]
pub struct Desktop {
    /// The names the desktop gave, in lower case.
    names: Vec<String>,
    /// A tiling compositor's socket is in the environment.
    tiling_socket: bool,
}

impl Desktop {
    /// The desktop this process runs on.
    pub fn current() -> Self {
        let names: Vec<String> = NAME_VARIABLES.iter().filter_map(|name| env::var(name).ok()).collect();
        let socket =
            TILING_SOCKETS.iter().any(|name| env::var_os(name).is_some_and(|value| !value.is_empty()));
        Self::from_names(&names.join(":"), socket)
    }

    /// From the names, split by `:`, and whether a tiling compositor's socket was there.
    pub fn from_names(names: &str, tiling_socket: bool) -> Self {
        let names = names
            .split([':', ';'])
            .map(|name| name.trim().to_ascii_lowercase())
            .filter(|name| !name.is_empty())
            .collect();
        Self { names, tiling_socket }
    }

    fn named(&self, any_of: &[&str]) -> bool {
        self.names.iter().any(|name| any_of.contains(&name.as_str()))
    }

    pub fn family(&self) -> Family {
        if self.named(&TILING_NAMES) {
            Family::Tiling
        } else if self.named(&KDE_NAMES) {
            Family::Kde
        } else if self.tiling_socket {
            Family::Tiling
        } else {
            Family::Other
        }
    }

    /// Whether the compositor places and closes windows itself, so a window needs no bar to be
    /// moved or closed by.
    pub fn tiles(&self) -> bool {
        self.family() == Family::Tiling
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tiling_compositor_is_known_by_its_name_or_its_socket() {
        assert_eq!(Desktop::from_names("Hyprland", false).family(), Family::Tiling);
        assert_eq!(Desktop::from_names("sway:wlroots", false).family(), Family::Tiling);
        assert_eq!(Desktop::from_names("niri", false).family(), Family::Tiling);
        assert_eq!(Desktop::from_names("", true).family(), Family::Tiling);
    }

    #[test]
    fn plasma_and_gnome_are_not_tiling() {
        assert_eq!(Desktop::from_names("KDE", false).family(), Family::Kde);
        assert_eq!(Desktop::from_names("ubuntu:GNOME", false).family(), Family::Other);
        assert_eq!(Desktop::from_names("", false).family(), Family::Other);
        assert!(!Desktop::from_names("GNOME", false).tiles());
    }

    #[test]
    fn a_named_desktop_wins_over_a_socket_left_in_the_environment() {
        // Sway started from a Plasma session's terminal, say, leaves the name KDE.
        assert_eq!(Desktop::from_names("KDE", true).family(), Family::Kde);
    }
}
