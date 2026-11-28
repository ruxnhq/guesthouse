//! The network's mode, the interface a NAT machine is held to, and the bridge. Port forwards have their
//! own group. passt and the MAC address are on the Advanced page.

use super::forwards::Forwards;
use super::{Shared, is_off};
use crate::common::in_background;
use crate::rows::{choice_row, explain, info_button, row_button};
use adw::prelude::*;
use guesthouse_app_core::bridge;
use guesthouse_app_core::library::unused_mac;
use guesthouse_app_core::machine::Kind;
use guesthouse_app_core::network::{self, DEFAULT_MAC, InUse, Mode};
use guesthouse_app_core::network_settings::{
    network_in_use, set_bridge, set_dns, set_mac, set_network, set_only_through,
};
use guesthouse_app_core::only_through::interface_names;
use guesthouse_app_core::tasks::{State, Status};
use guesthouse_app_core::vm_args::passt_installed;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

struct Parts {
    shared: Shared,
    group: adw::PreferencesGroup,
    /// passt and the MAC address, on the Advanced page.
    advanced: adw::PreferencesGroup,
    mode: adw::ComboRow,
    /// The whole of what the mode means, as the row says only its first sentence.
    mode_info: gtk::MenuButton,
    passt: adw::SwitchRow,
    through: adw::ComboRow,
    /// The names the rows of `through` stand for, after "Any".
    through_names: Vec<String>,
    dns: adw::EntryRow,
    bridge: adw::EntryRow,
    bridge_state: adw::ActionRow,
    bridge_help: gtk::Button,
    mac: adw::EntryRow,
    forwards: Forwards,
    in_use: Cell<Option<InUse>>,
    mac_problem: RefCell<Option<String>>,
    off: Cell<bool>,
    /// The page is changing a row to match the machine, which isn't the user's change.
    syncing: Cell<bool>,
}

/// The row that means no interface in particular.
const ANY_INTERFACE: &str = "Any";
const VM_MUST_BE_OFF: &str = "The VM must be off to change these.";
const THROUGH_NOTE: &str = "All of the guest's traffic goes out through this interface, such as a VPN's, and \
                            nothing else. If it goes away, the guest has no network until the machine is started \
                            again.";
const DNS_NOTE: &str = "Leave it empty, and the machine finds one at each start.";
const PASST_TITLE: &str = "Newer NAT Engine";
const PASST_NOTE: &str = "Runs the guest's NAT network through passt instead of QEMU's own. It needs no root, and \
                          it's what holds a machine to one interface.";

/// The host's interfaces, and the saved one if the host doesn't have it now: a VPN that isn't connected.
fn interfaces_with(saved: &str) -> Vec<String> {
    let mut names = interface_names();
    if !saved.is_empty() && !names.iter().any(|name| name == saved) {
        names.push(saved.to_owned());
    }
    names
}

#[derive(Clone)]
pub struct Network(Rc<Parts>);

