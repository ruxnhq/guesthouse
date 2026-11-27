//! Apps: the programs of a Windows machine, shown as windows of their own on this desktop. The page has
//! the machine's switch for it, the programs Windows lists with how many windows each has open, and for
//! each an Open button and a switch that puts it in this desktop's launcher.
//!
//! What it says for each state of the machine is decided by `guesthouse_app_core::app_windows::page`,
//! which has the tests. This file draws it. The helper in the guest is asked only while the page is
//! showing, the switch is on, and the machine runs with the way in to its helper. The answer is a
//! snapshot: the open windows are as they were when the helper answered, and are asked again after an
//! Open and when Try Again is pressed.

use super::Shared;
use crate::common::in_background;
use crate::rows::{explain, name_for_readers, row_button};
use crate::viewer::app_windows::launch::start_viewer;
use adw::prelude::*;
use gtk::{gdk, glib};
use guesthouse_app_core::app_windows::launcher::{self, Listed};
use guesthouse_app_core::app_windows::page::{self, Body, Look, Row, Situation};
use guesthouse_app_core::app_windows::{is_on, running_machine_has_forward, set_on};
use guesthouse_app_core::headless::{Opening, look, open_program};
use guesthouse_app_core::machine::Kind;
use guesthouse_app_core::slots;
use guesthouse_app_core::tasks::{State, Status};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

const ICON_SIZE: i32 = 36;
const FALLBACK_ICON: &str = crate::application::own_icon::APP;
/// How long after an Open the windows are counted again, as a window takes a moment to show.
const COUNT_AFTER_OPEN: Duration = Duration::from_secs(3);
const RETRY_FIRST: u64 = 3;
const RETRY_MOST: u64 = 15;
const NOTE_MARGIN: i32 = 6;

/// How often an Open looks whether the screen page it closed has let go of its machine, and how long
/// it waits for that at most.
const SCREEN_CLOSE_LOOK: Duration = Duration::from_millis(50);
const SCREEN_CLOSE_MOST: Duration = Duration::from_secs(10);

/// What a look came to, with the pictures its icons make, which are decoded on the thread that asked.
struct Looked {
    look: Look,
    icons: HashMap<String, gdk::Texture>,
}

/// The picture of an icon the guest sent, if it is a whole PNG of a sane size: it is not trusted, and
/// is checked before an image loader opens it, as it is before it is written to a file.
fn texture_of(png: Vec<u8>) -> Option<gdk::Texture> {
    if !launcher::valid_icon(&png) {
        return None;
    }
    gdk::Texture::from_bytes(&glib::Bytes::from_owned(png)).ok()
}

fn decode_icons(look: &Look) -> HashMap<String, gdk::Texture> {
    let Look::Listed(found) = look else { return HashMap::new() };
    found
        .apps
        .iter()
        .filter_map(|app| {
            let png = guesthouse_app_core::text::base64_decode(&app.icon)?;
            let texture = texture_of(png)?;
            Some((app.id.clone(), texture))
        })
        .collect()
}

#[derive(Default)]
struct Inner {
    status: Status,
    on: bool,
    /// Whether the running machine has the way in to its helper, looked at once for each run.
    forward: Option<bool>,
    forward_seen: bool,
    found: Option<guesthouse_app_core::app_windows::Found>,
    icons: HashMap<String, gdk::Texture>,
    problem: Option<(String, bool)>,
    asked: bool,
    looking: bool,
    retries: u64,
    show_all: bool,
    opening: HashSet<String>,
    /// Counts the looks, so an answer that comes after the page moved on is let go.
    generation: u64,
    drawn: Option<(Body, bool)>,
}

struct Parts {
    shared: Shared,
    switch: adw::SwitchRow,
    note: gtk::Label,
    found: adw::PreferencesGroup,
    count: gtk::Label,
    inner: RefCell<Inner>,
    /// What the list holds, to take out when it is drawn again.
    shown: RefCell<Vec<gtk::Widget>>,
    open_buttons: RefCell<HashMap<String, gtk::Button>>,
    /// The switch is being set from the file, not by a click.
    setting: Cell<bool>,
    mapped: Cell<bool>,
    closed: Cell<bool>,
}

#[derive(Clone)]
pub struct Apps(Rc<Parts>);

