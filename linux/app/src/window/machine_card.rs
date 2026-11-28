//! One machine in the library: its card in the rail, and what can be done to it now. The page's
//! hero shows the chosen machine at length from the same facts.

use super::Window;
use super::weight::Weight;
use crate::rows::{confirm, on_yes, question};
use adw::prelude::*;
use guesthouse_app_core::app_windows::open_windows::{OpenProgram, count_text};
use guesthouse_app_core::machine::{Kind, Machine};
use guesthouse_app_core::setup::checks::{machine_card_warning, machine_gpu_warning};
use guesthouse_app_core::start_checks::SECOND_INSTANCE_REFUSAL;
use guesthouse_app_core::tasks::{State, Status, no_slot_text};
use std::cell::RefCell;
use std::rc::Rc;

const THUMBNAIL_WIDTH: i32 = 112;
const THUMBNAIL_HEIGHT: i32 = 70;
/// The same in a sidebar of machines, where a card is one line of a list.
const COMPACT_THUMBNAIL_WIDTH: i32 = 54;
const COMPACT_THUMBNAIL_HEIGHT: i32 = 36;
const COMPACT_ICON_SIZE: i32 = 20;
const COMPACT_SPACING: i32 = 10;
const ICON_SIZE: i32 = 40;
const CARD_SPACING: i32 = 14;
const TEXT_SPACING: i32 = 3;
const HEADLINE_CHARS: i32 = 26;
const CHIP_ICON: i32 = 18;
const CHIP_SPACING: i32 = 6;
const CHIP_GAP: u32 = 6;
/// Show Screen, while the app windows that are open are being ended.
pub const SCREEN_BUSY: &str = "Closing App Windows…";
pub const SCREEN_LABEL: &str = "Show Screen";
const OPEN_APPS_LABEL: &str = "Open Another App";

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
    /// "3 app windows open.", for the card's status line. The hero has its own way to say it.
    pub app_windows: Option<String>,
    /// How heavily the machine sits in the library just now.
    pub weight: Weight,
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
    row: gtk::Box,
    icon: gtk::Image,
    picture: gtk::Picture,
    frame: adw::Clamp,
    headline: gtk::Label,
    spinner: gtk::Spinner,
    start: gtk::Button,
    show: gtk::Button,
    /// What is open as app windows, a chip for each program, and the button for another.
    apps: gtk::FlowBox,
    open_app: gtk::Button,
}

#[derive(Clone)]
pub struct MachineCard {
    window: Window,
    machine: MachineHandle,
    /// The machine as its file was when the handle was last read from it or saved to it, which says
    /// what the handle has changed since.
    known: Rc<RefCell<Machine>>,
    status: Rc<RefCell<Status>>,
    shown: Rc<RefCell<Shown>>,
    parts: Rc<Parts>,
    open: Rc<RefCell<Vec<OpenProgram>>>,
    /// The screen is being got ready, which can take a moment when app windows are open.
    screen_busy: Rc<std::cell::Cell<bool>>,
}

fn card_button(label: &str) -> gtk::Button {
    let button = crate::rows::row_button(label);
    // The card is what a click chooses, so its button doesn't take the focus from it.
    button.set_focus_on_click(false);
    button
}

/// The icon a machine shows until it has a picture of its screen: what it runs, as New Machine names it.
pub fn kind_icon(machine: &Machine) -> &'static str {
    match machine.kind {
        Kind::Windows => crate::application::own_icon::WINDOW,
        Kind::Linux => crate::application::own_icon::TERMINAL,
        Kind::Plain if machine.android() => crate::application::own_icon::PHONE,
        Kind::Plain if machine.macos() => crate::application::own_icon::DISPLAY,
        Kind::Plain if machine.linux_gpu => crate::application::own_icon::TERMINAL,
        Kind::Plain => crate::application::own_icon::DISC,
    }
}

