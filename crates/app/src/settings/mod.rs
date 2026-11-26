//! A machine's settings, on a few pages so no one page is a long scroll: an overview of what most people
//! look at, a page for each topic, and an Advanced page for switches that only matter to someone who knows
//! why they want them. Each group of rows is a module of its own, and gets the same three things: the
//! window, the machine, and this dialog to put its own dialogs over.
//!
//! The same pages can show a machine that isn't made yet, for New Machine: see [`SettingsDialog::draft`].
//! A draft's changes stay in the `Machine` value, nothing is saved or run, and what needs a machine that
//! exists (its disk, snapshots, copies, the guest) isn't shown.

mod account;
mod android;
mod apps;
mod copies;
mod disk;
mod display;
mod drives;
mod forwards;
mod gpu_choice;
mod gpu_preview;
mod hardware;
mod jail;
mod lean;
mod linux_gpu;
mod macos;
mod network;
mod nvapi_wrapper;
mod recipe;
mod remote_access;
pub mod remote_view;
mod remove;
mod scan_exclusions;
mod sharing;
mod snapshots;
mod starting;
mod token;
mod usb;
mod windows_card;

use crate::rows::info_button;
use crate::window::{MachineHandle, Window};
use adw::prelude::*;
use guesthouse_app_core::context::Context;
use guesthouse_app_core::machine::{Machine, valid_id};
use guesthouse_app_core::remote_access::Access;
use guesthouse_app_core::tasks::{State, Status};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// A machine that isn't made yet: its changes stay in memory, and what Guesthouse's own setters write
/// goes to a scratch folder instead of the store.
struct Draft {
    context: Arc<Context>,
    /// Remote Desktop and SSH, which a made machine keeps beside its settings.
    access: RefCell<Access>,
    changed: RefCell<Vec<Box<dyn Fn()>>>,
}

/// What every group of rows works with.
#[derive(Clone)]
pub struct Shared {
    window: Window,
    machine: MachineHandle,
    dialog: adw::Dialog,
    draft: Option<Rc<Draft>>,
}

impl Shared {
    /// Whether this is a machine New Machine hasn't made yet.
    fn is_draft(&self) -> bool {
        self.draft.is_some()
    }

    fn save(&self) {
        if let Some(draft) = &self.draft {
            draft.changed.borrow().iter().for_each(|changed| changed());
            return;
        }
        self.window.save_machine(&self.machine.borrow());
        self.window.refresh();
    }

    /// Runs a change that can be refused, and shows the refusal. A draft's is checked the same way, and
    /// what the change saves lands in the scratch folder.
    fn change(&self, action: impl FnOnce(&Context, &mut Machine) -> guesthouse_app_core::Result<()>) {
        if let Err(failure) = self.attempt(action) {
            self.window.problem(&failure.0);
        }
        if self.is_draft() {
            self.save();
        } else {
            self.window.refresh();
        }
    }

    /// Runs a change that can be refused. A draft's runs on a copy whose name, id and account can't be
    /// what stops it, as New Machine may not have them yet, and the copy is kept only if it's taken.
    fn attempt(
        &self,
        action: impl FnOnce(&Context, &mut Machine) -> guesthouse_app_core::Result<()>,
    ) -> guesthouse_app_core::Result<()> {
        let Some(draft) = &self.draft else {
            return action(&self.window.context(), &mut self.machine.borrow_mut());
        };
        let mut trial = self.machine.borrow().clone();
        if !valid_id(&trial.id) {
            trial.id = DRAFT_ID.to_owned();
        }
        if trial.name.trim().is_empty() {
            trial.name = DRAFT_ID.to_owned();
        }
        trial.user.clear();
        action(&draft.context, &mut trial)?;
        let mut machine = self.machine.borrow_mut();
        let (id, name, user) = (
            std::mem::take(&mut machine.id),
            std::mem::take(&mut machine.name),
            std::mem::take(&mut machine.user),
        );
        *machine = Machine { id, name, user, ..trial };
        Ok(())
    }
}

/// What a draft is called while a change is tried on it, if it has no name or id yet.
const DRAFT_ID: &str = "draft";

/// A command to copy, in a monospace card wide enough that it isn't broken mid-word, as a recipe's is.
fn command_card(text: &str) -> gtk::ScrolledWindow {
    let view = gtk::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .top_margin(COMMAND_MARGIN)
        .bottom_margin(COMMAND_MARGIN)
        .left_margin(COMMAND_MARGIN)
        .right_margin(COMMAND_MARGIN)
        .wrap_mode(gtk::WrapMode::WordChar)
        .build();
    view.buffer().set_text(text);
    let card = gtk::ScrolledWindow::builder()
        .child(&view)
        .min_content_width(COMMAND_WIDTH)
        .max_content_height(COMMAND_MOST_HEIGHT)
        .propagate_natural_height(true)
        .build();
    card.add_css_class("card");
    card
}