impl Apps {
    /// Only a Windows machine that exists has the page.
    pub fn new(shared: &Shared, page: &adw::PreferencesPage) -> Option<Self> {
        if shared.is_draft() || shared.machine.borrow().kind != Kind::Windows {
            return None;
        }
        let card = adw::PreferencesGroup::new();
        let switch =
            adw::SwitchRow::builder().title(page::SWITCH_TITLE).subtitle(page::SWITCH_SUBTITLE).build();
        explain(&switch, page::SWITCH_INFO);
        card.add(&switch);
        page.add(&card);
        let note =
            gtk::Label::builder().wrap(true).xalign(0.0).visible(false).margin_top(NOTE_MARGIN).build();
        note.add_css_class("dim-label");
        card.add(&note);
        let found = adw::PreferencesGroup::builder().title(page::HEADING_FOUND).build();
        let count = gtk::Label::builder().valign(gtk::Align::Center).build();
        count.add_css_class("dim-label");
        found.set_header_suffix(Some(&count));
        page.add(&found);
        let foot = adw::PreferencesGroup::new();
        let footnote = gtk::Label::builder().label(page::FOOTNOTE).wrap(true).xalign(0.0).build();
        footnote.add_css_class("dim-label");
        foot.add(&footnote);
        page.add(&foot);
        let this = Self(Rc::new(Parts {
            shared: shared.clone(),
            switch,
            note,
            found,
            count,
            inner: RefCell::default(),
            shown: RefCell::default(),
            open_buttons: RefCell::default(),
            setting: Cell::new(false),
            mapped: Cell::new(false),
            closed: Cell::new(false),
        }));
        this.0.inner.borrow_mut().on = this.read_switch();
        this.0.setting.set(true);
        this.0.switch.set_active(this.0.inner.borrow().on);
        this.0.setting.set(false);
        let handle = this.clone();
        this.0.switch.connect_active_notify(move |row| {
            if !handle.0.setting.get() {
                handle.on_switch(row.is_active());
            }
        });
        let handle = this.clone();
        page.connect_map(move |_| {
            handle.0.mapped.set(true);
            handle.update();
        });
        let handle = this.clone();
        page.connect_unmap(move |_| handle.0.mapped.set(false));
        let handle = this.clone();
        shared.dialog.connect_closed(move |_| {
            handle.0.closed.set(true);
            handle.0.inner.borrow_mut().generation += 1;
        });
        this.draw();
        Some(this)
    }

    fn read_switch(&self) -> bool {
        is_on(&self.0.shared.window.context(), &self.0.shared.machine.borrow())
    }

    fn situation(&self) -> Situation {
        let inner = self.0.inner.borrow();
        Situation { on: inner.on, state: inner.status.state, has_forward: inner.forward }
    }

    fn name(&self) -> String {
        self.0.shared.machine.borrow().name.clone()
    }

    pub fn set_status(&self, status: &Status) {
        {
            let mut inner = self.0.inner.borrow_mut();
            inner.status = status.clone();
            if status.state != State::Running {
                inner.forward_seen = false;
                inner.forward = None;
            }
        }
        self.update();
    }

    fn on_switch(&self, on: bool) {
        let context = self.0.shared.window.context();
        let result = set_on(&context, &self.0.shared.machine.borrow(), on);
        if let Err(failure) = result {
            self.0.shared.window.problem(&failure.0);
            self.0.setting.set(true);
            self.0.switch.set_active(!on);
            self.0.setting.set(false);
            return;
        }
        if !on {
            // A viewer that runs ends by itself as it sees the switch off. The helper in the guest
            // is ended too, so that the guest's windows are as Windows had them.
            let (context, machine) = (context.clone(), self.0.shared.machine.borrow().clone());
            std::thread::spawn(move || {
                if let Some(slot) = guesthouse_app_core::tasks::slot_of(&context, &machine) {
                    guesthouse_app_core::app_windows::modes::end_helper(&context, &machine, slot);
                }
            });
        }
        {
            let mut inner = self.0.inner.borrow_mut();
            inner.on = on;
            inner.forward_seen = false;
            inner.asked = false;
            inner.found = None;
            inner.problem = None;
            inner.generation += 1;
        }
        self.update();
    }

    /// Takes in what changed, draws what shows differently, and asks the helper if it is time to.
    fn update(&self) {
        if self.0.closed.get() {
            return;
        }
        self.look_at_the_forward();
        let situation = self.situation();
        let body = page::body(&situation);
        let note = page::start_note(&situation);
        {
            let mut inner = self.0.inner.borrow_mut();
            if body != Body::Ask {
                inner.asked = false;
                inner.found = None;
                inner.problem = None;
                inner.looking = false;
                inner.retries = 0;
                inner.generation += 1;
            }
        }
        self.0.note.set_label(note.unwrap_or_default());
        self.0.note.set_visible(note.is_some());
        let drawn = (body, self.0.inner.borrow().show_all);
        if self.0.inner.borrow().drawn != Some(drawn) {
            self.draw();
        }
        let ask = body == Body::Ask && self.0.mapped.get() && !self.0.inner.borrow().asked;
        if ask {
            self.ask();
        }
    }

