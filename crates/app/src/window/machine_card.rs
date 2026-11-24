//! One machine in the library: its card in the rail, and what can be done to it now. The page's
//! hero shows the chosen machine at length from the same facts.

use super::Window;
use crate::application::APP_ID;
use crate::rows::{confirm, on_yes, question};
use adw::prelude::*;
use guesthouse_app_core::machine::Machine;
use guesthouse_app_core::setup::checks::{machine_card_warning, machine_gpu_warning};
use guesthouse_app_core::start_checks::SECOND_INSTANCE_REFUSAL;
use guesthouse_app_core::tasks::{State, Status, no_slot_text};
use std::cell::RefCell;
use std::rc::Rc;

const THUMBNAIL_WIDTH: i32 = 112;
const THUMBNAIL_HEIGHT: i32 = 70;
const ICON_SIZE: i32 = 40;
const CARD_SPACING: i32 = 14;
const TEXT_SPACING: i32 = 3;
const HEADLINE_CHARS: i32 = 26;

/// One machine's settings, shared by the card and whatever page shows them.
pub type MachineHandle = Rc<RefCell<Machine>>;

/// What a machine shows of itself just now, for its card and for the hero.
#[derive(Clone, Default, PartialEq)]
pub struct Shown {
    pub state: State,
    pub busy: bool,
    /// The state and what the machine is, or the step it's on.
    pub headline: String,
    /// What else there is to say: a problem, or something about its card.
    pub notes: Vec<String>,
    pub start_label: &'static str,
    pub can_start: bool,
    pub can_stop: bool,
}

impl Shown {
    pub fn is_off(&self) -> bool {
        self.state == State::Off
    }

    /// Whether there's a screen to go and look at.
    pub fn has_screen(&self) -> bool {
        matches!(self.state, State::Starting | State::Installing | State::Running)
    }
}

struct Parts {
    card: gtk::Box,
    icon: gtk::Image,
    picture: gtk::Picture,
    headline: gtk::Label,
    spinner: gtk::Spinner,
    start: gtk::Button,
    show: gtk::Button,
}

#[derive(Clone)]
pub struct MachineCard {
    window: Window,
    machine: MachineHandle,
    status: Rc<RefCell<Status>>,
    shown: Rc<RefCell<Shown>>,
    parts: Rc<Parts>,
}

fn card_button(label: &str) -> gtk::Button {
    let button = crate::rows::row_button(label);
    // The card is what a click chooses, so its button doesn't take the focus from it.
    button.set_focus_on_click(false);
    button
}