const COMMAND_WIDTH: i32 = 480;
const COMMAND_MOST_HEIGHT: i32 = 240;
const COMMAND_MARGIN: i32 = 12;

const WIDTH: i32 = 880;
const HEIGHT: i32 = 660;
const SIDEBAR_WIDTH: f64 = 200.0;
const SMALLEST: (i32, i32) = (360, 300);
/// Under this the list of pages and the page shown take turns, as there's no room for both.
const NARROW: f64 = 620.0;
const FIRST_PAGE: &str = "overview";
const HEADER_SPACING: i32 = 6;

/// The groups of the Advanced page, which the modules put their expert rows in.
pub struct Advanced {
    hardware: adw::PreferencesGroup,
    graphics: adw::PreferencesGroup,
    network: adw::PreferencesGroup,
    security: adw::PreferencesGroup,
}

/// The groups of the Overview page, in the order they show.
struct Overview {
    /// Cores, RAM, the disk, sound and starting.
    figures: adw::PreferencesGroup,
    gpu: adw::PreferencesGroup,
    guest: adw::PreferencesGroup,
    actions: adw::PreferencesGroup,
}

/// The pages, in the order the list beside them names them.
struct Pages {
    overview: adw::PreferencesPage,
    display: adw::PreferencesPage,
    storage: adw::PreferencesPage,
    network: adw::PreferencesPage,
    devices: adw::PreferencesPage,
    apps: adw::PreferencesPage,
    advanced: adw::PreferencesPage,
    list: gtk::ListBox,
}

/// A bar with the dialog's close button over a page of the dialog.
fn under_a_bar(content: &impl IsA<gtk::Widget>) -> adw::ToolbarView {
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(content));
    view
}

/// A group's header end: an info button with its longer explanation, then its button.
fn header_with(text: &str, button: &impl IsA<gtk::Widget>) -> gtk::Box {
    let header = gtk::Box::builder().spacing(HEADER_SPACING).valign(gtk::Align::Center).build();
    header.append(&info_button(text));
    header.append(button);
    header
}

/// Shows a choice row's value as its subtitle rather than at its end, where a long one, a card's name
/// say, gets cut off. Called once the row has its choice: the row only fills the subtitle in when the
/// choice changes.
fn value_as_subtitle(row: &adw::ComboRow) {
    row.set_use_subtitle(true);
    if let Some(chosen) = row.selected_item().and_downcast::<gtk::StringObject>() {
        row.set_subtitle(&chosen.string());
    }
}

/// Says a text that changes in a row's subtitle and its info button: its first sentence in the one, and
/// all of it in the other.
fn first_sentence_and_info(row: &impl IsA<adw::ActionRow>, info: &gtk::MenuButton, text: &str) {
    let first = text.split_once(". ").map_or(text, |(first, _)| first).trim_end_matches('.');
    row.as_ref().set_subtitle(&gtk::glib::markup_escape_text(first));
    if let Some(label) = info.popover().and_then(|popover| popover.child()).and_downcast::<gtk::Label>() {
        label.set_label(text);
    }
}

/// Whether anything under this widget is a row that shows.
fn shows_rows(widget: &gtk::Widget) -> bool {
    if widget.is::<adw::PreferencesRow>() {
        return widget.is_visible();
    }
    if !widget.is_visible() {
        return false;
    }
    let mut child = widget.first_child();
    while let Some(found) = child {
        if shows_rows(&found) {
            return true;
        }
        child = found.next_sibling();
    }
    false
}

/// Whether anything under this widget is a group that shows.
fn shows_groups(widget: &gtk::Widget) -> bool {
    if !widget.is_visible() {
        return false;
    }
    if widget.is::<adw::PreferencesGroup>() {
        return true;
    }
    let mut child = widget.first_child();
    while let Some(found) = child {
        if shows_groups(&found) {
            return true;
        }
        child = found.next_sibling();
    }
    false
}

/// Hides a group that has nothing for this kind of machine.
fn hide_if_empty(group: &adw::PreferencesGroup) {
    group.set_visible(shows_rows(group.upcast_ref()));
}

fn group(page: &adw::PreferencesPage, title: &str) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder().title(title).build();
    page.add(&group);
    group
}

