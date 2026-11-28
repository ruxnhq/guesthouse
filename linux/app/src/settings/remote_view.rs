//! Remote view: a Windows machine's screen for other devices on the network, through remoteman. The group
//! has the machine's switch, the pairing of a new device, the devices that are paired and what each may do.
//!
//! The words are `guesthouse_remote_view_core::wording`'s, the same on every host. This file draws what a
//! [`ShareInfo`] says. The page asks the core through [`Backend`], one method for each thing it can be
//! told, always on a thread of its own, so nothing here waits on the guest. Answers are the state as it
//! is afterwards, and the page looks again every two seconds while it shows. The countdown of an open
//! pairing runs on the page's own clock, from the end time the core gave.

use adw::prelude::*;
use gtk::{gio, glib};
use guesthouse_app_core::machine::Machine;
use guesthouse_app_core::remote_view as rv;
use guesthouse_remote_view_core::share::{DeviceInfo, PAIRING_SECONDS, PERMISSIONS, ShareInfo, ShareState};
use guesthouse_remote_view_core::wording::{self, headline, pairing_seconds_left, permission_label};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub mod fixture;

/// How many ticks of the page's clock lie between two looks at the core.
const LOOK_EVERY: u32 = 2;
/// A list of this many devices or fewer starts with each device open.
const OPEN_UP_TO: usize = 3;
const NOTE_MARGIN: i32 = 6;
const WENT_WRONG: &str = "Something went wrong.";

/// What the page asks of the core. Each call may take as long as the guest does, so the page makes it on
/// a thread. `Err` is a sentence for the person.
pub trait Backend: Send + Sync + 'static {
    /// The machine's switch: whether viewing from other devices is allowed.
    fn allowed(&self) -> bool;
    /// Where sharing stands now.
    fn info(&self) -> Result<ShareInfo, String>;
    /// Turns the switch, and opens pairing for `pairing_seconds` (the default when `None`) if `pairing`.
    fn share(&self, on: bool, pairing: bool, pairing_seconds: Option<u64>) -> Result<ShareInfo, String>;
    fn pair_close(&self) -> Result<ShareInfo, String>;
    fn pair_accept(&self) -> Result<ShareInfo, String>;
    fn pair_decline(&self) -> Result<ShareInfo, String>;
    /// Allows or refuses one of `wording`'s [`PERMISSIONS`] for a device, by its fingerprint.
    fn device_allow(&self, fingerprint: &str, what: &str, on: bool) -> Result<ShareInfo, String>;
    /// Takes a device off the trust list.
    fn device_revoke(&self, fingerprint: &str) -> Result<ShareInfo, String>;
    /// Disconnects a device that is connected, and leaves it trusted.
    fn device_drop(&self, fingerprint: &str) -> Result<ShareInfo, String>;
    /// This PC's address on its network, which a device types in with the port. Cheap: no I/O waits.
    fn address(&self) -> Option<String> {
        None
    }
}

/// The real backend: a thin adapter over `guesthouse_app_core::remote_view::{allowed, info, share,
/// pair_close, pair_accept, pair_decline, device_allow, device_revoke, device_drop}`, each called with
/// the machine as it is saved now.
pub struct CoreBackend {
    context: Arc<guesthouse_app_core::context::Context>,
    machine: String,
}

impl CoreBackend {
    pub fn new(context: Arc<guesthouse_app_core::context::Context>, machine: &str) -> Self {
        Self { context, machine: machine.to_owned() }
    }

    /// Runs `call` on the machine as it is saved now.
    fn with<T>(
        &self,
        call: impl FnOnce(&guesthouse_app_core::context::Context, &Machine) -> guesthouse_app_core::Result<T>,
    ) -> Result<T, String> {
        let machine = guesthouse_app_core::machine::load(&self.context, &self.machine).map_err(|f| f.0)?;
        call(&self.context, &machine).map_err(|f| f.0)
    }
}

