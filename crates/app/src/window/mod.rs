//! The main window, and what every page asks of it: telling the user about things, starting tasks and
//! saving machines. The pages it shows are modules of their own.

mod actions;
mod go_back;
mod hero;
mod keys;
mod library_page;
mod machine_card;
mod refresh;
mod screen_hdr;
mod share_panel;
mod start_over;
mod updates;

pub use machine_card::{MachineCard, MachineHandle};

use crate::common::in_background;
use crate::log_dialog;
use crate::new_machine::NewMachineDialog;
use crate::settings::SettingsDialog;
use crate::tray::{Asked, Tray};
use crate::viewer::ScreenPage;
use crate::wizard;
use adw::prelude::*;
use gtk::glib;
use guesthouse_app_core::context::Context;
use guesthouse_app_core::gpu_support::{NO_GPU_PART, gpu_ready, what_machines_are};
use guesthouse_app_core::library::{delete, import_existing, remove_entry};
use guesthouse_app_core::log::write_log;
use guesthouse_app_core::machine::{Machine, load_all};
use guesthouse_app_core::settings::{Fields, read_prefs, write_prefs};
use guesthouse_app_core::setup::setup_done;
use guesthouse_app_core::tasks::launch;
use guesthouse_daemon::slot::Slot;
use library_page::LibraryPage;
use refresh::RefreshState;
use serde_json::{Value, json};
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;
use std::thread;

const DEFAULT_SIZE: (i32, i32) = (1120, 720);
const SMALLEST_WINDOW: (i32, i32) = (360, 300);
const TOAST_SECONDS: u32 = 6;
/// The preference that asks for the icon in the tray.
pub const TRAY: &str = "tray";
/// How long after a login started the app its machines start, so what's already running is known.
const LOGIN_START_AFTER_SECONDS: u32 = 3;
const PROBLEM_SECONDS: u32 = 10;
struct Inner {
    context: Arc<Context>,
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    pages: adw::NavigationView,
    library: LibraryPage,
    cards: RefCell<BTreeMap<String, MachineCard>>,
    load_errors: RefCell<Vec<String>>,
    settings: RefCell<Option<SettingsDialog>>,
    new_machine: RefCell<Option<NewMachineDialog>>,
    screen_page: RefCell<Option<ScreenPage>>,
    vm_running: Cell<bool>,
    host_key_down: Cell<bool>,
    refresh: RefCell<RefreshState>,
    screen_watch: OnceCell<Option<Rc<screen_hdr::ScreenWatch>>>,
    /// A newer release, once a check has found one.
    newer: RefCell<Option<String>>,
    /// The icon in the desktop's tray, while the preferences ask for one and the desktop has a
    /// place for it. With it there, closing the window hides it and the app runs on.
    tray: RefCell<Option<Tray>>,
    /// What keeps the app running with its window hidden.
    hold: RefCell<Option<gtk::gio::ApplicationHoldGuard>>,
}

/// A handle to the window, which any page can hold.
#[derive(Clone)]
pub struct Window(Rc<Inner>);

fn saved_size(prefs: &Fields, name: &str, smallest: i32, default: i32) -> i32 {
    prefs.get(name).and_then(Value::as_i64).map_or(default, |size| (size as i32).max(smallest))
}

impl Window {
    pub fn new(application: &adw::Application, context: Arc<Context>) -> Self {
        let prefs = read_prefs(&context);
        let window = adw::ApplicationWindow::builder()
            .application(application)
            .title("Guesthouse")
            .default_width(saved_size(&prefs, "width", SMALLEST_WINDOW.0, DEFAULT_SIZE.0))
            .default_height(saved_size(&prefs, "height", SMALLEST_WINDOW.1, DEFAULT_SIZE.1))
            .maximized(prefs.get("maximized") == Some(&Value::Bool(true)))
            .build();
        let toasts = adw::ToastOverlay::new();
        let pages = adw::NavigationView::new();
        toasts.set_child(Some(&pages));
        window.set_content(Some(&toasts));
        let library = LibraryPage::new(what_machines_are(&context));
        pages.add(&library.page);
        let this = Self(Rc::new(Inner {
            context,
            window,
            toasts,
            pages,
            library,
            cards: RefCell::default(),
            load_errors: RefCell::default(),
            settings: RefCell::default(),
            new_machine: RefCell::default(),
            screen_page: RefCell::default(),
            vm_running: Cell::new(false),
            host_key_down: Cell::new(false),
            refresh: RefCell::default(),
            screen_watch: OnceCell::new(),
            newer: RefCell::default(),
            tray: RefCell::default(),
            hold: RefCell::default(),
        }));
        crate::theme::install(&WidgetExt::display(&this.0.window), &prefs);
        if prefs.get(TRAY) == Some(&Value::Bool(true)) {
            this.set_tray(true, crate::login::started_at_login());
        }
        this.0.library.connect_new_machine(&this);
        this.connect_chosen();
        keys::add_host_keys(&this);
        actions::add_actions(&this);
        this.connect_close();
        this.load_machines();
        this.refresh();
        refresh::start_ticking(&this);
        this
    }