impl MachineCard {
    pub fn new(window: &Window, machine: Machine) -> Self {
        let icon = gtk::Image::builder()
            .icon_name(kind_icon(&machine))
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
        let show = card_button(SCREEN_LABEL);
        let row = gtk::Box::builder().spacing(CARD_SPACING).build();
        row.append(&icon);
        row.append(&frame);
        row.append(&text);
        row.append(&spinner);
        row.append(&start);
        row.append(&show);
        let open_app = card_button(OPEN_APPS_LABEL);
        open_app.add_css_class("flat");
        let apps = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .homogeneous(false)
            .column_spacing(CHIP_GAP)
            .row_spacing(CHIP_GAP)
            .max_children_per_line(32)
            .min_children_per_line(1)
            .can_focus(false)
            .visible(false)
            .build();
        apps.add_css_class("app-chips");
        let card = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(CARD_SPACING).build();
        card.add_css_class("machine-card");
        card.append(&row);
        card.append(&apps);
        let parts = Parts { card, row, icon, picture, frame, headline, spinner, start, show, apps, open_app };
        let this = Self {
            window: window.clone(),
            known: Rc::new(RefCell::new(machine.clone())),
            machine: Rc::new(RefCell::new(machine)),
            status: Rc::default(),
            shown: Rc::default(),
            parts: Rc::new(parts),
            open: Rc::default(),
            screen_busy: Rc::default(),
        };
        let pressed = this.clone();
        this.parts.start.connect_clicked(move |_| pressed.on_start());
        let pressed = this.clone();
        this.parts.show.connect_clicked(move |_| pressed.show_screen());
        let pressed = this.clone();
        this.parts.open_app.connect_clicked(move |_| pressed.open_apps_page());
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

    pub fn known(&self) -> Machine {
        self.known.borrow().clone()
    }

    pub fn set_known(&self, machine: Machine) {
        *self.known.borrow_mut() = machine;
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
            app_windows: count_text(&self.open.borrow()),
            weight: Weight::of(status, installing),
        };
        // This is called each second, and mostly nothing has changed.
        if *self.shown.borrow() == shown {
            return;
        }
        let parts = &self.parts;
        shown.weight.mark(&parts.card);
        // The card has room for one thing, so the rest is a pointer away.
        let notes = shown.notes.join("\n");
        parts.card.set_tooltip_text((!notes.is_empty()).then_some(notes.as_str()));
        if notes.is_empty() {
            parts.card.remove_css_class("has-notes");
        } else {
            parts.card.add_css_class("has-notes");
        }
        parts.spinner.set_visible(shown.busy);
        *self.shown.borrow_mut() = shown;
        self.show_headline();
        self.show_buttons();
        self.window.card_changed(self);
    }

    /// Whether the card is one line of a sidebar, where the hero beside it has the buttons and the
    /// app windows, or a card of its own in a rail under the hero.
    pub fn set_compact(&self, compact: bool) {
        let parts = &self.parts;
        if compact {
            parts.card.add_css_class("compact");
        } else {
            parts.card.remove_css_class("compact");
        }
        let (width, height, icon, spacing) = if compact {
            (COMPACT_THUMBNAIL_WIDTH, COMPACT_THUMBNAIL_HEIGHT, COMPACT_ICON_SIZE, COMPACT_SPACING)
        } else {
            (THUMBNAIL_WIDTH, THUMBNAIL_HEIGHT, ICON_SIZE, CARD_SPACING)
        };
        parts.icon.set_size_request(width, height);
        parts.icon.set_pixel_size(icon);
        parts.frame.set_maximum_size(width);
        parts.frame.set_tightening_threshold(width);
        parts.frame.set_height_request(height);
        parts.row.set_spacing(spacing);
        parts.headline.set_lines(if compact { 1 } else { 2 });
        self.show_headline();
        self.show_buttons();
    }