impl Backend for CoreBackend {
    fn allowed(&self) -> bool {
        self.with(|context, machine| Ok(rv::allowed(context, machine))).unwrap_or(false)
    }
    fn info(&self) -> Result<ShareInfo, String> {
        self.with(|context, machine| Ok(rv::info(context, machine)))
    }
    fn share(&self, on: bool, pairing: bool, seconds: Option<u64>) -> Result<ShareInfo, String> {
        self.with(|context, machine| rv::share(context, machine, on, pairing, seconds))
    }
    fn pair_close(&self) -> Result<ShareInfo, String> {
        self.with(rv::pair_close)
    }
    fn pair_accept(&self) -> Result<ShareInfo, String> {
        self.with(rv::pair_accept)
    }
    fn pair_decline(&self) -> Result<ShareInfo, String> {
        self.with(rv::pair_decline)
    }
    fn device_allow(&self, fingerprint: &str, what: &str, on: bool) -> Result<ShareInfo, String> {
        self.with(|context, machine| rv::device_allow(context, machine, fingerprint, what, on))
    }
    fn device_revoke(&self, fingerprint: &str) -> Result<ShareInfo, String> {
        self.with(|context, machine| rv::device_revoke(context, machine, fingerprint))
    }
    /// remoteman's host has one viewer at a time, so disconnecting drops whoever is connected.
    fn device_drop(&self, _fingerprint: &str) -> Result<ShareInfo, String> {
        self.with(rv::device_drop)
    }
    fn address(&self) -> Option<String> {
        rv::lan_address().map(|address| address.to_string())
    }
}

/// The group's title, and how it says it is a preview, as the other previews' pages do.
const TITLE: &str = "Remote View";
const PREVIEW: &str = "A preview: so far it has only run on my PC, with a Windows machine on an NVIDIA card.";

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| u64::try_from(since.as_millis()).unwrap_or(0))
}

/// "1:45" for a time left, which is what a countdown reads best as.
fn clock(seconds: u64) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// What the page drew from, to leave it alone while nothing in it changed.
#[derive(Clone, PartialEq)]
struct Drawn {
    info: ShareInfo,
    allowed: bool,
    running: bool,
}

#[derive(Default)]
struct Live {
    /// The row that counts down an open pairing, and the one that counts down a device's wait.
    pairing: Option<adw::ActionRow>,
    pending: Option<(adw::ActionRow, u64)>,
}

struct Parts {
    backend: Arc<dyn Backend>,
    problem: Box<dyn Fn(&str)>,
    switch: adw::SwitchRow,
    connect: adw::PreferencesGroup,
    pairing: adw::PreferencesGroup,
    devices: adw::PreferencesGroup,
    info: RefCell<ShareInfo>,
    allowed: Cell<bool>,
    running: Cell<bool>,
    drawn: RefCell<Option<Drawn>>,
    /// What each group holds that is drawn again when the state changes.
    shown: RefCell<Vec<(adw::PreferencesGroup, gtk::Widget)>>,
    live: RefCell<Live>,
    /// When the device that waits was first seen, to count its seconds down from.
    pending_seen: RefCell<Option<(String, Instant)>>,
    /// Devices a person opened or closed, over how many there are.
    expanded: RefCell<HashMap<String, bool>>,
    /// Counts what was asked, so a look that was started before it is let go.
    epoch: Cell<u64>,
    looking: Cell<bool>,
    ticks: Cell<u32>,
    /// The switch is being set from the state, not by a click.
    setting: Cell<bool>,
}

#[derive(Clone)]
pub struct RemoteView(Rc<Parts>);

impl RemoteView {
    /// Adds the groups to `page`. `problem` says a failure to the person. The caller decides who has the
    /// page: Windows machines only.
    pub fn new(
        page: &adw::PreferencesPage,
        backend: Arc<dyn Backend>,
        problem: impl Fn(&str) + 'static,
    ) -> Self {
        let card = adw::PreferencesGroup::builder()
            .title(TITLE)
            .description(format!("{PREVIEW} {}", wording::NETWORK_WARNING))
            .build();
        let switch = adw::SwitchRow::builder().title(wording::ALLOW_TITLE).build();
        card.add(&switch);
        let group = |title: &str| adw::PreferencesGroup::builder().title(title).visible(false).build();
        let connect = group(wording::CONNECT_TITLE);
        let pairing = group(wording::PAIRING_TITLE);
        let devices = group(wording::DEVICES_TITLE);
        for added in [&card, &connect, &pairing, &devices] {
            page.add(added);
        }
        let this = Self(Rc::new(Parts {
            backend,
            problem: Box::new(problem),
            switch,
            connect,
            pairing,
            devices,
            info: RefCell::default(),
            allowed: Cell::new(false),
            running: Cell::new(false),
            drawn: RefCell::default(),
            shown: RefCell::default(),
            live: RefCell::default(),
            pending_seen: RefCell::default(),
            expanded: RefCell::default(),
            epoch: Cell::new(0),
            looking: Cell::new(false),
            ticks: Cell::new(0),
            setting: Cell::new(false),
        }));
        let changing = Rc::downgrade(&this.0);
        this.0.switch.connect_active_notify(move |row| {
            if let Some(parts) = changing.upgrade() {
                Self(parts).switched(row.is_active());
            }
        });
        this.redraw();
        this.look();
        let weak = Rc::downgrade(&this.0);
        glib::timeout_add_seconds_local(1, move || Self::tick(&weak));
        this
    }