    fn look_at_the_forward(&self) {
        let (running, seen, slot) = {
            let inner = self.0.inner.borrow();
            (inner.status.state == State::Running && inner.on, inner.forward_seen, inner.status.slot)
        };
        if running && !seen {
            let context = self.0.shared.window.context();
            let forward = running_machine_has_forward(
                &context,
                &self.0.shared.machine.borrow(),
                slot.map(slots::number),
            );
            let mut inner = self.0.inner.borrow_mut();
            inner.forward = forward;
            inner.forward_seen = true;
        }
    }

    /// Asks the helper, off the GTK thread, and takes the answer if the page is still waiting for it.
    fn ask(&self) {
        let generation = {
            let mut inner = self.0.inner.borrow_mut();
            inner.asked = true;
            inner.looking = true;
            inner.generation += 1;
            inner.generation
        };
        self.draw();
        let context = self.0.shared.window.context();
        let machine = self.0.shared.machine.borrow().clone();
        let asking = context.clone();
        let this = self.clone();
        in_background(
            &context,
            move || {
                let answer = look(&asking, &machine);
                let icons = decode_icons(&answer);
                Ok(Looked { look: answer, icons })
            },
            move |result| {
                let looked =
                    result.unwrap_or_else(|text| Looked { look: Look::Failed(text), icons: HashMap::new() });
                this.looked(generation, looked);
            },
        );
    }

    fn looked(&self, generation: u64, looked: Looked) {
        if self.0.closed.get() || self.0.inner.borrow().generation != generation {
            return;
        }
        let retry = {
            let mut inner = self.0.inner.borrow_mut();
            inner.looking = false;
            match looked.look {
                Look::Listed(found) => {
                    inner.found = Some(found);
                    inner.icons = looked.icons;
                    inner.problem = None;
                    inner.retries = 0;
                    None
                }
                other => {
                    inner.problem = page::look_problem(&self.name(), &other);
                    let again = inner.problem.as_ref().is_some_and(|(_, again)| *again);
                    inner.retries += 1;
                    again.then(|| Duration::from_secs((RETRY_FIRST * inner.retries).min(RETRY_MOST)))
                }
            }
        };
        self.draw();
        if let Some(after) = retry {
            let this = self.clone();
            glib::timeout_add_local_once(after, move || {
                // Only the same wait: a page that moved on, or was asked again by hand, has its own.
                let waiting = {
                    let inner = this.0.inner.borrow();
                    inner.generation == generation && inner.problem.is_some()
                };
                if waiting && !this.0.closed.get() {
                    this.0.inner.borrow_mut().asked = false;
                    this.update();
                }
            });
        }
    }

    fn try_again(&self) {
        {
            let mut inner = self.0.inner.borrow_mut();
            inner.asked = false;
            inner.problem = None;
            inner.retries = 0;
            inner.generation += 1;
        }
        self.update();
    }

    fn entries(&self) -> Vec<Listed> {
        launcher::list(&self.0.shared.window.context(), &self.0.shared.machine.borrow())
    }

    fn rows(&self, body: Body, entries: &[Listed]) -> Vec<Row> {
        let context = self.0.shared.window.context();
        let machine = self.0.shared.machine.borrow().clone();
        let icon_of = move |app_id: &str| launcher::icon_of(&context, app_id);
        let inner = self.0.inner.borrow();
        match (&inner.found, body) {
            (Some(found), Body::Ask) => {
                page::rows_found(&machine, &found.apps, Some(&found.windows), entries, &icon_of)
            }
            _ => page::rows_launcher(&machine, entries, &icon_of),
        }
    }

    fn clear(&self) {
        for widget in self.0.shown.borrow_mut().drain(..) {
            self.0.found.remove(&widget);
        }
        self.0.open_buttons.borrow_mut().clear();
    }

    fn add(&self, widget: &impl IsA<gtk::Widget>) {
        self.0.found.add(widget);
        self.0.shown.borrow_mut().push(widget.clone().upcast());
    }