impl Pages {
    /// Fills the dialog with a list of its pages down the side and the chosen page beside it.
    fn new(dialog: &adw::Dialog, machine_name: &str) -> Self {
        let stack = gtk::Stack::new();
        let list = gtk::ListBox::new();
        list.add_css_class("navigation-sidebar");
        let add = |name: &str, title: &str| {
            let page = adw::PreferencesPage::builder().name(name).title(title).build();
            stack.add_named(&page, Some(name));
            let row = gtk::ListBoxRow::builder()
                .name(name)
                .child(&gtk::Label::builder().label(title).xalign(0.0).build())
                .build();
            list.append(&row);
            page
        };
        let pages = Self {
            overview: add(FIRST_PAGE, "Overview"),
            display: add("display", "Display"),
            storage: add("storage", "Storage"),
            network: add("network", "Network"),
            devices: add("devices", "Devices"),
            apps: add("apps", "Apps"),
            advanced: add("advanced", "Advanced"),
            list,
        };
        let shown = adw::NavigationPage::builder().child(&under_a_bar(&stack)).title("Overview").build();
        let listed = adw::NavigationPage::builder()
            .child(&under_a_bar(&gtk::ScrolledWindow::builder().child(&pages.list).build()))
            .title(machine_name)
            .build();
        let split = adw::NavigationSplitView::builder()
            .sidebar(&listed)
            .content(&shown)
            .min_sidebar_width(SIDEBAR_WIDTH)
            .max_sidebar_width(SIDEBAR_WIDTH)
            .build();
        let (showing, chosen) = (split.clone(), shown.clone());
        pages.list.connect_row_selected(move |_, row| {
            let Some(row) = row else { return };
            let name = row.widget_name();
            stack.set_visible_child_name(&name);
            if let Some(page) = stack.visible_child().and_downcast::<adw::PreferencesPage>() {
                chosen.set_title(&page.title());
            }
        });
        // In a narrow window a click on a page's name is what brings the page in.
        pages.list.connect_row_activated(move |_, _| showing.set_show_content(true));
        pages.list.select_row(pages.row(FIRST_PAGE).as_ref());
        let narrow = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth,
            NARROW,
            adw::LengthUnit::Sp,
        ));
        narrow.add_setter(&split, "collapsed", Some(&true.to_value()));
        dialog.add_breakpoint(narrow);
        dialog.set_child(Some(&split));
        dialog.set_content_width(WIDTH);
        dialog.set_content_height(HEIGHT);
        // A dialog whose layout changes with its width has to say how small it can get.
        dialog.set_width_request(SMALLEST.0);
        dialog.set_height_request(SMALLEST.1);
        pages
    }

    fn overview(&self, machine_name: &str) -> Overview {
        Overview {
            figures: group(&self.overview, machine_name),
            gpu: group(&self.overview, "GPU"),
            guest: group(&self.overview, "Guest"),
            actions: group(&self.overview, "Machine"),
        }
    }

    fn advanced(&self) -> Advanced {
        Advanced {
            hardware: group(&self.advanced, "Processor and Memory"),
            graphics: group(&self.advanced, "Graphics"),
            network: group(&self.advanced, "Network"),
            security: group(&self.advanced, "Security"),
        }
    }

    fn row(&self, name: &str) -> Option<gtk::ListBoxRow> {
        let mut row = self.list.first_child();
        while let Some(found) = row {
            if found.widget_name() == name {
                return found.downcast().ok();
            }
            row = found.next_sibling();
        }
        None
    }

    /// Hides the name of a page with nothing on it for this kind of machine: Ubuntu's network is fixed,
    /// so its page would be empty.
    /// Counts groups and not rows, as some groups fill their rows in later.
    fn hide_if_empty(&self, page: &adw::PreferencesPage, name: &str) {
        if !shows_groups(page.upcast_ref())
            && let Some(row) = self.row(name)
        {
            row.set_visible(false);
        }
    }
}

/// The Remote View group: a machine that exists and is Windows has it.
fn remote_view_group(shared: &Shared, page: &adw::PreferencesPage) -> Option<remote_view::RemoteView> {
    let machine = shared.machine.borrow().clone();
    if shared.is_draft() || machine.kind != guesthouse_app_core::machine::Kind::Windows {
        return None;
    }
    let window = shared.window.clone();
    let backend = Arc::new(remote_view::CoreBackend::new(shared.window.context(), &machine.id));
    Some(remote_view::RemoteView::new(page, backend, move |text| window.problem(text)))
}

/// Whether the VM is off, with nothing going on.
fn is_off(status: &Status) -> bool {
    status.state == State::Off && !status.busy
}