    /// Whether the machine is up, since the screen is only shared then.
    pub fn set_running(&self, running: bool) {
        if self.0.running.replace(running) != running {
            self.redraw();
        }
    }

    fn tick(weak: &Weak<Parts>) -> glib::ControlFlow {
        let Some(parts) = weak.upgrade() else { return glib::ControlFlow::Break };
        let this = Self(parts);
        let ticks = this.0.ticks.get() + 1;
        this.0.ticks.set(ticks % LOOK_EVERY);
        let ended = this.count_down();
        if (ticks.is_multiple_of(LOOK_EVERY) || ended) && this.0.switch.is_mapped() {
            this.look();
        }
        glib::ControlFlow::Continue
    }

    /// Writes the seconds left into the rows that count. True when a time just ran out, which is the
    /// core's to close: it is asked at once.
    fn count_down(&self) -> bool {
        let live = self.0.live.borrow();
        let info = self.0.info.borrow();
        let mut ended = false;
        if let Some(row) = &live.pairing {
            match pairing_seconds_left(&info, now_ms()) {
                Some(left) => row.set_subtitle(&format!("Closes in {}", clock(left))),
                None => ended = true,
            }
        }
        if let Some((row, seconds)) = &live.pending {
            let seen =
                self.0.pending_seen.borrow().as_ref().map_or(0, |(_, since)| since.elapsed().as_secs());
            row.set_subtitle(&pending_text(&info, seconds.saturating_sub(seen)));
        }
        ended
    }

    fn adopt(&self, info: ShareInfo) {
        let parts = &self.0;
        let now_pending =
            info.pending.as_ref().map(|pending| format!("{}/{}", pending.fingerprint, pending.code));
        let mut seen = parts.pending_seen.borrow_mut();
        if seen.as_ref().map(|(key, _)| key) != now_pending.as_ref() {
            *seen = now_pending.map(|key| (key, Instant::now()));
        }
        drop(seen);
        *parts.info.borrow_mut() = info;
        self.redraw();
    }