impl Network {
    /// passt and the MAC address go in `advanced`.
    pub fn new(shared: &Shared, page: &adw::PreferencesPage, advanced: &adw::PreferencesGroup) -> Self {
        let machine = shared.machine.borrow().clone();
        // Ubuntu's network is fixed, with its SSH port forwarded.
        let shown = machine.kind != Kind::Linux;
        let group = adw::PreferencesGroup::builder().title("Network").visible(shown).build();
        page.add(&group);
        advanced.set_visible(shown);
        let names: Vec<&str> = Mode::ALL.iter().map(|mode| mode.name()).collect();
        let mode = choice_row("Mode", &names);
        mode.set_selected(
            Mode::ALL.iter().position(|found| *found == machine.network_mode).unwrap_or(0) as u32
        );
        let mode_info = info_button("");
        mode.add_suffix(&mode_info);
        group.add(&mode);
        let passt = adw::SwitchRow::builder().title(PASST_TITLE).active(machine.passt).build();
        explain(&passt, PASST_NOTE);
        let through_names = interfaces_with(&machine.only_through);
        let shown: Vec<&str> =
            std::iter::once(ANY_INTERFACE).chain(through_names.iter().map(String::as_str)).collect();
        let through = choice_row("Send Traffic Only Through", &shown);
        through.set_subtitle("Such as a VPN's interface");
        explain(&through, THROUGH_NOTE);
        let chosen = through_names.iter().position(|name| *name == machine.only_through);
        through.set_selected(chosen.map_or(0, |at| at as u32 + 1));
        group.add(&through);
        let dns =
            adw::EntryRow::builder().title("DNS Server").text(&machine.dns).show_apply_button(true).build();
        dns.add_suffix(&info_button(DNS_NOTE));
        group.add(&dns);
        let bridge =
            adw::EntryRow::builder().title("Bridge").text(&machine.bridge).show_apply_button(true).build();
        group.add(&bridge);
        let bridge_state = adw::ActionRow::builder().title("Bridge Status").subtitle_lines(0).build();
        let bridge_help = row_button("Show What to Do");
        bridge_state.add_suffix(&bridge_help);
        group.add(&bridge_state);
        let text = if machine.mac.is_empty() { DEFAULT_MAC } else { &machine.mac };
        let mac = adw::EntryRow::builder().title("MAC Address").text(text).show_apply_button(true).build();
        let new_mac = row_button("New");
        new_mac.set_tooltip_text(Some("Make a New Address"));
        mac.add_suffix(&new_mac);
        advanced.add(&passt);
        advanced.add(&mac);
        let this = Self(Rc::new(Parts {
            shared: shared.clone(),
            group,
            advanced: advanced.clone(),
            mode,
            mode_info,
            passt,
            through,
            through_names,
            dns,
            bridge,
            bridge_state,
            bridge_help,
            mac,
            forwards: Forwards::new(shared, page),
            in_use: Cell::new(None),
            mac_problem: RefCell::default(),
            off: Cell::new(true),
            syncing: Cell::new(false),
        }));
        this.connect(&new_mac);
        this.show();
        this
    }

    fn connect(&self, new_mac: &gtk::Button) {
        let parts = &self.0;
        let this = self.clone();
        parts.mode.connect_selected_notify(move |row| this.on_mode(Mode::ALL[row.selected() as usize]));
        let this = self.clone();
        parts.passt.connect_active_notify(move |row| {
            if this.0.syncing.get() {
                return;
            }
            this.0.shared.machine.borrow_mut().passt = row.is_active();
            this.0.shared.save();
            this.show();
        });
        let this = self.clone();
        parts.through.connect_selected_notify(move |row| this.on_through(row.selected() as usize));
        let this = self.clone();
        parts.dns.connect_apply(move |row| {
            let shared = &this.0.shared;
            shared.change(|context, machine| set_dns(context, machine, &row.text()));
            this.show();
        });
        let this = self.clone();
        parts.bridge.connect_apply(move |row| {
            let shared = &this.0.shared;
            shared.change(|context, machine| set_bridge(context, machine, &row.text()));
            this.show();
        });
        let this = self.clone();
        parts.mac.connect_apply(move |row| this.on_mac(row));
        let this = self.clone();
        parts.mac.connect_changed(move |row| {
            row.remove_css_class("error");
            *this.0.mac_problem.borrow_mut() = None;
            this.show();
        });
        let this = self.clone();
        new_mac.connect_clicked(move |_| {
            let mac = &this.0.mac;
            mac.set_text(&unused_mac(&this.0.shared.window.context()));
            this.on_mac(mac);
        });
        let this = self.clone();
        parts.bridge_help.connect_clicked(move |_| this.on_bridge_help());
    }

    fn on_mode(&self, mode: Mode) {
        let shared = &self.0.shared;
        // The address is found among the machines of the store, which a draft's scratch folder hasn't.
        if mode == Mode::Bridged && shared.machine.borrow().mac.is_empty() {
            let mac = unused_mac(&shared.window.context());
            shared.machine.borrow_mut().mac = mac;
        }
        shared.change(|context, machine| set_network(context, machine, mode));
        let mac = shared.machine.borrow().mac.clone();
        self.0.mac.set_text(if mac.is_empty() { DEFAULT_MAC } else { &mac });
        self.show();
    }

    fn on_through(&self, row: usize) {
        let shared = &self.0.shared;
        let name =
            row.checked_sub(1).and_then(|at| self.0.through_names.get(at)).cloned().unwrap_or_default();
        shared.change(|context, machine| set_only_through(context, machine, &name));
        self.show();
    }

    fn on_mac(&self, row: &adw::EntryRow) {
        let shared = &self.0.shared;
        let text = row.text();
        let refused = shared.attempt(|context, machine| set_mac(context, machine, &text));
        match refused {
            Err(failure) => {
                row.add_css_class("error");
                // A toast would cover this row in a narrow window.
                *self.0.mac_problem.borrow_mut() = Some(failure.0);
                self.show();
            }
            Ok(()) => {
                let mac = shared.machine.borrow().mac.clone();
                row.set_text(&mac);
                if shared.is_draft() {
                    shared.save();
                } else {
                    shared.window.refresh();
                }
            }
        }
    }