    /// A row that says one thing instead of a program: what the page is doing, or why it has no list.
    fn message(&self, text: &str, spinner: bool, button: Option<gtk::Button>) {
        let row = adw::ActionRow::builder().title(text).use_markup(false).build();
        if spinner {
            let spinning = gtk::Spinner::builder().spinning(true).valign(gtk::Align::Center).build();
            name_for_readers(&spinning, "Looking");
            row.add_prefix(&spinning);
        }
        if let Some(button) = button {
            row.add_suffix(&button);
        }
        self.add(&row);
    }

    fn draw(&self) {
        let situation = self.situation();
        let body = page::body(&situation);
        let name = self.name();
        self.clear();
        let entries =
            if matches!(body, Body::SwitchedOff | Body::Waiting(_)) { Vec::new() } else { self.entries() };
        let shows_list = !matches!(body, Body::SwitchedOff | Body::Waiting(_));
        self.0.count.set_label(&page::launcher_count_text(entries.len()));
        self.0.count.set_visible(shows_list);
        let waiting_for_list = body == Body::Ask && self.0.inner.borrow().found.is_none();
        self.0.found.set_title(if matches!(body, Body::MachineOff | Body::NeedsRestart) {
            page::HEADING_LAUNCHER
        } else {
            page::HEADING_FOUND
        });
        let show_all = self.0.inner.borrow().show_all;
        self.0.inner.borrow_mut().drawn = Some((body, show_all));
        if let Some(text) = page::placeholder(body, &name) {
            self.message(&text, false, None);
            return;
        }
        if waiting_for_list {
            self.draw_not_listed();
            return;
        }
        if body == Body::MachineOff {
            self.message(&page::off_note(&name), false, None);
        }
        let rows = self.rows(body, &entries);
        if rows.is_empty() {
            let text =
                if body == Body::MachineOff { page::off_and_none(&name) } else { page::nothing_found(&name) };
            self.message(&text, false, None);
            return;
        }
        let all = self.0.inner.borrow().show_all;
        let (kept, left_out) = page::shown(&rows, all);
        for row in kept {
            self.add(&self.program_row(row, body));
        }
        if left_out > 0 {
            let more = adw::ActionRow::builder().title(page::more_text(left_out)).use_markup(false).build();
            let show = row_button(page::SHOW_ALL);
            let this = self.clone();
            show.connect_clicked(move |_| {
                this.0.inner.borrow_mut().show_all = true;
                this.draw();
            });
            more.add_suffix(&show);
            self.add(&more);
        }
    }

    /// The body is Ask and the helper has not given a list: it is being asked, or could not be.
    fn draw_not_listed(&self) {
        let problem = self.0.inner.borrow().problem.clone();
        match problem {
            None => self.message(page::LOOKING, true, None),
            Some((text, _)) => {
                // A look that tries again by itself keeps the button: it asks at once.
                let button = row_button(page::TRY_AGAIN);
                let this = self.clone();
                button.connect_clicked(move |_| this.try_again());
                self.message(&text, false, Some(button));
            }
        }
    }

    fn program_row(&self, row: &Row, body: Body) -> adw::ActionRow {
        let otherwise = if body == Body::NeedsRestart { page::AFTER_RESTART } else { page::STARTS_MACHINE };
        let line = adw::ActionRow::builder()
            .title(&row.name)
            .use_markup(false)
            .subtitle(page::open_text(row.open, otherwise))
            .build();
        line.add_prefix(&self.icon_of(row));
        let open = row_button(page::OPEN);
        name_for_readers(&open, &page::open_label(&row.name));
        let opening = self.0.inner.borrow().opening.contains(&row.app_id);
        if opening {
            open.set_child(Some(&gtk::Spinner::builder().spinning(true).build()));
        }
        open.set_sensitive(!opening && body != Body::NeedsRestart);
        if body == Body::NeedsRestart {
            open.set_tooltip_text(Some(page::OPEN_WAITS));
        }
        let this = self.clone();
        let chosen = row.clone();
        open.connect_clicked(move |_| this.on_open(&chosen));
        self.0.open_buttons.borrow_mut().insert(row.app_id.clone(), open.clone());
        line.add_suffix(&open);
        let label = gtk::Label::new(Some(page::IN_LAUNCHER));
        label.add_css_class("dim-label");
        label.set_valign(gtk::Align::Center);
        line.add_suffix(&label);
        let switch = gtk::Switch::builder().active(row.in_launcher).valign(gtk::Align::Center).build();
        name_for_readers(&switch, &page::launcher_label(&row.name));
        let this = self.clone();
        let chosen = row.clone();
        switch.connect_active_notify(move |switch| this.on_launcher(&chosen, switch));
        line.add_suffix(&switch);
        line
    }

