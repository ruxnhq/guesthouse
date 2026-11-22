//! The AppImage's window. It runs from inside the AppImage before anything is installed: it shows what
//! install.sh would do on this PC, installs on Install, and starts the installed app. The app never runs
//! from the AppImage itself, as the AppImage's folder goes away when it stops and a VM must outlive it.

use crate::application::APP_ID;
use crate::common::in_background;
use adw::prelude::*;
use gtk::{gio, glib};
use guesthouse_app_core::context::Context;
use guesthouse_app_core::log::write_log;
use guesthouse_app_core::payload_install::{Fact, Plan, install, installed_program, plan};
use guesthouse_app_core::settings::Fields;
use std::cell::Cell;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::Arc;

/// Set by the AppImage's AppRun to the release it carries. The app is the installer while it's set.
pub const VARIABLE: &str = "GUESTHOUSE_INSTALL_FROM";
/// What the window exits with when someone would rather install in a terminal. AppRun opens one.
pub const IN_A_TERMINAL: u8 = 75;
/// Set by AppRun when the window runs on the AppImage's own GTK, as the PC has none yet.
const BUNDLED_GTK_VARIABLE: &str = "GUESTHOUSE_BUNDLED_GTK";
/// What AppRun sets for the AppImage's own GTK. Nothing the window starts may have them: install.sh
/// runs the PC's own programs, and the installed app the PC's own GTK.
const BUNDLED_GTK_ONLY: [&str; 5] =
    ["LD_LIBRARY_PATH", "GDK_PIXBUF_MODULE_FILE", "GIO_MODULE_DIR", "GTK_IM_MODULE", BUNDLED_GTK_VARIABLE];
/// The installed app finds its folder by this, as the launcher sets it.
const PROGRAM_VARIABLE: &str = "GUESTHOUSE_PROGRAM";
const APP_PROGRAM: &str = "guesthouse";

const WINDOW_WIDTH: i32 = 620;
const WINDOW_HEIGHT: i32 = 720;
const PAGE_WIDTH: i32 = 560;
const MARGIN: i32 = 24;
const SPACING: i32 = 18;
const ICON_SIZE: i32 = 96;
const SPINNER_SIZE: i32 = 32;

const CHECKING: &str = "checking";
const PLAN: &str = "plan";
const INSTALLING: &str = "installing";
const FAILED: &str = "failed";

#[derive(Clone)]
struct Installer {
    window: adw::ApplicationWindow,
    pages: gtk::Stack,
    plan_page: adw::Bin,
    failed_words: gtk::Label,
    install: gtk::Button,
    context: Arc<Context>,
    payload: Rc<PathBuf>,
    terminal_wanted: Rc<Cell<bool>>,
    /// What the window's children mustn't have.
    keep_out: &'static [&'static str],
}

fn spinner_page(title: &str, words: &str) -> gtk::Box {
    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(SPACING)
        .valign(gtk::Align::Center)
        .margin_start(MARGIN)
        .margin_end(MARGIN)
        .build();
    let spinner = gtk::Spinner::builder()
        .spinning(true)
        .width_request(SPINNER_SIZE)
        .height_request(SPINNER_SIZE)
        .build();
    page.append(&spinner);
    let title = gtk::Label::builder().label(title).wrap(true).justify(gtk::Justification::Center).build();
    title.add_css_class("title-2");
    page.append(&title);
    let words = gtk::Label::builder().label(words).wrap(true).justify(gtk::Justification::Center).build();
    page.append(&words);
    page
}

fn fact_row(fact: &Fact) -> adw::ActionRow {
    let lines: Vec<&str> =
        [fact.value.as_str(), fact.hint.as_str()].into_iter().filter(|line| !line.is_empty()).collect();
    adw::ActionRow::builder()
        .title(&fact.label)
        .subtitle(lines.join("\n"))
        .use_markup(false)
        .subtitle_selectable(true)
        .build()
}

fn group(title: &str) -> adw::PreferencesGroup {
    adw::PreferencesGroup::builder().title(title).build()
}

