//! Remote Desktop and SSH in a Windows guest, beside the port forwards that reach them. A change goes
//! to Windows at once while it runs, and otherwise at its next start or its install. A draft's choice
//! stays with the draft, for New Machine to make the machine with.

use super::Shared;
use crate::rows::{explain, info_button};
use adw::prelude::*;
use guesthouse_app_core::machine::Kind;
use guesthouse_app_core::remote_access::{
    APPLY_TASK, Reaches, Service, choose, reaches_by, read_access, ssh_problem,
};
use guesthouse_app_core::tasks::Status;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const GROUP_SHORT: &str = "Through a port forward";
const GROUP_NOTE: &str = "Only a port forward from this PC reaches them, whatever the network. Forward a host port \
                          to the guest port each one says.";
const NOT_CHOSEN: &str = "Windows keeps what it has";

struct Parts {
    shared: Shared,
    status: RefCell<Status>,
    /// Set while a refused change is put back, so that putting it back isn't taken for a change.
    reverting: Cell<bool>,
}

#[derive(Clone)]
pub struct RemoteAccess(Rc<Parts>);

/// What a row says of its service in one line, with no choice made yet if `chosen` is `None`.
fn note(service: Service, chosen: Option<bool>, no_openssh: Option<&str>) -> String {
    match (service, no_openssh, chosen) {
        (Service::Ssh, Some(problem), choice) if choice != Some(true) => problem.to_owned(),
        (_, _, None) => NOT_CHOSEN.to_owned(),
        _ => format!("On guest port {}", service.port()),
    }
}

/// The longer explanation behind a row's info button.
fn explanation(service: Service, account: &str) -> String {
    let port = service.port();
    match service {
        Service::Ssh => format!(
            "OpenSSH Server on guest port {port}. Log in as {account} with the password under Account, or with \
             your public keys from ~/.ssh, which go in too. Until Guesthouse has set it, Windows keeps what it has."
        ),
        Service::RemoteDesktop => format!(
            "Windows' own, on guest port {port}. Sign in as {account} with the password under Account. While \
             someone is connected, the machine's window shows Windows' lock screen. Until Guesthouse has set it, \
             Windows keeps what it has."
        ),
    }
}

impl RemoteAccess {
    pub fn new(shared: &Shared, page: &adw::PreferencesPage) -> Self {
        let this = Self(Rc::new(Parts {
            shared: shared.clone(),
            status: RefCell::default(),
            reverting: Cell::new(false),
        }));
        let machine = shared.machine.borrow().clone();
        if machine.kind != Kind::Windows {
            return this;
        }
        let context = shared.window.context();
        let access = shared
            .draft
            .as_ref()
            .map_or_else(|| read_access(&context, &machine), |draft| *draft.access.borrow());
        let no_openssh = ssh_problem(&context);
        let group = adw::PreferencesGroup::builder().title("Remote Access").description(GROUP_SHORT).build();
        group.set_header_suffix(Some(&info_button(GROUP_NOTE)));
        page.add(&group);
        for service in Service::ALL {
            let chosen = access.get(service);
            let row = adw::SwitchRow::builder()
                .title(service.name())
                .subtitle(note(service, chosen, no_openssh))
                .active(chosen == Some(true))
                .build();
            explain(&row, &explanation(service, machine.account()));
            // A machine with SSH can always lose it, and one without can't get it from this build.
            row.set_sensitive(service != Service::Ssh || no_openssh.is_none() || chosen == Some(true));
            let changing = this.clone();
            row.connect_active_notify(move |row| changing.changed(service, row));
            group.add(&row);
        }
        this
    }

    fn changed(&self, service: Service, row: &adw::SwitchRow) {
        let parts = &self.0;
        if parts.reverting.get() {
            return;
        }
        let shared = &parts.shared;
        let context = shared.window.context();
        let machine = shared.machine.borrow().clone();
        let on = row.is_active();
        if let Some(draft) = &shared.draft {
            draft.access.borrow_mut().set(service, on);
            row.set_subtitle(&note(service, Some(on), ssh_problem(&context)));
            shared.save();
            return;
        }
        if let Err(failure) = choose(&context, &machine, service, on) {
            shared.window.problem(&failure.0);
            parts.reverting.set(true);
            row.set_active(!on);
            parts.reverting.set(false);
            return;
        }
        row.set_subtitle(&note(service, Some(on), ssh_problem(&context)));
        match reaches_by(&machine, &parts.status.borrow()) {
            Reaches::Now => shared.window.launch(APPLY_TASK, &machine, &[], None),
            later => shared.window.toast(later.text(), adw::ToastPriority::Normal),
        }
    }

    pub fn set_status(&self, status: &Status) {
        *self.0.status.borrow_mut() = status.clone();
    }
}
