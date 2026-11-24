//! The account the install made in the guest, and where its password is kept. The name was the user's
//! to choose before the install, and the password file is theirs to read. A draft's account is New
//! Machine's to set.

use super::Shared;
use crate::rows::explain;
use adw::prelude::*;
use guesthouse_app_core::machine::Kind;

const WINDOWS_NOTE: &str =
    "Windows signs in by itself. The password is for permission prompts, Remote Desktop and SSH.";
const LINUX_NOTE: &str = "For a console login if the desktop won't start. The app gets in with its own key.";

pub struct Account;

impl Account {
    pub fn new(shared: &Shared, group: &adw::PreferencesGroup) -> Self {
        let machine = shared.machine.borrow().clone();
        let note = match machine.kind {
            Kind::Windows => WINDOWS_NOTE,
            Kind::Linux => LINUX_NOTE,
            Kind::Plain => return Self,
        };
        if shared.is_draft() {
            return Self;
        }
        let file = machine.password_file(&shared.window.context());
        // The file's path stays in the subtitle, where it can be copied.
        let row = adw::ActionRow::builder()
            .title("Account")
            .subtitle(format!("{}, with its password in {}", machine.account(), file.display()))
            .use_markup(false)
            .subtitle_selectable(true)
            .build();
        explain(&row, note);
        group.add(&row);
        Self
    }
}
