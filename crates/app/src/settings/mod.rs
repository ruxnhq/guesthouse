//! A machine's settings, on a few pages so no one page is a long scroll. Each group of rows is a module
//! of its own, and gets the same three things: the window, the machine, and this dialog to put its own
//! dialogs over.

mod account;
mod disk;
mod display;
mod drives;
mod forwards;
mod gpu_choice;
mod gpu_preview;
mod hardware;
mod jail;
mod linux_gpu;
mod macos;
mod network;
mod nvapi_wrapper;
mod remote_access;
mod remove;
mod sharing;
mod snapshots;
mod starting;
mod token;
mod usb;
mod windows_card;

use crate::window::{MachineHandle, Window};
use adw::prelude::*;
use guesthouse_app_core::machine::Kind;
use guesthouse_app_core::tasks::{State, Status};
use std::rc::Rc;

/// What every group of rows works with.
#[derive(Clone)]
pub struct Shared {
    window: Window,
    machine: MachineHandle,
    dialog: adw::Dialog,
}

impl Shared {
    fn save(&self) {
        self.window.save_machine(&self.machine.borrow());
        self.window.refresh();
    }

    /// Runs a change that can be refused, and shows the refusal.
    fn change(
        &self,
        action: impl FnOnce(&mut guesthouse_app_core::machine::Machine) -> guesthouse_app_core::Result<()>,
    ) {
        if let Err(failure) = action(&mut self.machine.borrow_mut()) {
            self.window.problem(&failure.0);
        }
        self.window.refresh();
    }
}

const WIDTH: i32 = 880;
const HEIGHT: i32 = 660;
const SIDEBAR_WIDTH: f64 = 200.0;
const SMALLEST: (i32, i32) = (360, 300);
/// Under this the list of pages and the page shown take turns, as there's no room for both.
const NARROW: f64 = 620.0;

/// The pages, in the order the list beside them names them.
struct Pages {
    general: adw::PreferencesPage,
    display: adw::PreferencesPage,
    storage: adw::PreferencesPage,
    network: adw::PreferencesPage,
    devices: adw::PreferencesPage,
    list: gtk::ListBox,
}

/// A bar with the dialog's close button over a page of the dialog.
fn under_a_bar(content: &impl IsA<gtk::Widget>) -> adw::ToolbarView {
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(content));
    view
}

impl Pages {
    /// Fills the dialog with a list of its pages down the side and the chosen page beside it.
    fn new(dialog: &adw::Dialog, machine_name: &str, kind: Kind) -> Self {
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
            general: add("general", "General"),
            display: add("display", "Display"),
            storage: add("storage", "Storage"),
            network: add("network", "Network"),
            devices: add("devices", "Devices"),
            list,
        };
        // Ubuntu's network is fixed, so its page would be empty.
        if kind == Kind::Linux
            && let Some(row) = pages.row("network")
        {
            row.set_visible(false);
        }
        let shown = adw::NavigationPage::builder().child(&under_a_bar(&stack)).title("General").build();
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
        pages.list.select_row(pages.row("general").as_ref());
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
    _starting: starting::Starting,
    _token: token::Token,
    _account: account::Account,
    _gpu_preview: gpu_preview::GpuPreview,
    _nvapi_wrapper: nvapi_wrapper::NvapiWrapper,
    _jail: jail::Jail,
    linux_gpu: linux_gpu::LinuxGpu,
    macos: macos::Macos,
    windows_card: windows_card::WindowsCard,
    sharing: sharing::Sharing,
    network: network::Network,
    remote_access: remote_access::RemoteAccess,
    usb: usb::Usb,
    remove: remove::Remove,
    /// The names of the pages, down the dialog's side.
    list: gtk::ListBox,
}

#[derive(Clone)]
pub struct SettingsDialog(Rc<Parts>);

impl SettingsDialog {
    pub fn new(window: &Window, machine: MachineHandle) -> Self {
        let dialog = adw::Dialog::new();
        dialog.set_title(&format!("{} Settings", machine.borrow().name));
        let pages = {
            let machine = machine.borrow();
            Pages::new(&dialog, &machine.name, machine.kind)
        };
        let list = pages.list.clone();
        let shared = Shared { window: window.clone(), machine, dialog };
        let hardware = hardware::Hardware::new(&shared, &pages.general);
        let cap = hardware.cap_row();
        let this = Self(Rc::new(Parts {
            hardware,
            _starting: starting::Starting::new(&shared, &pages.general),
            windows_card: windows_card::WindowsCard::new(&shared, &pages.general, cap),
            _nvapi_wrapper: nvapi_wrapper::NvapiWrapper::new(&shared, &pages.general),
            _gpu_preview: gpu_preview::GpuPreview::new(&shared, &pages.general),
            linux_gpu: linux_gpu::LinuxGpu::new(&shared, &pages.general),
            macos: macos::Macos::new(&shared, &pages.general),
            _account: account::Account::new(&shared, &pages.general),
            _token: token::Token::new(&shared, &pages.general),
            _jail: jail::Jail::new(&shared, &pages.general),
            remove: remove::Remove::new(&shared, &pages.general),
            display: display::Display::new(&shared, &pages.display),
            disk: disk::Disk::new(&shared, &pages.storage),
            drives: drives::Drives::new(&shared, &pages.storage),
            snapshots: snapshots::Snapshots::new(&shared, &pages.storage),
            network: network::Network::new(&shared, &pages.network),
            remote_access: remote_access::RemoteAccess::new(&shared, &pages.network),
            sharing: sharing::Sharing::new(&shared, &pages.devices),
            usb: usb::Usb::new(&shared, &pages.devices),
            shared,
            list,
        }));
        let window = window.clone();
        this.0.shared.dialog.connect_closed(move |_| window.settings_closed());
        this
    }

    pub fn machine_id(&self) -> String {
        self.0.shared.machine.borrow().id.clone()
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

    pub fn set_status(&self, status: &Status) {
        let parts = &self.0;
        parts.hardware.set_status(status);
        parts.display.set_status(status);
        parts.disk.set_status(status);
        parts.drives.set_status(status);
        parts.snapshots.set_status(status);
        parts.sharing.set_status(status);
        parts.network.set_status(status);
        parts.remote_access.set_status(status);
        parts.usb.set_status(status);
        parts.linux_gpu.set_status(status);
        parts.macos.set_status(status);
        parts.windows_card.set_status(status);
        parts.remove.set_status(status);
    }
}