    fn icon_of(&self, row: &Row) -> gtk::Image {
        let texture =
            self.0.inner.borrow().icons.get(&row.program).cloned().or_else(|| texture_of(row.icon.clone()?));
        let image = match texture {
            Some(texture) => gtk::Image::from_paintable(Some(&texture)),
            None => gtk::Image::from_icon_name(FALLBACK_ICON),
        };
        image.set_pixel_size(ICON_SIZE);
        image.set_valign(gtk::Align::Center);
        image.set_accessible_role(gtk::AccessibleRole::Presentation);
        image
    }

    fn on_launcher(&self, row: &Row, switch: &gtk::Switch) {
        if self.0.setting.get() {
            return;
        }
        let context = self.0.shared.window.context();
        let machine = self.0.shared.machine.borrow().clone();
        let result = if switch.is_active() {
            launcher::add(&context, &machine, &row.to_program()).map(|_| ())
        } else {
            launcher::remove(&context, &machine, &row.app_id).map(|_| ())
        };
        match result {
            Ok(()) => {
                self.0.count.set_label(&page::launcher_count_text(self.entries().len()));
            }
            Err(failure) => {
                self.0.shared.window.problem(&failure.0);
                // The switch goes back to what the launcher has, without asking again.
                self.0.setting.set(true);
                switch.set_active(!switch.is_active());
                self.0.setting.set(false);
            }
        }
    }

    /// Does what `guesthouse --app` does: starts the machine if it is off, starts the program, and sees
    /// that one viewer shows the machine's windows. It can take minutes, and a failure comes as a toast.
    fn on_open(&self, row: &Row) {
        if !self.0.inner.borrow_mut().opening.insert(row.app_id.clone()) {
            return;
        }
        if let Some(button) = self.0.open_buttons.borrow().get(&row.app_id) {
            button.set_sensitive(false);
            button.set_child(Some(&gtk::Spinner::builder().spinning(true).build()));
        }
        let context = self.0.shared.window.context();
        let machine = self.0.shared.machine.borrow().id.clone();
        let (program, start_menu) = (row.program.clone(), row.start_menu);
        let running = context.clone();
        let running_for_work = context.clone();
        let this = self.clone();
        let key = row.app_id.clone();
        // The screen and the app windows are two modes of one machine. Asking for a program is asking
        // for its window, so the machine's open screen goes, and then the program opens.
        let closed = self.0.shared.window.close_screen_of(&machine);
        let work = move || {
            in_background(
                &running_for_work,
                move || {
                    let opening = Opening { machine: &machine, program: &program, start_menu };
                    open_program(&running, &opening, &mut start_viewer, &mut std::io::sink())
                },
                move |result| this.opened(&key, result),
            );
        };
        if !closed {
            work();
            return;
        }
        // The screen page lets go of the machine's connection as it finishes closing, and then it is
        // no longer the window's open screen.
        let window = self.0.shared.window.clone();
        let this = self.clone();
        let key = row.app_id.clone();
        let name = self.0.shared.machine.borrow().name.clone();
        let deadline = std::time::Instant::now() + SCREEN_CLOSE_MOST;
        let mut work = Some(work);
        glib::timeout_add_local(SCREEN_CLOSE_LOOK, move || {
            if window.open_screen().is_none() {
                if let Some(work) = work.take() {
                    work();
                }
                return glib::ControlFlow::Break;
            }
            if std::time::Instant::now() >= deadline {
                let text = format!("The screen of {name} did not close, so the app was not opened.");
                this.opened(&key, Err(text));
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
    }

    fn opened(&self, key: &str, result: Result<(), String>) {
        self.0.inner.borrow_mut().opening.remove(key);
        if let Some(button) = self.0.open_buttons.borrow().get(key) {
            button.set_child(None::<&gtk::Widget>);
            button.set_label(page::OPEN);
            button.set_sensitive(true);
        }
        match result {
            Err(text) => self.0.shared.window.problem(&text),
            Ok(()) => {
                // The window takes a moment to show, and then the count is asked for again.
                let this = self.clone();
                glib::timeout_add_local_once(COUNT_AFTER_OPEN, move || {
                    if !this.0.closed.get() && page::body(&this.situation()) == Body::Ask {
                        this.0.inner.borrow_mut().asked = false;
                        this.update();
                    }
                });
            }
        }
    }
}