impl MachineCard {
    pub fn new(window: &Window, machine: Machine) -> Self {
        let icon = gtk::Image::builder()
            .icon_name(APP_ID)
            .pixel_size(ICON_SIZE)
            .width_request(THUMBNAIL_WIDTH)
            .height_request(THUMBNAIL_HEIGHT)
            .build();
        icon.add_css_class("machine-thumbnail");
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Cover)
            .overflow(gtk::Overflow::Hidden)
            .can_shrink(true)
            .build();
        picture.add_css_class("machine-thumbnail");
        // A picture asks for its image's own size, so this holds it to the size of the icon.
        let frame = adw::Clamp::builder()
            .child(&picture)
            .maximum_size(THUMBNAIL_WIDTH)
            .tightening_threshold(THUMBNAIL_WIDTH)
            .valign(gtk::Align::Center)
            .height_request(THUMBNAIL_HEIGHT)
            .visible(false)
            .build();
        let name = gtk::Label::builder()
            .label(&machine.name)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        name.add_css_class("heading");
        let headline = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .lines(2)
            // So a long summary doesn't make every card as wide as the window.
            .max_width_chars(HEADLINE_CHARS)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        headline.add_css_class("dim-label");
        headline.add_css_class("caption");
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(TEXT_SPACING)
            .valign(gtk::Align::Center)
            .hexpand(true)
            .build();
        text.append(&name);
        text.append(&headline);
        let spinner = gtk::Spinner::builder().spinning(true).visible(false).build();
        let start = card_button("Start");
        start.add_css_class("suggested-action");
        let show = card_button("Show Screen");
        let card = gtk::Box::builder().spacing(CARD_SPACING).build();
        card.add_css_class("machine-card");
        card.append(&icon);
        card.append(&frame);
        card.append(&text);
        card.append(&spinner);
        card.append(&start);
        card.append(&show);
        let parts = Parts { card, icon, picture, headline, spinner, start, show };
        let this = Self {
            window: window.clone(),
            machine: Rc::new(RefCell::new(machine)),
            status: Rc::default(),
            shown: Rc::default(),
            parts: Rc::new(parts),
        };
        let pressed = this.clone();
        this.parts.start.connect_clicked(move |_| pressed.on_start());
        let pressed = this.clone();
        this.parts.show.connect_clicked(move |_| pressed.show_screen());
        this.show_thumbnail();
        this.set_status(&Status::default(), false);
        this
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.parts.card
    }

    pub fn machine(&self) -> MachineHandle {
        self.machine.clone()
    }

    pub fn status(&self) -> Status {
        self.status.borrow().clone()
    }

    pub fn shown(&self) -> Shown {
        self.shown.borrow().clone()
    }

    /// The last picture of the guest's screen, if one was kept and can be read.
    pub fn thumbnail(&self) -> Option<gtk::gdk::Texture> {
        let path = self.machine.borrow().thumbnail(&self.window.context());
        gtk::gdk::Texture::from_filename(path).ok()
    }

    /// No picture, or one that can't be read, leaves the icon.
    pub fn show_thumbnail(&self) {
        let texture = self.thumbnail();
        self.parts.picture.set_paintable(texture.as_ref());
        if let Some(frame) = self.parts.picture.parent() {
            frame.set_visible(texture.is_some());
        }
        self.parts.icon.set_visible(texture.is_none());
        self.window.card_picture_changed(self);
    }

    /// The words about the machine: the first is its state, the rest are notes.
    fn lines(&self, status: &Status, blocked: bool, second: bool) -> Vec<String> {
        let machine = self.machine.borrow();
        let state = status.state.name();
        let mut lines = vec![if !status.step.is_empty() {
            format!("{state}: {}", status.step)
        } else if status.screen.is_empty() {
            format!("{state}. {}", machine.summary())
        } else {
            format!("{state}. {}", machine.summary_showing(&status.screen))
        }];
        if machine.installing && status.state == State::Off {
            lines[0] = "The install did not finish. Start Over runs it again from the beginning.".to_owned();
        }
        let notes = [
            &status.card_note,
            &status.error,
            &status.display_problem,
            &status.identity_note,
            &status.token_note,
            &status.jail_note,
            &status.guest_gpu,
        ];
        lines.extend(notes.into_iter().filter(|note| !note.is_empty()).cloned());
        if let Some(point) = &status.restore_point {
            let before = if point.automatic {
                "A snapshot was taken before the last start"
            } else {
                "The last start began from the snapshot it had gone back to"
            };
            lines.push(format!("{before}: {}. Settings, Storage, Snapshots goes back to it.", point.name));
        }
        if blocked {
            lines.push(no_slot_text(&self.window.context()));
        }
        if second {
            lines.push(SECOND_INSTANCE_REFUSAL.to_owned());
        }
        lines
    }

    pub fn set_status(&self, status: &Status, vm_running: bool) {
        *self.status.borrow_mut() = status.clone();
        let (installing, has_gpu) = {
            let machine = self.machine.borrow();
            (machine.installing, machine.has_gpu())
        };
        let off = status.state == State::Off;
        let blocked = off && vm_running;
        let second = off && self.window.context().second_instance() && has_gpu;
        let mut lines = self.lines(status, blocked, second);
        let shown = Shown {
            state: status.state,
            busy: status.busy,
            headline: lines.remove(0),
            notes: lines,
            start_label: if installing { "Start Over" } else { "Start" },
            can_start: !status.busy && !blocked && !second,
            can_stop: !status.busy,
        };
        // This is called each second, and mostly nothing has changed.
        if *self.shown.borrow() == shown {
            return;
        }
        let parts = &self.parts;
        parts.headline.set_label(&shown.headline);
        // The card has room for one thing, so the rest is a pointer away.
        let notes = shown.notes.join("\n");
        parts.card.set_tooltip_text((!notes.is_empty()).then_some(notes.as_str()));
        if notes.is_empty() {
            parts.card.remove_css_class("has-notes");
        } else {
            parts.card.add_css_class("has-notes");
        }
        parts.spinner.set_visible(shown.busy);
        parts.start.set_label(shown.start_label);
        parts.start.set_visible(off);
        parts.start.set_sensitive(shown.can_start);
        parts.show.set_visible(shown.has_screen());
        *self.shown.borrow_mut() = shown;
        self.window.card_changed(self);
    }

    fn start(&self) {
        let machine = self.machine.borrow();
        let action = if machine.installing { "install" } else { "start" };
        self.window.launch(action, &machine, &[], None);
    }

    pub fn on_start(&self) {
        let warning = machine_gpu_warning(&self.window.context(), &self.machine.borrow());
        let Some(warning) = warning else {
            self.start();
            return;
        };
        let experimental = machine_card_warning(&self.window.context(), &self.machine.borrow()).is_some();
        let title = if experimental { "Experimental Card" } else { "Different NVIDIA Driver" };
        let dialog = question(title, &warning, "Start Anyway", adw::ResponseAppearance::Default);
        let this = self.clone();
        on_yes(&dialog, move || this.start());
        dialog.present(Some(self.window.widget()));
    }

    /// Whether its machine has a screen to show just now.
    pub fn has_screen(&self) -> bool {
        self.shown.borrow().has_screen()
    }

    pub fn show_screen(&self) {
        self.window.show_screen(self.machine.clone());
    }

    pub fn shut_down(&self) {
        self.window.launch("stop", &self.machine.borrow(), &[], None);
    }

    pub fn show_settings(&self) {
        self.window.show_settings(self.machine.clone());
    }

    pub fn on_force(&self) {
        let this = self.clone();
        confirm(
            self.window.widget(),
            "Force Off?",
            "This cuts the power. The guest gets no chance to save anything.",
            "Force Off",
            move || this.window.launch("force", &this.machine.borrow(), &[], None),
        );
    }
}