    /// The card's line under the name. A card in a sidebar has room for one line, so a machine that
    /// needs the user or failed says why there, and the rest say how they stand.
    fn show_headline(&self) {
        let shown = self.shown.borrow();
        let line = match (&shown.app_windows, shown.notes.first()) {
            (_, Some(note))
                if self.is_compact() && matches!(shown.weight, Weight::Swells | Weight::Spills) =>
            {
                note.clone()
            }
            // The hero beside it says what the machine is.
            _ if self.is_compact() => shown.state.name().to_owned(),
            (Some(count), _) => format!("{} {count}", shown.headline),
            (None, _) => shown.headline.clone(),
        };
        self.parts.headline.set_label(&line);
    }

    fn is_compact(&self) -> bool {
        self.parts.card.has_css_class("compact")
    }

    /// Start, Show Screen and the app windows, on a card with room for them.
    fn show_buttons(&self) {
        let parts = &self.parts;
        let shown = self.shown.borrow();
        let roomy = !self.is_compact();
        parts.start.set_label(shown.start_label);
        parts.start.set_visible(roomy && shown.is_off());
        parts.start.set_sensitive(shown.can_start);
        parts.show.set_visible(roomy && shown.has_screen());
        parts.apps.set_visible(roomy && !self.open.borrow().is_empty());
    }

    /// The programs that have app windows open, as the machine's viewer says. The chips are made again
    /// only when that changes, and none show when nothing is open.
    pub fn set_app_windows(&self, programs: &[OpenProgram]) {
        if *self.open.borrow() == programs {
            return;
        }
        *self.open.borrow_mut() = programs.to_vec();
        let apps = &self.parts.apps;
        apps.remove_all();
        for program in programs {
            apps.append(&chip(program));
        }
        if !programs.is_empty() {
            apps.append(&self.parts.open_app);
        }
        apps.set_visible(!programs.is_empty() && !self.is_compact());
        // The hero shows the same chips when this machine is the chosen one.
        self.window.card_changed(self);
    }

    /// Whether Show Screen is waiting for the app windows to end.
    pub fn screen_busy(&self) -> bool {
        self.screen_busy.get()
    }

    /// Show Screen says it is busy, on the card and on the hero.
    pub fn set_screen_busy(&self, busy: bool) {
        self.screen_busy.set(busy);
        self.parts.show.set_sensitive(!busy);
        self.parts.show.set_label(if busy { SCREEN_BUSY } else { SCREEN_LABEL });
        self.window.card_changed(self);
    }

    /// The programs with windows open, as the viewer last said.
    pub fn open_programs(&self) -> Vec<OpenProgram> {
        self.open.borrow().clone()
    }

    /// The machine's Settings, turned to the Apps page.
    pub(super) fn open_apps_page(&self) {
        self.show_settings();
        self.window.show_settings_page("apps");
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
        self.window.open_screen_of(self);
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

/// One program with windows open: its icon, its name, and how many windows when that is more than one.
/// It only tells. A window of another program can't be raised from here, as Wayland lets only the
/// program that owns a window say which one has the focus.
pub(super) fn chip(program: &OpenProgram) -> gtk::FlowBoxChild {
    let name = program.shown_name();
    let holder = gtk::Box::builder().spacing(CHIP_SPACING).build();
    holder.add_css_class("app-chip");
    let texture = program.icon.as_ref().and_then(|file| gtk::gdk::Texture::from_filename(file).ok());
    let icon = match texture {
        Some(texture) => gtk::Image::from_paintable(Some(&texture)),
        None => gtk::Image::from_icon_name(crate::application::own_icon::APP),
    };
    icon.set_pixel_size(CHIP_ICON);
    holder.append(&icon);
    holder.append(&gtk::Label::builder().label(&name).ellipsize(gtk::pango::EllipsizeMode::End).build());
    let described = if program.windows > 1 {
        let count = gtk::Label::new(Some(&program.windows.to_string()));
        count.add_css_class("dim-label");
        holder.append(&count);
        format!("{name}, {} windows open", program.windows)
    } else {
        format!("{name}, 1 window open")
    };
    let child = gtk::FlowBoxChild::builder().child(&holder).focusable(false).can_focus(false).build();
    crate::rows::name_for_readers(&child, &described);
    child
}