struct Parts {
    shared: Shared,
    hardware: hardware::Hardware,
    display: display::Display,
    disk: disk::Disk,
    drives: drives::Drives,
    snapshots: snapshots::Snapshots,
    apps: Option<apps::Apps>,
    _starting: starting::Starting,
    _token: token::Token,
    _account: account::Account,
    _gpu_preview: gpu_preview::GpuPreview,
    _nvapi_wrapper: nvapi_wrapper::NvapiWrapper,
    _jail: jail::Jail,
    linux_gpu: linux_gpu::LinuxGpu,
    macos: macos::Macos,
    android: android::Android,
    windows_card: windows_card::WindowsCard,
    sharing: sharing::Sharing,
    network: network::Network,
    remote_access: remote_access::RemoteAccess,
    remote_view: Option<remote_view::RemoteView>,
    /// Only a machine made for a recipe has one.
    recipe: Option<recipe::Recipe>,
    /// Only a machine made lean has one.
    lean: Option<lean::Lean>,
    /// Only a machine that leaves folders out of Defender's scanning has one.
    scan_exclusions: Option<scan_exclusions::ScanExclusions>,
    usb: usb::Usb,
    copies: copies::Copies,
    remove: remove::Remove,
    /// The names of the pages, down the dialog's side.
    list: gtk::ListBox,
}

#[derive(Clone)]
pub struct SettingsDialog(Rc<Parts>);

impl SettingsDialog {
    pub fn new(window: &Window, machine: MachineHandle) -> Self {
        let this = Self::build(window, machine, None);
        let window = window.clone();
        this.0.shared.dialog.connect_closed(move |_| window.settings_closed());
        this
    }

    /// The settings of a machine New Machine hasn't made yet. Changes stay in the `Machine` value, read
    /// back with [`Self::machine`], and Remote Desktop and SSH with [`Self::access`]. Nothing is saved
    /// and nothing runs, and what needs a made machine isn't shown. Show it with [`Self::present_over`].
    pub fn draft(window: &Window, machine: Machine, access: Access) -> Self {
        let real = window.context();
        let scratch = real.paths.runtime.join(format!("settings-draft-{}", std::process::id()));
        let mut context = (*real).clone();
        context.paths.machines.clone_from(&scratch);
        let draft =
            Draft { context: Arc::new(context), access: RefCell::new(access), changed: RefCell::default() };
        let this = Self::build(window, Rc::new(RefCell::new(machine)), Some(Rc::new(draft)));
        this.set_status(&Status::default());
        this.0.shared.dialog.connect_closed(move |_| {
            let _ = std::fs::remove_dir_all(&scratch);
        });
        this
    }

    fn build(window: &Window, machine: MachineHandle, draft: Option<Rc<Draft>>) -> Self {
        let dialog = adw::Dialog::new();
        let name = machine.borrow().name.clone();
        dialog.set_title(&format!("{name} Settings"));
        let pages = Pages::new(&dialog, &name);
        let list = pages.list.clone();
        let shared = Shared { window: window.clone(), machine, dialog, draft };
        // Built in the order they show on each page. The token comes first, as the app can't reach the
        // guest until it's chosen.
        let token = token::Token::new(&shared, &pages.overview);
        let overview = pages.overview(&name);
        let advanced = pages.advanced();
        let hardware = hardware::Hardware::new(&shared, &overview.figures, &advanced.hardware);
        let disk = disk::Disk::new(&shared, &overview.figures, &pages.storage);
        overview.figures.add(hardware.sound_row());
        let starting = starting::Starting::new(&shared, &overview.figures);
        let windows_card = windows_card::WindowsCard::new(&shared, &overview.gpu, hardware.cap_row());
        let gpu_preview = gpu_preview::GpuPreview::new(&shared, &overview.gpu);
        let linux_gpu = linux_gpu::LinuxGpu::new(&shared, &overview.gpu);
        let macos = macos::Macos::new(&shared, &overview.gpu);
        let android = android::Android::new(&shared, &overview.gpu);
        let account = account::Account::new(&shared, &overview.guest);
        let recipe = recipe::Recipe::new(&shared, &overview.guest);
        let lean = lean::Lean::new(&shared, &overview.guest);
        let scan_exclusions = scan_exclusions::ScanExclusions::new(&shared, &overview.guest);
        let copies = copies::Copies::new(&shared, &overview.actions);
        let remove = remove::Remove::new(&shared, &overview.actions);
        let display = display::Display::new(&shared, &pages.display, &advanced.graphics);
        let nvapi_wrapper = nvapi_wrapper::NvapiWrapper::new(&shared, &advanced.graphics);
        let drives = drives::Drives::new(&shared, &pages.storage);
        let snapshots = snapshots::Snapshots::new(&shared, &pages.storage);
        let network = network::Network::new(&shared, &pages.network, &advanced.network);
        let remote_access = remote_access::RemoteAccess::new(&shared, &pages.network);
        let remote_view = remote_view_group(&shared, &pages.network);
        let sharing = sharing::Sharing::new(&shared, &pages.devices);
        let usb = usb::Usb::new(&shared, &pages.devices);
        let apps = apps::Apps::new(&shared, &pages.apps);
        let jail = jail::Jail::new(&shared, &advanced.security);
        for group in [&overview.gpu, &overview.guest, &overview.actions] {
            hide_if_empty(group);
        }
        for group in [&advanced.hardware, &advanced.graphics, &advanced.network, &advanced.security] {
            hide_if_empty(group);
        }
        let named = [
            (&pages.display, "display"),
            (&pages.storage, "storage"),
            (&pages.network, "network"),
            (&pages.devices, "devices"),
            (&pages.apps, "apps"),
            (&pages.advanced, "advanced"),
        ];
        for (page, name) in named {
            pages.hide_if_empty(page, name);
        }
        Self(Rc::new(Parts {
            hardware,
            _starting: starting,
            windows_card,
            _nvapi_wrapper: nvapi_wrapper,
            _gpu_preview: gpu_preview,
            linux_gpu,
            macos,
            android,
            _account: account,
            _token: token,
            _jail: jail,
            recipe,
            copies,
            lean,
            scan_exclusions,
            remove,
            display,
            disk,
            drives,
            snapshots,
            apps,
            network,
            remote_access,
            remote_view,
            sharing,
            usb,
            shared,
            list,
        }))
    }