    fn connect_close(&self) {
        let this = self.clone();
        self.0.window.connect_close_request(move |_| {
            if let Some(page) = this.0.screen_page.borrow().as_ref() {
                page.save_thumbnail_now();
            }
            this.remember_size();
            // With the icon in the tray the app stays, and the icon brings the window back.
            if this.has_tray() {
                this.0.window.set_visible(false);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
    }

    pub fn has_tray(&self) -> bool {
        self.0.tray.borrow().is_some()
    }

    /// Puts the app's icon in the tray or takes it out, and says whether it's there now. A desktop
    /// with no tray leaves it out. `waits` is for a run a login started, when the tray may not
    /// be up yet.
    pub fn set_tray(&self, wanted: bool, waits: bool) -> bool {
        if wanted && !self.has_tray() {
            let icons = self.0.context.paths.runtime.join("icons");
            let Some((tray, asks)) = Tray::start(icons, waits) else { return false };
            *self.0.tray.borrow_mut() = Some(tray);
            *self.0.hold.borrow_mut() = self.0.window.application().map(|application| application.hold());
            let this = self.clone();
            glib::spawn_future_local(async move {
                while let Ok(asked) = asks.recv().await {
                    match asked {
                        Asked::Show => this.present(),
                        Asked::Quit => this.quit(),
                    }
                }
            });
        } else if !wanted && self.has_tray() {
            self.drop_tray();
            // Nothing else would bring a hidden window back.
            self.present();
        }
        self.has_tray()
    }

    fn drop_tray(&self) {
        let tray = self.0.tray.borrow_mut().take();
        if let Some(tray) = tray {
            tray.stop();
        }
        self.0.hold.borrow_mut().take();
    }

    /// Ends the app, whether or not its icon would have kept it running. The machines run on.
    pub fn quit(&self) {
        if let Some(page) = self.0.screen_page.borrow().as_ref() {
            page.save_thumbnail_now();
        }
        self.remember_size();
        self.drop_tray();
        if let Some(application) = self.0.window.application() {
            application.quit();
        }
    }

    fn remember_size(&self) {
        let window = &self.0.window;
        if !window.is_fullscreen() {
            let maximized = window.is_maximized();
            let mut fields = json!({"maximized": maximized});
            if !maximized {
                fields["width"] = window.width().into();
                fields["height"] = window.height().into();
            }
            self.remember(fields);
        }
    }

    pub fn context(&self) -> Arc<Context> {
        self.0.context.clone()
    }

    pub fn widget(&self) -> &adw::ApplicationWindow {
        &self.0.window
    }

    pub fn present(&self) {
        self.0.window.present();
    }

    pub fn pages(&self) -> &adw::NavigationView {
        &self.0.pages
    }

    pub fn vm_running(&self) -> bool {
        self.0.vm_running.get()
    }

    fn load_machines(&self) {
        let context = &self.0.context;
        let problem = import_existing(context).err();
        // A copy or an import cut short, as by a power cut, left a hidden folder its size.
        guesthouse_app_core::duplicate::sweep_abandoned(context);
        let (machines, mut errors) = load_all(context);
        for machine in machines {
            self.add_machine(machine);
        }
        errors.extend(problem.map(|failure| failure.0));
        *self.0.load_errors.borrow_mut() = errors;
        self.show_problems();
    }

    /// Makes this the machine the hero shows, and the one the keyboard's shortcuts are for.
    fn choose(&self, card: Option<&MachineCard>) {
        self.0.library.hero().choose(card);
        self.0.library.mark_chosen(card.map(MachineCard::widget));
    }

    /// A card says something about its machine changed, which the hero shows too if it's chosen.
    pub(super) fn card_changed(&self, card: &MachineCard) {
        let hero = self.0.library.hero();
        if hero.shows(card) {
            hero.refresh();
        }
    }

    /// A card has a new picture of its machine's screen, which the hero shows too if it's chosen.
    pub(super) fn card_picture_changed(&self, card: &MachineCard) {
        let hero = self.0.library.hero();
        if hero.shows(card) {
            hero.show_picture();
        }
    }

    fn connect_chosen(&self) {
        let this = self.clone();
        self.0.library.connect_chosen(move |clicked| {
            let card = this.0.cards.borrow().values().find(|card| card.widget() == clicked).cloned();
            this.choose(card.as_ref());
        });
    }

    /// Starts the machines set to start with the PC, once the first look at what's running is in:
    /// one that a login finds already running is left alone.
    pub fn start_machines_for_login(&self) {
        let this = self.clone();
        glib::timeout_add_seconds_local_once(LOGIN_START_AFTER_SECONDS, move || {
            let wanted: Vec<MachineCard> = this
                .0
                .cards
                .borrow()
                .values()
                .filter(|card| card.machine().borrow().start_with_pc)
                .filter(|card| card.shown().is_off() && card.shown().can_start)
                .cloned()
                .collect();
            for card in wanted {
                card.on_start();
            }
        });
    }

    /// The card of the machine with this id, which is where its actions are.
    pub fn card(&self, id: &str) -> Option<MachineCard> {
        self.0.cards.borrow().get(id).cloned()
    }

    fn show_problems(&self) {
        let errors = self.0.load_errors.borrow();
        self.0.library.show(!self.0.cards.borrow().is_empty(), &errors);
    }

    pub fn add_machine(&self, machine: Machine) -> MachineHandle {
        // A look at the library may have found it first.
        if let Some(card) = self.card(&machine.id) {
            return card.machine();
        }
        let id = machine.id.clone();
        let row = MachineCard::new(self, machine);
        let handle = row.machine();
        self.0.library.add(row.widget());
        let first = self.0.library.hero().chosen().is_none();
        self.0.cards.borrow_mut().insert(id, row.clone());
        if first {
            self.choose(Some(&row));
        }
        self.show_problems();
        handle
    }

    /// On a thread, as a disk can take a while to delete.
    pub fn remove(&self, machine: Machine) {
        let context = self.context();
        let working = context.clone();
        let this = self.clone();
        in_background(
            &context,
            move || {
                let owned = machine.owns_files(&working);
                if owned { delete(&working, &machine) } else { remove_entry(&working, &machine) }
                    .map(|()| machine.id)
            },
            move |result| match result {
                Ok(id) => this.on_removed(&id),
                Err(problem) => this.problem(&problem),
            },
        );
    }

    pub(crate) fn on_removed(&self, id: &str) {
        let removed = self.0.cards.borrow_mut().remove(id);
        if let Some(card) = removed {
            self.0.library.remove(card.widget());
            if self.0.library.hero().shows(&card) {
                let next = self.0.cards.borrow().values().next().cloned();
                self.choose(next.as_ref());
            }
        }
        self.show_problems();
    }

    pub fn toast(&self, text: &str, priority: adw::ToastPriority) {
        let toast = adw::Toast::builder()
            .title(glib::markup_escape_text(text).as_str())
            .timeout(TOAST_SECONDS)
            .priority(priority)
            .build();
        self.0.toasts.add_toast(toast);
    }

    /// Tells the user about a failure, and leaves it in the log with the rest of the story.
    pub fn problem(&self, text: &str) {
        // If the log can't be written, the toast still says what matters.
        let _ = write_log(&self.0.context, &format!("app: {text}"));
        let toast = adw::Toast::builder()
            .title(glib::markup_escape_text(text).as_str())
            .timeout(PROBLEM_SECONDS)
            .button_label("See Details")
            .build();
        let this = self.clone();
        toast.connect_button_clicked(move |_| this.show_log());
        self.0.toasts.add_toast(toast);
    }

    /// Starts a task. Nothing waits for it, and a thread only collects it when it ends.
    pub fn launch(&self, action: &str, machine: &Machine, arguments: &[String], slot: Option<Slot>) {
        match launch(&self.0.context, action, machine, arguments, slot) {
            Ok(mut child) => {
                thread::spawn(move || child.wait());
            }
            Err(failure) => self.problem(&failure.0),
        }
        self.refresh();
    }

    pub fn save_machine(&self, machine: &Machine) {
        if let Err(failure) = machine.save(&self.0.context) {
            self.problem(&failure.0);
        }
    }

    pub fn remember(&self, fields: Value) {
        let Value::Object(fields) = fields else { return };
        if let Err(failure) = write_prefs(&self.0.context, fields) {
            let _ = write_log(&self.0.context, &format!("app: could not remember a setting: {failure}"));
        }
    }

    pub fn show_thumbnail(&self, id: &str) {
        if let Some(row) = self.0.cards.borrow().get(id) {
            row.show_thumbnail();
        }
    }

    pub fn show_screen(&self, machine: MachineHandle) {
        actions::allow_host_keys(self, false);
        let fullscreen = machine.borrow().opens_fullscreen && !self.0.window.is_fullscreen();
        let page = ScreenPage::new(self, machine);
        self.0.pages.push(page.widget());
        if fullscreen {
            page.toggle_fullscreen();
        }
        *self.0.screen_page.borrow_mut() = Some(page);
    }

    pub fn screen_closed(&self) {
        *self.0.screen_page.borrow_mut() = None;
        actions::allow_host_keys(self, true);
    }

    /// Once by itself. After that it's in the menu, and New Machine says when it's needed.
    pub fn offer_setup(&self) {
        let context = &self.0.context;
        let offered =
            read_prefs(context).get("setup_offered").is_some_and(|value| value != &Value::Bool(false));
        if gpu_ready(context) && !setup_done(context) && !offered {
            self.remember(json!({"setup_offered": true}));
            self.show_setup();
        }
    }

    pub fn say_if_there_is_an_update(&self) {
        updates::check_at_start(self);
    }

    pub fn check_for_updates(&self) {
        updates::check_now(self);
    }

    pub fn update(&self) {
        updates::ask_to_update(self);
    }

    /// Setup is all about the GPU, so with no GPU support there's nothing it could finish.
    pub fn show_setup(&self) {
        if gpu_ready(&self.0.context) {
            wizard::show(self);
        } else {
            self.problem(NO_GPU_PART);
        }
    }

    /// Turns the open settings to the page of this name, for the page checks.
    pub fn show_settings_page(&self, name: &str) {
        if let Some(settings) = self.0.settings.borrow().as_ref() {
            settings.show_page(name);
        }
    }

    pub fn show_log(&self) {
        log_dialog::show(self);
    }

    pub fn show_new_machine(&self) {
        let dialog = NewMachineDialog::new(self);
        dialog.present();
        *self.0.new_machine.borrow_mut() = Some(dialog);
    }

    pub fn new_machine_closed(&self) {
        *self.0.new_machine.borrow_mut() = None;
    }

    pub fn show_settings(&self, machine: MachineHandle) {
        let id = machine.borrow().id.clone();
        let status = self.0.cards.borrow().get(&id).map(MachineCard::status).unwrap_or_default();
        self.note_screen_hdr();
        let dialog = SettingsDialog::new(self, machine);
        dialog.set_status(&status);
        dialog.present();
        *self.0.settings.borrow_mut() = Some(dialog);
    }

    pub fn settings_closed(&self) {
        *self.0.settings.borrow_mut() = None;
    }

    /// The settings dialog open now, for the page checks.
    pub fn open_settings(&self) -> Option<SettingsDialog> {
        self.0.settings.borrow().clone()
    }

    /// The New Machine dialog open now, for the page checks.
    pub fn open_new_machine(&self) -> Option<NewMachineDialog> {
        self.0.new_machine.borrow().clone()
    }

    /// The screen page open now, for the page checks.
    pub fn open_screen(&self) -> Option<ScreenPage> {
        self.0.screen_page.borrow().clone()
    }
}
