//! Whether a machine may start with no jail, on a PC that can't give it one.
//! Off unless the user turns it on, and the machine's row says so whenever it runs that way. It's on the
//! Advanced page.

use super::Shared;
use crate::rows::explain;
use adw::prelude::*;

const SHORT: &str = "Only on a PC with no Landlock";
const NOTE: &str = "Only on a PC whose kernel has no Landlock, where QEMU and the daemon then run as you with \
                    nothing around them. Without it, a machine doesn't start there. A guest that broke out would \
                    have your account. The machine's row says so while it runs.";

pub struct Jail;

impl Jail {
    pub fn new(shared: &Shared, group: &adw::PreferencesGroup) -> Self {
        let machine = shared.machine.borrow().clone();
        let row = adw::SwitchRow::builder()
            .title("Start Without a Jail")
            .subtitle(SHORT)
            .active(machine.start_without_jail)
            .build();
        explain(&row, NOTE);
        let saving = shared.clone();
        row.connect_active_notify(move |row| {
            saving.machine.borrow_mut().start_without_jail = row.is_active();
            saving.save();
        });
        group.add(&row);
        Self
    }
}