/// Root's part: why it's needed, and the exact command, which the password window only names as `sh`.
fn root_group(plan: &Plan) -> adw::PreferencesGroup {
    let root = group("Root");
    let Some(command) = &plan.root_command else {
        let row = adw::ActionRow::builder().title("Nothing here needs your password").build();
        root.add(&row);
        for note in &plan.root_notes {
            root.add(&adw::ActionRow::builder().title(note).use_markup(false).title_selectable(true).build());
        }
        return root;
    };
    root.set_description(Some(
        "A window asks for your password once, then this one command runs as root. It's the same command a \
         terminal install would show.",
    ));
    for note in &plan.root_notes {
        root.add(&adw::ActionRow::builder().title(note).use_markup(false).build());
    }
    let shown = gtk::Label::builder()
        .label(command)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .selectable(true)
        .xalign(0.0)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    shown.add_css_class("monospace");
    let expander = adw::ExpanderRow::builder().title("The command").build();
    expander.add_row(&shown);
    root.add(&expander);
    root
}

fn plan_page(plan: &Plan) -> gtk::ScrolledWindow {
    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(SPACING)
        .margin_top(MARGIN)
        .margin_bottom(MARGIN)
        .margin_start(MARGIN)
        .margin_end(MARGIN)
        .build();
    column.append(&gtk::Image::builder().icon_name(APP_ID).pixel_size(ICON_SIZE).build());
    let title =
        gtk::Label::builder().label(plan.title()).wrap(true).justify(gtk::Justification::Center).build();
    title.add_css_class("title-1");
    column.append(&title);
    let places = group("What goes where");
    places
        .set_description(Some("All of it in your own folders. Your machines and settings stay as they are."));
    for place in &plan.places {
        places.add(&fact_row(place));
    }
    column.append(&places);
    column.append(&root_group(plan));
    let afterwards = group("Afterwards");
    afterwards.add(
        &adw::ActionRow::builder()
            .title("This installer isn't needed any more")
            .subtitle(
                "Guesthouse will be in your apps, and it updates itself from there. You can delete this file.",
            )
            .build(),
    );
    column.append(&afterwards);
    if !plan.pc.is_empty() {
        let pc = group("This PC");
        let expander = adw::ExpanderRow::builder().title("What was checked").build();
        for fact in &plan.pc {
            expander.add_row(&fact_row(fact));
        }
        pc.add(&expander);
        column.append(&pc);
    }
    let clamp = adw::Clamp::builder().maximum_size(PAGE_WIDTH).child(&column).build();
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&clamp)
        .vexpand(true)
        .build()
}

fn failed_page(words: &gtk::Label, again: &gtk::Button) -> gtk::Box {
    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(SPACING)
        .valign(gtk::Align::Center)
        .margin_start(MARGIN)
        .margin_end(MARGIN)
        .build();
    page.append(&gtk::Image::builder().icon_name("dialog-error-symbolic").pixel_size(ICON_SIZE / 2).build());
    let title = gtk::Label::builder().label("Nothing was installed").build();
    title.add_css_class("title-2");
    page.append(&title);
    page.append(words);
    again.set_halign(gtk::Align::Center);
    page.append(again);
    page
}