    pub fn machine_id(&self) -> String {
        self.0.shared.machine.borrow().id.clone()
    }

    /// The machine as the pages have it now, with a draft's changes.
    pub fn machine(&self) -> Machine {
        self.0.shared.machine.borrow().clone()
    }

    /// A draft's Remote Desktop and SSH as chosen on the Network page. A made machine keeps its own in
    /// its folder, so it has none here.
    pub fn access(&self) -> Option<Access> {
        self.0.shared.draft.as_ref().map(|draft| *draft.access.borrow())
    }

    /// Called after each change a draft takes, while the dialog is open.
    pub fn connect_changed(&self, changed: impl Fn() + 'static) {
        if let Some(draft) = &self.0.shared.draft {
            draft.changed.borrow_mut().push(Box::new(changed));
        }
    }

    /// The dialog itself, for its `closed` signal.
    pub fn dialog(&self) -> &adw::Dialog {
        &self.0.shared.dialog
    }

    /// Shows the page of this name, as a click on it in the list does.
    pub fn show_page(&self, name: &str) {
        let mut row = self.0.list.first_child();
        while let Some(found) = row {
            if found.widget_name() == name {
                self.0.list.select_row(found.downcast_ref::<gtk::ListBoxRow>());
            }
            row = found.next_sibling();
        }
    }

    pub fn present(&self) {
        self.0.shared.dialog.present(Some(self.0.shared.window.widget()));
    }

    /// Shows the dialog over another, as New Machine shows a draft's.
    pub fn present_over(&self, parent: &impl IsA<gtk::Widget>) {
        self.0.shared.dialog.present(Some(parent));
    }

    pub fn set_status(&self, status: &Status) {
        let parts = &self.0;
        parts.hardware.set_status(status);
        parts.display.set_status(status);
        parts.disk.set_status(status);
        parts.drives.set_status(status);
        parts.snapshots.set_status(status);
        if let Some(apps) = &parts.apps {
            apps.set_status(status);
        }
        parts.sharing.set_status(status);
        parts.network.set_status(status);
        parts.remote_access.set_status(status);
        if let Some(view) = &parts.remote_view {
            view.set_running(status.state == State::Running);
        }
        if let Some(lean) = &parts.lean {
            lean.set_status(status);
        }
        if let Some(skipped) = &parts.scan_exclusions {
            skipped.set_status(status);
        }
        if let Some(recipe) = &parts.recipe {
            recipe.set_status(status);
        }
        parts.usb.set_status(status);
        parts.linux_gpu.set_status(status);
        parts.macos.set_status(status);
        parts.android.set_status(status);
        parts.windows_card.set_status(status);
        parts.copies.set_status(status);
        parts.remove.set_status(status);
    }
}
