//! Word from the app while its window isn't being looked at: a machine that has started or been
//! installed, one that has stopped without being asked to, and a new version.
//!
//! Each goes to the desktop as a notification, which a click on brings the window back from. With
//! the window in front there's nothing to send, as the window says it all itself.

use crate::window::Window;
use adw::prelude::*;
use gtk::gio;
use guesthouse_app_core::settings::read_prefs;
use guesthouse_app_core::tasks::{State, Status};
use serde_json::Value;

/// The preference that turns these off. They're on unless it says so.
pub const NOTIFICATIONS: &str = "notifications";

pub fn wanted(window: &Window) -> bool {
    read_prefs(&window.context()).get(NOTIFICATIONS) != Some(&Value::Bool(false))
}

/// Sends this unless the window is in front or notifications are off. `about` names what it's
/// about, so a newer one about the same thing takes the place of the older.
pub fn send(window: &Window, about: &str, title: &str, body: &str) {
    let widget = window.widget();
    if (widget.is_visible() && widget.is_active()) || !wanted(window) {
        return;
    }
    let Some(application) = widget.application() else { return };
    let notification = gio::Notification::new(title);
    if !body.is_empty() {
        notification.set_body(Some(body));
    }
    application.send_notification(Some(about), &notification);
}

/// What a machine going from one status to the next is worth saying, if anything.
pub fn of_change(name: &str, before: &Status, now: &Status) -> Option<(String, String)> {
    if !now.error.is_empty() && now.error != before.error {
        return Some((name.to_owned(), now.error.clone()));
    }
    match (before.state, now.state) {
        (State::Installing, State::Running | State::Off) if now.error.is_empty() => {
            Some((format!("{name} is installed"), String::new()))
        }
        (State::Starting, State::Running) => Some((format!("{name} has started"), String::new())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(state: State, error: &str) -> Status {
        Status { state, error: error.to_owned(), ..Status::default() }
    }

    #[test]
    fn a_start_an_install_and_a_new_error_are_worth_saying() {
        let said = |before, now| of_change("W11", &before, &now).map(|(title, _)| title);
        assert_eq!(
            said(status(State::Starting, ""), status(State::Running, "")).as_deref(),
            Some("W11 has started")
        );
        assert_eq!(
            said(status(State::Installing, ""), status(State::Running, "")).as_deref(),
            Some("W11 is installed")
        );
        let stopped = of_change("W11", &status(State::Running, ""), &status(State::Off, "It stopped."));
        assert_eq!(stopped, Some(("W11".to_owned(), "It stopped.".to_owned())));
    }

    #[test]
    fn nothing_new_is_nothing_to_say() {
        assert_eq!(of_change("W11", &status(State::Running, ""), &status(State::Running, "")), None);
        assert_eq!(of_change("W11", &status(State::Running, ""), &status(State::Off, "")), None);
        let same = status(State::Off, "It stopped.");
        assert_eq!(of_change("W11", &same, &same), None);
    }
}