    fn on_bridge_help(&self) {
        let shared = &self.0.shared;
        let context = shared.window.context();
        let host = &context.bridge;
        let name = shared.machine.borrow().bridge.clone();
        let interface = host.default_interface();
        let wireless = interface.as_deref().is_some_and(|found| host.is_wireless(found));
        let body = bridge::advice(&name, interface.as_deref(), wireless, host.filtered());
        let exists = host.exists(&name);
        let ready_to_run = exists || (interface.is_some() && !wireless);
        let commands = ready_to_run.then(|| host.commands(&name, interface.as_deref(), exists));
        show_root_step(
            shared,
            &format!("{body} This page sees it as soon as it is done."),
            commands.as_deref(),
        );
    }

    fn show(&self) {
        let parts = &self.0;
        let machine = parts.shared.machine.borrow().clone();
        let context = parts.shared.window.context();
        let off = parts.off.get();
        let installed = passt_installed();
        let bridged = machine.network_mode == Mode::Bridged;
        let mode = network::describe(machine.network_mode, machine.passt, installed, &machine.only_through);
        super::first_sentence_and_info(&parts.mode, &parts.mode_info, &mode);
        let notes = if off {
            String::new()
        } else {
            let running = network::in_use_text(parts.in_use.get());
            format!("{VM_MUST_BE_OFF} {running}").trim().to_owned()
        };
        parts.group.set_description(Some(&notes).filter(|text| !text.is_empty()).map(String::as_str));
        let advanced = if off { parts.mac_problem.borrow().clone() } else { Some(VM_MUST_BE_OFF.to_owned()) };
        parts.advanced.set_description(advanced.as_deref());
        parts.mode.set_sensitive(off);
        parts.bridge.set_sensitive(off);
        parts.mac.set_sensitive(off);
        let nat = machine.network_mode == Mode::Nat;
        let held = !machine.only_through.is_empty();
        parts.passt.set_visible(nat);
        // Held to an interface, a machine always has passt, which nothing else can bind to one.
        parts.passt.set_sensitive(off && installed && !held);
        parts.passt.set_subtitle(if held {
            "Always on while traffic goes only through one interface"
        } else if installed {
            "Needs no root"
        } else {
            "passt is not installed"
        });
        parts.syncing.set(true);
        parts.passt.set_active(machine.passt || held);
        parts.syncing.set(false);
        parts.through.set_visible(nat);
        parts.through.set_sensitive(off);
        parts.dns.set_visible(nat && held);
        parts.dns.set_sensitive(off);
        parts.bridge.set_visible(bridged);
        parts.bridge_state.set_visible(bridged);
        if bridged {
            let found = context.bridge.problems(&machine.bridge);
            let ready = format!("{} is ready.", machine.bridge);
            let text = if found.is_empty() { ready } else { found.join(" ") };
            parts.bridge_state.set_subtitle(&gtk::glib::markup_escape_text(&text));
            parts.bridge_help.set_visible(!found.is_empty());
        }
        parts.forwards.set_off(off);
    }

    fn read_in_use(&self) {
        let context = self.0.shared.window.context();
        let machine = self.0.shared.machine.borrow().clone();
        let reading = context.clone();
        let this = self.clone();
        in_background(
            &context,
            move || Ok(network_in_use(&reading, &machine)),
            move |result| match result {
                Ok(found) => {
                    this.0.in_use.set(found);
                    this.show();
                }
                Err(text) => this.0.shared.window.problem(&text),
            },
        );
    }

    pub fn set_status(&self, status: &Status) {
        self.0.off.set(is_off(status));
        if status.state == State::Running && self.0.in_use.get().is_none() {
            self.read_in_use();
        } else if status.state != State::Running {
            self.0.in_use.set(None);
        }
        self.show();
    }
}

/// The one step only the user can do, with commands that can be copied.
pub fn show_root_step(shared: &Shared, body: &str, commands: Option<&str>) {
    let dialog = adw::AlertDialog::new(Some("One Step Needs Root"), Some(body));
    if let Some(commands) = commands {
        dialog.set_extra_child(Some(&super::command_card(commands)));
    }
    dialog.add_response("close", "Close");
    dialog.present(Some(&shared.dialog));
}