impl Installer {
    fn new(
        application: &adw::Application,
        context: Arc<Context>,
        payload: PathBuf,
        terminal_wanted: Rc<Cell<bool>>,
    ) -> Self {
        let pages = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build();
        pages.add_named(&spinner_page("Checking this PC", "Nothing changes yet."), Some(CHECKING));
        let plan_page = adw::Bin::new();
        pages.add_named(&plan_page, Some(PLAN));
        pages.add_named(
            &spinner_page(
                "Installing Guesthouse",
                "This takes a minute. If a part needs root, a window asks for your password.",
            ),
            Some(INSTALLING),
        );
        let failed_words = gtk::Label::builder()
            .wrap(true)
            .selectable(true)
            .justify(gtk::Justification::Center)
            .max_width_chars(60)
            .build();
        let again = gtk::Button::with_label("Try Again");
        again.add_css_class("pill");
        pages.add_named(&failed_page(&failed_words, &again), Some(FAILED));

        let install = gtk::Button::with_label("Install");
        install.add_css_class("suggested-action");
        install.add_css_class("pill");
        let terminal = gtk::Button::with_label("Use a Terminal");
        terminal.set_tooltip_text(Some("Install in a terminal instead, where sudo asks for your password"));
        terminal.add_css_class("flat");
        let buttons = gtk::Box::builder()
            .spacing(12)
            .halign(gtk::Align::Center)
            .margin_top(12)
            .margin_bottom(12)
            .build();
        buttons.append(&terminal);
        buttons.append(&install);

        let header = adw::HeaderBar::new();
        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&pages));
        view.add_bottom_bar(&buttons);
        let window = adw::ApplicationWindow::builder()
            .application(application)
            .title("Install Guesthouse")
            .default_width(WINDOW_WIDTH)
            .default_height(WINDOW_HEIGHT)
            .content(&view)
            .build();
        let this = Self {
            window,
            pages,
            plan_page,
            failed_words,
            install: install.clone(),
            context,
            payload: Rc::new(payload),
            terminal_wanted,
            keep_out: if std::env::var_os(BUNDLED_GTK_VARIABLE).is_some() { &BUNDLED_GTK_ONLY } else { &[] },
        };
        let installing = this.clone();
        install.connect_clicked(move |_| installing.run_install());
        let checking = this.clone();
        again.connect_clicked(move |_| checking.check());
        let leaving = this.clone();
        terminal.connect_clicked(move |_| {
            leaving.terminal_wanted.set(true);
            leaving.window.close();
        });
        this
    }

    fn show(&self, page: &str) {
        self.pages.set_visible_child_name(page);
        self.install.set_sensitive(page == PLAN);
    }

    fn fail(&self, words: &str) {
        self.failed_words.set_label(words);
        self.show(FAILED);
    }

    fn check(&self) {
        self.show(CHECKING);
        let asking = self.payload.to_path_buf();
        let keep_out = self.keep_out;
        let this = self.clone();
        in_background(
            &self.context,
            move || plan(&asking, std::process::id(), keep_out),
            move |found| match found {
                Ok(plan) => {
                    this.plan_page.set_child(Some(&plan_page(&plan)));
                    this.show(PLAN);
                }
                Err(words) => this.fail(&words),
            },
        );
    }

    fn run_install(&self) {
        self.show(INSTALLING);
        self.window.set_deletable(false);
        let payload = self.payload.to_path_buf();
        let keep_out = self.keep_out;
        let context = self.context.clone();
        let this = self.clone();
        in_background(
            &self.context,
            move || {
                let said = install(&payload, std::process::id(), keep_out);
                let words = match &said {
                    Ok(said) => said.clone(),
                    Err(failure) => failure.0.clone(),
                };
                let _ = write_log(&context, &format!("installer: install.sh said:\n{words}"));
                said
            },
            move |result| {
                this.window.set_deletable(true);
                match result {
                    Ok(_) => this.start_installed_app(),
                    Err(words) => this.fail(&words),
                }
            },
        );
    }

    /// Started in a process group of its own, so it stays when this one and the AppImage are gone.
    fn start_installed_app(&self) {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        let folder = installed_program(&home);
        let mut app = Command::new(folder.join(APP_PROGRAM));
        for variable in self.keep_out {
            app.env_remove(variable);
        }
        let started = app
            .env(PROGRAM_VARIABLE, &folder)
            .env_remove(VARIABLE)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn();
        if started.is_err() {
            self.fail("Guesthouse is installed, but it couldn't start. Open it from your apps.");
            return;
        }
        self.window.close();
    }
}

/// The installer for the release in `payload`. It exits with [`IN_A_TERMINAL`] when someone chose a
/// terminal instead.
pub fn run(payload: &Path) -> glib::ExitCode {
    let context = match Context::from_process() {
        Ok(context) => Arc::new(context),
        Err(error) => {
            eprintln!("{error}");
            return glib::ExitCode::FAILURE;
        }
    };
    // Its own instance, so an installed Guesthouse that's running isn't handed this window.
    let application =
        adw::Application::builder().application_id(APP_ID).flags(gio::ApplicationFlags::NON_UNIQUE).build();
    let terminal_wanted: Rc<Cell<bool>> = Rc::default();
    let wanted = terminal_wanted.clone();
    let payload = payload.to_path_buf();
    let icons = context.paths.runtime.join("icons");
    application.connect_startup(move |_| crate::application::use_built_in_icons(&icons));
    application.connect_activate(move |application| {
        let installer = Installer::new(application, context.clone(), payload.clone(), wanted.clone());
        crate::theme::install(&WidgetExt::display(&installer.window), &Fields::default());
        installer.window.present();
        installer.check();
    });
    let code = application.run_with_args::<&str>(&[]);
    if terminal_wanted.get() { glib::ExitCode::from(IN_A_TERMINAL) } else { code }
}