    /// Asks the core how things stand, unless that is being done.
    fn look(&self) {
        let parts = &self.0;
        if parts.looking.replace(true) {
            return;
        }
        let epoch = parts.epoch.get();
        let backend = parts.backend.clone();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let answer = gio::spawn_blocking(move || (backend.allowed(), backend.info())).await;
            let parts = &this.0;
            parts.looking.set(false);
            // Something was asked since, and its answer is newer.
            if parts.epoch.get() != epoch {
                return;
            }
            match answer {
                Ok((allowed, Ok(info))) => {
                    parts.allowed.set(allowed);
                    this.adopt(info);
                }
                Ok((_, Err(failure))) => {
                    let mut info = parts.info.borrow().clone();
                    info.state = ShareState::Failed;
                    info.reason = Some(failure);
                    this.adopt(info);
                }
                Err(_) => (parts.problem)(WENT_WRONG),
            }
        });
    }

    /// Tells the core something, and shows the state it answers with. `on_failure` runs after the person
    /// has been told, to put right what was drawn ahead of the answer.
    fn ask(
        &self,
        work: impl FnOnce(&dyn Backend) -> Result<ShareInfo, String> + Send + 'static,
        on_failure: impl FnOnce(&Self) + 'static,
    ) {
        let parts = &self.0;
        parts.epoch.set(parts.epoch.get() + 1);
        let backend = parts.backend.clone();
        let this = self.clone();
        glib::spawn_future_local(async move {
            match gio::spawn_blocking(move || work(&*backend)).await {
                Ok(Ok(info)) => this.adopt(info),
                Ok(Err(failure)) => {
                    (this.0.problem)(&failure);
                    on_failure(&this);
                    this.look();
                }
                Err(_) => {
                    (this.0.problem)(WENT_WRONG);
                    on_failure(&this);
                    this.look();
                }
            }
        });
    }

    /// Draws again from scratch, even if nothing changed.
    fn redraw_anyway(&self) {
        *self.0.drawn.borrow_mut() = None;
        self.redraw();
    }

    fn switched(&self, on: bool) {
        let parts = &self.0;
        if parts.setting.get() {
            return;
        }
        parts.allowed.set(on);
        self.redraw();
        self.ask(
            move |core| core.share(on, false, None),
            move |this| {
                this.0.allowed.set(!on);
                this.redraw_anyway();
            },
        );
    }

    fn open_pairing(&self) {
        self.ask(|core| core.share(true, true, Some(PAIRING_SECONDS)), |_| {});
    }

    fn redraw(&self) {
        let parts = &self.0;
        let now = Drawn {
            info: parts.info.borrow().clone(),
            allowed: parts.allowed.get(),
            running: parts.running.get(),
        };
        if parts.drawn.borrow().as_ref() == Some(&now) {
            return;
        }
        for (group, widget) in parts.shown.borrow_mut().drain(..) {
            group.remove(&widget);
        }
        *parts.live.borrow_mut() = Live::default();
        let info = &now.info;
        parts.setting.set(true);
        parts.switch.set_active(now.allowed);
        parts.setting.set(false);
        parts.switch.set_subtitle(&glib::markup_escape_text(&headline(now.allowed, now.running, info)));
        parts.connect.set_visible(false);
        // Nothing can be paired while the screen is not shared.
        let pairable = now.allowed && info.state != ShareState::Failed;
        parts.pairing.set_visible(pairable);
        parts.devices.set_visible(now.allowed);
        if now.allowed {
            self.draw_connect(info, now.running);
            if pairable {
                self.draw_pairing(info);
            }
            self.draw_devices(info);
        }
        *parts.drawn.borrow_mut() = Some(now);
        self.count_down();
    }

    fn put(&self, group: &adw::PreferencesGroup, widget: &impl IsA<gtk::Widget>) {
        group.add(widget);
        self.0.shown.borrow_mut().push((group.clone(), widget.clone().upcast()));
    }

    fn note(&self, group: &adw::PreferencesGroup, text: &str) {
        let label = gtk::Label::builder().label(text).wrap(true).xalign(0.0).margin_top(NOTE_MARGIN).build();
        label.add_css_class("dim-label");
        self.put(group, &label);
    }

    fn draw_connect(&self, info: &ShareInfo, running: bool) {
        let group = &self.0.connect;
        if !running || info.state != ShareState::Listening {
            return;
        }
        group.set_visible(true);
        if let Some(port) = info.port {
            let row = match self.0.backend.address() {
                Some(address) => {
                    let text = format!("{address}:{port}");
                    let host = guesthouse_remote_view_core::announce::host_name(&info.machine);
                    let row = adw::ActionRow::builder()
                        .title("Address")
                        .subtitle(format!("{text}\nor {}:{port}", host.trim_end_matches('.')))
                        .subtitle_selectable(true)
                        .build();
                    let copy = button(wording::COPY_ADDRESS);
                    copy.connect_clicked(move |button| button.clipboard().set_text(&text));
                    row.add_suffix(&copy);
                    row
                }
                None => adw::ActionRow::builder()
                    .title("Port")
                    .subtitle(port.to_string())
                    .subtitle_selectable(true)
                    .build(),
            };
            self.put(group, &row);
        }
        if let Some(fingerprint) = &info.fingerprint {
            let row = adw::ActionRow::builder()
                .title("Fingerprint")
                .subtitle(glib::markup_escape_text(&wording::short_fingerprint(fingerprint)))
                .subtitle_selectable(true)
                .tooltip_text(fingerprint)
                .build();
            self.put(group, &row);
        }
        self.note(group, wording::PAIRED_HINT);
    }

    fn draw_pairing(&self, info: &ShareInfo) {
        let group = &self.0.pairing;
        let ready = info.state == ShareState::Listening;
        if let Some(pending) = &info.pending {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&wording::wants_to_pair(&pending.name)))
                .subtitle_lines(0)
                .build();
            let decline = button(wording::DECLINE);
            let accept = button(wording::ACCEPT);
            accept.add_css_class("suggested-action");
            let this = self.clone();
            decline.connect_clicked(move |_| this.ask(|core| core.pair_decline(), |_| {}));
            let this = self.clone();
            accept.connect_clicked(move |_| this.ask(|core| core.pair_accept(), |_| {}));
            row.add_suffix(&decline);
            row.add_suffix(&accept);
            self.put(group, &row);
            self.0.live.borrow_mut().pending = Some((row, pending.seconds));
            return;
        }
        if info.pairing {
            let row = adw::ActionRow::builder().title("Pairing is open").build();
            let stop = button(wording::STOP);
            let this = self.clone();
            stop.connect_clicked(move |_| this.ask(|core| core.pair_close(), |_| {}));
            row.add_suffix(&stop);
            self.put(group, &row);
            self.0.live.borrow_mut().pairing = Some(row);
            self.note(
                group,
                &wording::pairing_steps()
                    .iter()
                    .enumerate()
                    .map(|(at, step)| format!("{}. {step}", at + 1))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        } else {
            // How the last pairing ended, or just that it is closed.
            let closed = wording::closed_text(info.closed_reason, info.paired.as_deref());
            let row =
                adw::ActionRow::builder().title(glib::markup_escape_text(&closed)).title_lines(0).build();
            let open = button(wording::PAIR_NEW_DEVICE);
            open.set_sensitive(ready);
            let this = self.clone();
            open.connect_clicked(move |_| this.open_pairing());
            row.add_suffix(&open);
            row.set_activatable_widget(Some(&open));
            self.put(group, &row);
        }
        self.note(group, wording::PAIRING_NOTE);
    }

    fn draw_devices(&self, info: &ShareInfo) {
        let group = &self.0.devices;
        if info.devices.is_empty() {
            self.note(group, wording::NO_DEVICES);
            return;
        }
        for device in &info.devices {
            self.put(group, &self.device_row(device, info.devices.len()));
        }
    }

    fn device_row(&self, device: &DeviceInfo, count: usize) -> adw::ExpanderRow {
        let fingerprint = device.fingerprint.clone();
        let name = if device.name.is_empty() {
            wording::short_fingerprint(&fingerprint)
        } else {
            device.name.clone()
        };
        let row = adw::ExpanderRow::builder()
            .title(glib::markup_escape_text(&name))
            .subtitle(glib::markup_escape_text(&wording::device_line(device)))
            .tooltip_text(&fingerprint)
            .build();
        row.set_expanded(self.0.expanded.borrow().get(&fingerprint).copied().unwrap_or(count <= OPEN_UP_TO));
        let this = self.clone();
        let key = fingerprint.clone();
        row.connect_expanded_notify(move |row| {
            this.0.expanded.borrow_mut().insert(key.clone(), row.is_expanded());
        });
        let remove = button(wording::REMOVE);
        remove.add_css_class("destructive-action");
        let (this, key, shown) = (self.clone(), fingerprint.clone(), name.clone());
        remove.connect_clicked(move |button| {
            let (this, key) = (this.clone(), key.clone());
            crate::rows::confirm(
                button,
                &wording::remove_question(&shown),
                wording::REMOVE_MESSAGE,
                wording::REMOVE,
                move || {
                    let key = key.clone();
                    this.ask(move |core| core.device_revoke(&key), |_| {});
                },
            );
        });
        row.add_suffix(&remove);
        if device.connected {
            let disconnect = button(wording::DISCONNECT);
            let (this, key) = (self.clone(), fingerprint.clone());
            disconnect.connect_clicked(move |_| {
                let key = key.clone();
                this.ask(move |core| core.device_drop(&key), |_| {});
            });
            row.add_suffix(&disconnect);
        }
        for what in PERMISSIONS {
            let (Some(label), Some(on)) = (permission_label(what), device.permissions.get(what)) else {
                continue;
            };
            let switch = adw::SwitchRow::builder().title(label).active(on).build();
            let (this, key) = (self.clone(), fingerprint.clone());
            switch.connect_active_notify(move |switch| {
                let (key, on) = (key.clone(), switch.is_active());
                this.ask(move |core| core.device_allow(&key, what, on), |this| this.redraw_anyway());
            });
            row.add_row(&switch);
        }
        row
    }
}

fn button(label: &str) -> gtk::Button {
    crate::rows::row_button(label)
}

/// What a waiting device's row says: its code, which the person compares, and the time to answer in.
/// The pairing row's second line as markup: the code is the guest's, so it is shown as it is written.
fn pending_text(info: &ShareInfo, left: u64) -> String {
    let code = info.pending.as_ref().map_or("", |pending| pending.code.as_str());
    glib::markup_escape_text(&format!(
        "{}\n{}",
        wording::code_time_left(code, left),
        wording::COMPARE_CODE_LINE
    ))
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use guesthouse_remote_view_core::share::CloseReason;

    #[test]
    fn clock_reads_minutes_and_seconds() {
        assert_eq!(clock(105), "1:45");
        assert_eq!(clock(9), "0:09");
    }

    #[test]
    fn closed_reasons_all_have_words() {
        for reason in [CloseReason::Paired, CloseReason::Declined, CloseReason::Timeout, CloseReason::Asked] {
            assert!(wording::closed_text(Some(reason), None).starts_with("Closed"));
        }
    }
}

#[cfg(test)]
mod tests_words;
