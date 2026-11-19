//! The app's icon in the desktop's tray, for a desktop that has one: KDE, Xfce, Cinnamon, bars such
//! as Waybar, and GNOME with an extension for it. With the icon there, closing the window leaves
//! the app running behind it.
//!
//! The icon lives on a thread of its own, which is how the library that speaks the tray's protocol
//! works. What's asked of it comes back to GTK's loop over a channel.

use crate::application::APP_ID;
use async_channel::{Receiver, Sender};
use ksni::blocking::TrayMethods;
use std::path::PathBuf;

/// What a click on the icon or in its menu asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asked {
    Show,
    Quit,
}

struct Icon {
    asked: Sender<Asked>,
    /// Where the app wrote its own icons, as a tray can't find one that nothing installed.
    icons: PathBuf,
    /// The machines that are running, by name.
    running: Vec<String>,
}

/// What the icon says of the machines: its tooltip, and the first line of its menu.
fn words(running: &[String]) -> String {
    match running {
        [] => "No machine is running".to_owned(),
        [one] => format!("{one} is running"),
        many => format!("{} are running", many.join(", ")),
    }
}

impl Icon {
    fn ask(&self, what: Asked) {
        // The window's side has gone only when the app is on its way out.
        let _ = self.asked.send_blocking(what);
    }
}

impl ksni::Tray for Icon {
    fn id(&self) -> String {
        APP_ID.to_owned()
    }

    fn title(&self) -> String {
        "Guesthouse".to_owned()
    }

    fn icon_name(&self) -> String {
        format!("{APP_ID}-symbolic")
    }

    fn icon_theme_path(&self) -> String {
        self.icons.to_string_lossy().into_owned()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "Guesthouse".to_owned(),
            description: words(&self.running),
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.ask(Asked::Show);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::{MenuItem, StandardItem};
        let item = |label: &str, what: Asked| {
            MenuItem::Standard(StandardItem {
                label: label.to_owned(),
                activate: Box::new(move |icon: &mut Self| icon.ask(what)),
                ..Default::default()
            })
        };
        // An underscore in a label marks a key, so one in a machine's name is doubled.
        let said = StandardItem {
            label: words(&self.running).replace('_', "__"),
            enabled: false,
            ..Default::default()
        };
        vec![
            MenuItem::Standard(said),
            MenuItem::Separator,
            item("Show Guesthouse", Asked::Show),
            item("Quit", Asked::Quit),
        ]
    }
}

pub struct Tray(ksni::blocking::Handle<Icon>);

impl Tray {
    /// Puts the icon in the tray, if the desktop has one. A run that a login started waits for the
    /// tray to come up, as the app can be there first.
    pub fn start(icons: PathBuf, at_login: bool) -> Option<(Self, Receiver<Asked>)> {
        let (asked, asks) = async_channel::unbounded();
        let icon = Icon { asked, icons, running: Vec::new() };
        let handle = icon.assume_sni_available(at_login).spawn().ok()?;
        Some((Self(handle), asks))
    }

    /// Says which machines are running, for the tooltip and the menu.
    pub fn set_running(&self, running: Vec<String>) {
        self.0.update(move |icon| {
            if icon.running != running {
                icon.running = running;
            }
        });
    }

    pub fn stop(self) {
        self.0.shutdown().wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_says_which_machines_run() {
        assert_eq!(words(&[]), "No machine is running");
        assert_eq!(words(&["Windows 11".to_owned()]), "Windows 11 is running");
        assert_eq!(words(&["Windows 11".to_owned(), "Ubuntu".to_owned()]), "Windows 11, Ubuntu are running");
    }
}
