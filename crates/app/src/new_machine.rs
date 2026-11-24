//! The dialog that makes a machine from an ISO. Looking inside the ISO decides what it will be. A
//! macOS machine has no ISO to look into: its installer comes from Apple.

use crate::common::{choose_file, in_background};
use crate::disk_place_row::DiskPlaceRow;
use crate::driver_row::DriverRow;
use crate::recipe_dialog;
use crate::rows::{bottom_bar, choice_row, number_row, page_heading, row_button};
use crate::window::Window;
use adw::prelude::*;
use guesthouse_app_core::account::{default_user, name_problem, password_problem};
use guesthouse_app_core::card_choice::{CardChoice, default_choice, windows_card_choices};
use guesthouse_app_core::defaults::{NO_GPU, account_name, sizes_for_disc, suggested_name};
use guesthouse_app_core::display::DriverPackage;
use guesthouse_app_core::gpu_maker::GpuMaker;
use guesthouse_app_core::gpu_support::{kind_offered, plain_reason, read_disc, ubuntu_note};
use guesthouse_app_core::host::{RamField, physical_cores};
use guesthouse_app_core::iso::Iso;
use guesthouse_app_core::lean::what_it_does;
use guesthouse_app_core::library::{
    Extras, MAX_DISK_GIB, MIN_PLAIN_DISK_GIB, MIN_WINDOWS_DISK_GIB, NewMachine, create_with,
};
use guesthouse_app_core::machine::{Kind, MIN_CORES, MIN_RAM_GIB, Machine};
use guesthouse_app_core::macos_setup::{
    MACOS_SIZES, MACOS_TITLE, MIN_MACOS_DISK_GIB, NewMac, create_macos, how_it_installs,
};
use guesthouse_app_core::recipe::{
    Chosen, Entry, SettingsAsked, Source, Usual, fit, list_for, origin_text, what_it_will_run,
};
use guesthouse_app_core::remote_access::{Access, Service, ssh_problem};
use guesthouse_app_core::scan_exclusions::{self, WHAT_IT_MEANS};
use guesthouse_app_core::settings::max_vram_gib_for;
use guesthouse_app_core::setup::amd::NO_AMD_SETUP;
use guesthouse_app_core::setup::checks::machine_gpu_warning;
use guesthouse_app_core::setup::guest_driver::guest_driver;
use guesthouse_app_core::setup::intel::NO_INTEL_SETUP;
use guesthouse_app_core::tasks::no_slot_text;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const DISPLAY_DRIVER_NOTE: &str =
    "Guesthouse adds its own display driver to Windows, signed with a certificate made on this machine.";
const WINDOWS_ACCOUNT_NOTE: &str = "Windows signs in by itself. The password is for permission prompts, Remote \
                                    Desktop and SSH. Leave it empty for a random one, kept in the machine's \
                                    folder.";
const LINUX_ACCOUNT_NOTE: &str = "For a console login if the desktop won't start. Leave the password empty for a \
                                  random one, kept in the machine's folder.";
const DISK_NOTE: &str = "The most it can hold. Its file starts small and grows as the guest fills it.";
const ACCESS_NOTE: &str = "Only a port forward from this PC reaches them, whatever the network. The machine's \
                           settings can change them later.";
const REMOTE_DESKTOP_NOTE: &str = "Windows' own, on guest port 3389. Sign in with the account above.";
const SSH_NOTE: &str = "OpenSSH Server on guest port 22, for the account above. Your public keys from ~/.ssh go \
                        in too.";
const NO_RECIPE: &str = "Nothing extra";
const LEAN_TITLE: &str = "Lean Windows";
const SKIP_SCANNING_TITLE: &str = "Leave Folders Out of Virus Scanning";
const LEAN_NOTE: &str = "Leaves out apps Windows adds for every account, and keeps some programs from running in \
                         the background. Show lists all of it, and what it leaves alone.";
const RECIPE_NOTE: &str = "A recipe sets a few of the machine's settings and gives it software once it is installed. \
                           Nothing runs in the machine until it is.";
const WIDTH: i32 = 520;
const HEIGHT: i32 = 720;
const NOTE_MARGIN: i32 = 12;
const CARD_TITLE: &str = "Card";
const MACOS_ROW_NOTE: &str = "Needs no ISO. Apple's installer is downloaded for it, and macOS \
                              support the first time.";
const PREVIEW: &str = "Preview";
const MACOS_NAME: &str = "macOS";
/// Said plainly until a guest on an AMD card has drawn with it.
const AMD_NOT_FINISHED: &str = "starts on the AMD card. GPU-PV on AMD is not finished, so Windows may have no \
                                GPU there yet";
/// Said plainly, as one card has drawn Windows' desktop and the rest are untried.
const INTEL_NOT_FINISHED: &str = "starts on the Intel card. Intel support is experimental: the card draws Windows' \
                                  desktop on an Arc B580, and other cards are untried";
const CARD_NOTE: &str = "Windows is installed for this card's maker. It can move to another card of the same maker \
                         later, but not to another maker's.";

struct Parts {
    window: Window,
    dialog: adw::Dialog,
    create_button: gtk::Button,
    iso_row: adw::ActionRow,
    /// Whether macOS was chosen, and not a disc.
    macos: Cell<bool>,
    verdict: adw::ActionRow,
    hardware: adw::PreferencesGroup,
    name: adw::EntryRow,
    account: adw::PreferencesGroup,
    user: adw::EntryRow,
    password: adw::PasswordEntryRow,
    /// Remote Desktop and SSH, for a Windows machine.
    access: adw::PreferencesGroup,
    remote_desktop: adw::SwitchRow,
    ssh: adw::SwitchRow,
    /// Lean Windows, for a Windows machine. Off unless the user or the chosen recipe turns it on.
    lean_group: adw::PreferencesGroup,
    lean: adw::SwitchRow,
    /// The switch is on because the recipe turned it on, and goes off with the recipe.
    lean_from_recipe: Cell<bool>,
    /// Folders the chosen recipe names for Defender to leave out. There only when it names some, and
    /// off whatever the recipe says, as only the user turns it on.
    skip_scanning_group: adw::PreferencesGroup,
    skip_scanning: adw::SwitchRow,
    cores: adw::SpinRow,
    ram: adw::SpinRow,
    disk: adw::SpinRow,
    disk_place: DiskPlaceRow,
    /// Which card a Windows machine runs on, there only when this PC has more than one to choose.
    card: adw::ComboRow,
    cards: Vec<CardChoice>,
    driver_row: DriverRow,
    /// What the machine is for, there only when a recipe exists for the disc's kind.
    recipes_group: adw::PreferencesGroup,
    recipe_row: adw::ComboRow,
    recipe_show: gtk::Button,
    /// The recipes behind the row's choices after "Nothing extra", for the disc's kind.
    recipes: RefCell<Vec<Entry>>,
    /// The sizes are a recipe's, and go back to the disc's own when it is taken off.
    recipe_sized: Cell<bool>,
    note: gtk::Label,
    iso_path: RefCell<Option<String>>,
    iso: RefCell<Option<Iso>>,
    /// The name last offered, to tell it from one the user typed.
    suggested: RefCell<String>,
    no_driver: Cell<bool>,
    /// This PC can't give the machine the RAM it needs, which the RAM setting says.
    no_room: Cell<bool>,
    vm_running: Cell<bool>,
}

/// A name the user typed stays when they choose another disc. One this dialog offered goes with its
/// disc, or a machine ends up named after the disc they changed their mind about.
fn keeps_following_the_disc(name: &str, suggested: &str) -> bool {
    name.is_empty() || name == suggested
}

#[derive(Clone)]
pub struct NewMachineDialog(Rc<Parts>);

/// A row with a problem says so, in red and in its tooltip.
fn mark_problem(row: &gtk::Widget, problem: Option<&str>) {
    if problem.is_some() {
        row.add_css_class("error");
    } else {
        row.remove_css_class("error");
    }
    row.set_tooltip_text(problem);
}

fn hardware_row(group: &adw::PreferencesGroup, title: &str, lowest: f64, highest: f64) -> adw::SpinRow {
    let row = number_row(title, lowest, lowest, highest);
    group.add(&row);
    row
}

impl NewMachineDialog {
    pub fn new(window: &Window) -> Self {
        let context = window.context();
        let create_button = gtk::Button::builder().label("Create").sensitive(false).build();
        create_button.add_css_class("suggested-action");
        let cancel = gtk::Button::with_label("Cancel");
        let header = adw::HeaderBar::builder().show_title(false).build();
        let page = adw::PreferencesPage::new();
        let disc = adw::PreferencesGroup::new();
        page.add(&disc);
        let iso_row =
            adw::ActionRow::builder().title("ISO").subtitle("Choose the disc to install from.").build();
        let choose = row_button("Choose");
        iso_row.add_suffix(&choose);
        disc.add(&iso_row);
        let macos_row = adw::ActionRow::builder().title(MACOS_TITLE).subtitle(MACOS_ROW_NOTE).build();
        let preview = gtk::Label::builder().label(PREVIEW).valign(gtk::Align::Center).build();
        preview.add_css_class("dim-label");
        preview.add_css_class("caption-heading");
        macos_row.add_suffix(&preview);
        let use_macos = row_button("Use");
        macos_row.add_suffix(&use_macos);
        disc.add(&macos_row);
        let verdict = adw::ActionRow::builder().visible(false).activatable(false).build();
        verdict.add_css_class("verdict");
        disc.add(&verdict);
        let hardware = adw::PreferencesGroup::new();
        hardware.set_sensitive(false);
        page.add(&hardware);
        let name = adw::EntryRow::builder().title("Name").build();
        hardware.add(&name);
        let cores = hardware_row(&hardware, "Cores", 1.0, physical_cores(&context.cpus).max(1).into());
        let ram = hardware_row(&hardware, "RAM in GiB", 1.0, 1.0);
        let disk = hardware_row(&hardware, "Disk in GiB", MIN_PLAIN_DISK_GIB.into(), MAX_DISK_GIB.into());
        disk.set_subtitle(DISK_NOTE);
        let disk_place = DiskPlaceRow::new(window, &hardware);
        let account = adw::PreferencesGroup::builder().title("Account").visible(false).build();
        page.add(&account);
        let user = adw::EntryRow::builder().title("User name").build();
        account.add(&user);
        let password = adw::PasswordEntryRow::builder().title("Password").build();
        account.add(&password);
        let access = adw::PreferencesGroup::builder()
            .title("Remote Access")
            .description(ACCESS_NOTE)
            .visible(false)
            .build();
        page.add(&access);
        let remote_desktop = adw::SwitchRow::builder()
            .title(Service::RemoteDesktop.name())
            .subtitle(REMOTE_DESKTOP_NOTE)
            .active(Access::NEW.is_on(Service::RemoteDesktop))
            .build();
        access.add(&remote_desktop);
        // A build without OpenSSH's installer can't give a guest SSH, and says so rather than fail later.
        let no_openssh = ssh_problem(&context);
        let ssh = adw::SwitchRow::builder()
            .title(Service::Ssh.name())
            .subtitle(no_openssh.unwrap_or(SSH_NOTE))
            .active(Access::NEW.is_on(Service::Ssh) && no_openssh.is_none())
            .sensitive(no_openssh.is_none())
            .build();
        access.add(&ssh);
        let lean_group = adw::PreferencesGroup::builder().visible(false).build();
        page.add(&lean_group);
        let lean = adw::SwitchRow::builder().title(LEAN_TITLE).subtitle(LEAN_NOTE).build();
        let lean_show = row_button("Show");
        lean.add_suffix(&lean_show);
        lean_group.add(&lean);
        let skip_scanning_group = adw::PreferencesGroup::builder().visible(false).build();
        page.add(&skip_scanning_group);
        let skip_scanning = adw::SwitchRow::builder()
            .title(SKIP_SCANNING_TITLE)
            .subtitle(WHAT_IT_MEANS)
            .subtitle_lines(0)
            .build();
        let skip_scanning_show = row_button("Show");
        skip_scanning.add_suffix(&skip_scanning_show);
        skip_scanning_group.add(&skip_scanning);
        let cards = windows_card_choices(&context);
        let labels: Vec<&str> = cards.iter().map(|choice| choice.label.as_str()).collect();
        let card = choice_row(CARD_TITLE, &labels);
        card.set_subtitle(CARD_NOTE);
        card.set_selected(default_choice(&context, &cards) as u32);
        card.set_visible(false);
        hardware.add(&card);
        let driver_row = DriverRow::new(window, &hardware);
        let recipes_group = adw::PreferencesGroup::builder().description(RECIPE_NOTE).visible(false).build();
        page.add(&recipes_group);
        let recipe_row = choice_row("Set up for", &[NO_RECIPE]);
        let recipe_show = row_button("Show");
        recipe_show.set_sensitive(false);
        recipe_row.add_suffix(&recipe_show);
        recipes_group.add(&recipe_row);
        let note = gtk::Label::builder()
            .wrap(true)
            .xalign(0.0)
            .margin_start(NOTE_MARGIN)
            .margin_end(NOTE_MARGIN)
            .margin_bottom(NOTE_MARGIN)
            .build();
        note.add_css_class("dim-label");
        let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        column.append(&page_heading("", "New Machine"));
        column.append(&page);
        column.append(&note);
        page.set_vexpand(true);
        let toolbar = adw::ToolbarView::new();
        toolbar.set_content(Some(&column));
        toolbar.add_top_bar(&header);
        toolbar.add_bottom_bar(&bottom_bar(&[&cancel, &create_button]));
        let dialog = adw::Dialog::builder()
            .title("New Machine")
            .content_width(WIDTH)
            .content_height(HEIGHT)
            .child(&toolbar)
            .build();
        let this = Self(Rc::new(Parts {
            window: window.clone(),
            dialog,
            create_button,
            iso_row,
            macos: Cell::new(false),
            verdict,
            hardware,
            name,
            account,
            user,
            password,
            access,
            remote_desktop,
            ssh,
            lean_group,
            lean,
            lean_from_recipe: Cell::new(false),
            skip_scanning_group,
            skip_scanning,
            cores,
            ram,
            disk,
            disk_place,
            card,
            cards,
            driver_row,
            recipes_group,
            recipe_row,
            recipe_show,
            recipes: RefCell::default(),
            recipe_sized: Cell::new(false),
            note,
            iso_path: RefCell::default(),
            iso: RefCell::default(),
            suggested: RefCell::default(),
            no_driver: Cell::new(false),
            no_room: Cell::new(false),
            vm_running: Cell::new(false),
        }));
        this.set_vm_running(window.vm_running());
        this.connect(&cancel, &choose);
        let dialog = this.0.dialog.clone();
        lean_show.connect_clicked(move |_| recipe_dialog::show(&dialog, LEAN_TITLE, &what_it_does()));
        let handle = this.clone();
        skip_scanning_show.connect_clicked(move |_| {
            if let Some(entry) = handle.chosen_recipe() {
                let text = scan_exclusions::what_it_does(&entry.recipe.scan_exclusions);
                recipe_dialog::show(&handle.0.dialog, SKIP_SCANNING_TITLE, &text);
            }
        });
        let handle = this.clone();
        use_macos.connect_clicked(move |_| handle.choose_macos());
        this
    }

    fn connect(&self, cancel: &gtk::Button, choose: &gtk::Button) {
        let this = self.clone();
        self.0.name.connect_changed(move |_| this.update());
        let this = self.clone();
        self.0.user.connect_changed(move |_| this.update());
        let this = self.clone();
        self.0.password.connect_changed(move |_| this.update());
        let dialog = self.0.dialog.clone();
        cancel.connect_clicked(move |_| {
            dialog.close();
        });
        let this = self.clone();
        choose.connect_clicked(move |_| this.on_choose());
        let this = self.clone();
        self.0.create_button.connect_clicked(move |_| this.on_create());
        let this = self.clone();
        self.0.card.connect_selected_notify(move |_| this.card_changed());
        let this = self.clone();
        self.0.recipe_row.connect_selected_notify(move |_| this.recipe_changed());
        let this = self.clone();
        self.0.recipe_show.connect_clicked(move |_| this.show_recipe());
        let this = self.clone();
        self.0.driver_row.connect_ready(move || this.card_changed());
        let window = self.0.window.clone();
        let driver_row = self.0.driver_row.clone();
        self.0.dialog.connect_closed(move |_| {
            driver_row.stop();
            window.new_machine_closed();
        });
    }

    pub fn present(&self) {
        self.0.dialog.present(Some(self.0.window.widget()));
    }

    pub fn set_vm_running(&self, running: bool) {
        self.0.vm_running.set(running);
        let note = if running {
            format!("{} Shut one down first.", no_slot_text(&self.0.window.context()))
        } else {
            String::new()
        };
        self.0.note.set_label(&note);
        self.update();
    }

    fn on_choose(&self) {
        let window = self.0.window.clone();
        let failing = window.clone();
        let this = self.clone();
        choose_file(
            window.widget(),
            "Choose an ISO",
            "Disc images",
            "iso",
            move |path| this.read_iso(path),
            move |text| failing.problem(&text),
        );
    }

    /// Takes this ISO as if it had been chosen. It's public for the page checks, where no file dialog
    /// can be answered.
    /// macOS in place of a disc: nothing to read, and sizes it runs well with.
    fn choose_macos(&self) {
        let parts = &self.0;
        let context = parts.window.context();
        parts.macos.set(true);
        *parts.iso_path.borrow_mut() = Some(String::new());
        parts.iso_row.set_subtitle("Choose the disc to install from.");
        self.show_verdict(MACOS_TITLE, &how_it_installs(&context));
        let field = RamField::for_plain(&context, 1, MACOS_SIZES.ram);
        parts.cores.adjustment().set_lower(1.0);
        parts.cores.set_value(MACOS_SIZES.cores.min(physical_cores(&context.cpus)).into());
        parts.ram.adjustment().set_lower(field.lower as f64);
        parts.ram.adjustment().set_upper(field.upper as f64);
        parts.ram.set_value(field.value as f64);
        parts.ram.set_sensitive(field.fits);
        parts.ram.set_subtitle(&field.subtitle);
        parts.no_room.set(!field.fits);
        parts.disk.adjustment().set_lower(MIN_MACOS_DISK_GIB.into());
        parts.disk.set_value(MACOS_SIZES.disk.into());
        parts.card.set_visible(false);
        parts.account.set_visible(false);
        parts.lean_group.set_visible(false);
        self.fill_recipes(Kind::Plain);
        parts.no_driver.set(false);
        if keeps_following_the_disc(&parts.name.text(), &parts.suggested.borrow()) {
            parts.name.set_text(MACOS_NAME);
        }
        *parts.suggested.borrow_mut() = MACOS_NAME.to_owned();
        let title = MACOS_TITLE.to_owned();
        *parts.iso.borrow_mut() =
            Some(Iso { kind: Kind::Plain, title, edition: String::new(), windows_disc: false });
        parts.hardware.set_sensitive(true);
        self.update();
    }

    fn create_macos(&self) {
        let parts = &self.0;
        parts.create_button.set_sensitive(false);
        let new = NewMac {
            name: parts.name.text().to_string(),
            cores: parts.cores.value() as u32,
            ram_gib: parts.ram.value() as u32,
            disk_gib: parts.disk.value() as u32,
        };
        let context = parts.window.context();
        let working = context.clone();
        let this = self.clone();
        in_background(
            &context,
            move || create_macos(&working, &new),
            move |result| match result {
                Ok(machine) => {
                    let window = &this.0.window;
                    window.add_machine(machine.clone());
                    window.launch("install", &machine, &[], None);
                    this.0.dialog.close();
                }
                Err(text) => {
                    this.0.window.problem(&text);
                    this.update();
                }
            },
        );
    }

    pub fn read_iso(&self, path: String) {
        self.0.macos.set(false);
        *self.0.iso_path.borrow_mut() = Some(path.clone());
        *self.0.iso.borrow_mut() = None;
        self.0.iso_row.set_subtitle(&path);
        self.0.hardware.set_sensitive(false);
        self.show_verdict("Reading the ISO", "");
        self.update();
        let context = self.0.window.context();
        let this = self.clone();
        let reading = path.clone();
        let reader = context.clone();
        in_background(
            &context,
            move || read_disc(&reader, &reading),
            move |result| {
                // Another choice may have come since. Only the newest counts.
                if this.0.iso_path.borrow().as_deref() != Some(&path) {
                    return;
                }
                match result {
                    Ok(iso) => this.show_iso(&path, iso),
                    Err(text) => this.show_verdict(&text, ""),
                }
            },
        );
    }

    fn show_verdict(&self, title: &str, subtitle: &str) {
        let verdict = &self.0.verdict;
        verdict.set_visible(true);
        verdict.set_title(&gtk::glib::markup_escape_text(title));
        verdict.set_subtitle(&gtk::glib::markup_escape_text(subtitle));
    }

    /// `reason` says why this disc is only a plain VM here, if it is.
    fn describe(&self, iso: &Iso, reason: &str) {
        let context = self.0.window.context();
        let windows = iso.kind == Kind::Windows;
        if iso.kind == Kind::Linux {
            self.show_verdict(&iso.title, ubuntu_note(&context));
            return;
        }
        let missing_driver = if windows { guest_driver(&context, &self.chosen_card()).err() } else { None };
        self.0.no_driver.set(missing_driver.is_some());
        // The maker whose installer the row asks for, when it's that driver that is missing.
        let installer_for = match missing_driver {
            Some(missing) if missing == NO_AMD_SETUP => Some(GpuMaker::Amd),
            Some(missing) if missing == NO_INTEL_SETUP => Some(GpuMaker::Intel),
            _ => None,
        };
        self.0.driver_row.show_for(installer_for.map(|maker| (maker, self.chosen_device())));
        if let Some(missing) = missing_driver {
            self.show_verdict(&iso.title, missing);
        } else if windows {
            let maker = self.chosen_choice().map_or(GpuMaker::Nvidia, |choice| choice.maker);
            // The warning is about NVIDIA's card and setup, so the start's own rule says who hears it.
            let warning = machine_gpu_warning(&context, &self.chosen_card())
                .map_or_else(String::new, |text| format!(" {text}"));
            // A build from source has no driver to add, so it says nothing about one.
            let display = DriverPackage::find(&context).map_or("", |_| DISPLAY_DRIVER_NOTE);
            let gpu = match maker {
                GpuMaker::Nvidia => "starts with GPU-PV",
                GpuMaker::Amd => AMD_NOT_FINISHED,
                GpuMaker::Intel => INTEL_NOT_FINISHED,
            };
            let said = format!("Installs by itself and {gpu}. {display}{warning}");
            self.show_verdict(&iso.title, said.trim());
        } else {
            self.show_verdict(
                &format!("{}: a plain VM", iso.title),
                format!("You install it by hand. {NO_GPU} {reason}").trim(),
            );
        }
    }

    fn set_sizes(&self, iso: &Iso) {
        let context = self.0.window.context();
        let windows = iso.kind == Kind::Windows;
        let sizes = sizes_for_disc(iso);
        let field = if windows {
            RamField::for_gpu(&context, context.slot, MIN_RAM_GIB.into(), sizes.ram)
        } else {
            RamField::for_plain(&context, 1, sizes.ram)
        };
        let parts = &self.0;
        let cores = physical_cores(&context.cpus);
        parts.cores.adjustment().set_lower(if windows { MIN_CORES.into() } else { 1.0 });
        parts.cores.set_value(sizes.cores.min(cores).into());
        parts.ram.adjustment().set_lower(field.lower as f64);
        parts.ram.adjustment().set_upper(field.upper as f64);
        parts.ram.set_value(field.value as f64);
        parts.ram.set_sensitive(field.fits);
        parts.ram.set_subtitle(&field.subtitle);
        parts.no_room.set(!field.fits);
        let least = if windows { MIN_WINDOWS_DISK_GIB } else { MIN_PLAIN_DISK_GIB };
        parts.disk.adjustment().set_lower(least.into());
        parts.disk.set_value(sizes.disk.into());
    }

    fn show_iso(&self, path: &str, iso: Iso) {
        let context = self.0.window.context();
        let reason = plain_reason(&context, iso.kind);
        let iso = Iso { kind: kind_offered(&context, iso.kind), ..iso };
        self.describe(&iso, reason);
        self.set_sizes(&iso);
        self.0.card.set_visible(iso.kind == Kind::Windows && self.0.cards.len() > 1);
        let suggestion = suggested_name(iso.kind, path);
        if keeps_following_the_disc(&self.0.name.text(), &self.0.suggested.borrow()) {
            self.0.name.set_text(&suggestion);
        }
        *self.0.suggested.borrow_mut() = suggestion;
        self.show_account(iso.kind);
        self.fill_recipes(iso.kind);
        self.0.access.set_visible(iso.kind == Kind::Windows);
        self.0.lean_group.set_visible(iso.kind == Kind::Windows);
        *self.0.iso.borrow_mut() = Some(iso);
        self.0.hardware.set_sensitive(true);
        self.update();
    }

    /// The account rows, for a disc that installs by itself. The name starts as the one the guest
    /// scripts have always used, and a name the user typed stays through a change of disc.
    fn show_account(&self, kind: Kind) {
        let parts = &self.0;
        let note = match kind {
            Kind::Windows => WINDOWS_ACCOUNT_NOTE,
            Kind::Linux => LINUX_ACCOUNT_NOTE,
            Kind::Plain => {
                parts.account.set_visible(false);
                return;
            }
        };
        let typed = parts.user.text();
        let was_default = [default_user(Kind::Windows), default_user(Kind::Linux)].contains(&typed.as_str());
        if typed.is_empty() || was_default {
            parts.user.set_text(default_user(kind));
        }
        parts.account.set_description(Some(note));
        parts.account.set_visible(true);
    }

    /// Says on the row what's wrong with its text, if anything, and whether something is.
    fn account_problems(&self) -> bool {
        let parts = &self.0;
        let Some(kind) = parts.iso.borrow().as_ref().map(|iso| iso.kind) else { return false };
        if kind == Kind::Plain {
            return false;
        }
        let name = name_problem(kind, parts.user.text().trim());
        let password = password_problem(&parts.password.text());
        mark_problem(parts.user.upcast_ref(), name.as_deref());
        mark_problem(parts.password.upcast_ref(), password.as_deref());
        name.is_some() || password.is_some()
    }

    /// The PCI address of the card chosen, empty for NVIDIA's, as a machine keeps it.
    fn chosen_card_pci(&self) -> String {
        self.chosen_choice().map(|choice| choice.pci.clone()).unwrap_or_default()
    }

    fn chosen_choice(&self) -> Option<&CardChoice> {
        self.0.cards.get(self.0.card.selected() as usize)
    }

    fn chosen_device(&self) -> u32 {
        self.0.cards.get(self.0.card.selected() as usize).map_or(0, |choice| choice.device)
    }

    /// A Windows machine on the card chosen, to ask what it would need.
    fn chosen_card(&self) -> Machine {
        let mut machine = Machine::new("", "", "", "");
        if let Some(choice) = self.chosen_choice() {
            machine.use_windows_card(choice.maker, &choice.pci);
        }
        machine
    }

    /// Another card can need another maker's driver, which may not be here.
    fn card_changed(&self) {
        let Some(iso) = self.0.iso.borrow().clone() else { return };
        let context = self.0.window.context();
        self.describe(&iso, plain_reason(&context, iso.kind));
        self.update();
    }

    /// The recipes for a disc of this kind. With none, there is no row, and the dialog is as it was.
    fn fill_recipes(&self, kind: Kind) {
        let parts = &self.0;
        let listing = list_for(&parts.window.context(), kind);
        let mut names = vec![NO_RECIPE.to_owned()];
        names.extend(listing.entries.iter().map(|entry| entry.recipe.name.clone()));
        let labels: Vec<&str> = names.iter().map(String::as_str).collect();
        parts.recipe_row.set_model(Some(&gtk::StringList::new(&labels)));
        parts.recipes_group.set_visible(!listing.entries.is_empty());
        parts.recipe_sized.set(false);
        *parts.recipes.borrow_mut() = listing.entries;
        parts.recipe_row.set_selected(0);
        let unread = listing.problems.len();
        let subtitle = match unread {
            0 => String::new(),
            1 => "One file in your recipes folder could not be read.".to_owned(),
            more => format!("{more} files in your recipes folder could not be read."),
        };
        parts.recipe_row.set_subtitle(&subtitle);
    }

    /// The recipe chosen, if one is.
    fn chosen_recipe(&self) -> Option<Entry> {
        let index = self.0.recipe_row.selected() as usize;
        self.0.recipes.borrow().get(index.checked_sub(1)?).cloned()
    }

    /// The recipe's own sizes go in the rows, as far as this PC allows, and the rows are the user's to
    /// change after. Taking it off puts the disc's own sizes back.
    fn recipe_changed(&self) {
        let parts = &self.0;
        let Some(iso) = parts.iso.borrow().clone() else { return };
        if parts.lean_from_recipe.replace(false) {
            parts.lean.set_active(false);
        }
        // Never carried from one recipe to the next: each time, the user turns it on for these folders.
        parts.skip_scanning.set_active(false);
        let named = self.chosen_recipe().is_some_and(|entry| !entry.recipe.scan_exclusions.is_empty());
        parts.skip_scanning_group.set_visible(named);
        let Some(entry) = self.chosen_recipe() else {
            parts.recipe_show.set_sensitive(false);
            parts.recipe_row.set_subtitle("");
            if parts.recipe_sized.replace(false) {
                self.set_sizes(&iso);
            }
            return;
        };
        let context = parts.window.context();
        let recipe = &entry.recipe;
        parts.recipe_show.set_sensitive(true);
        // As with the sizes, the recipe's choice goes in the row, and the row is the user's after.
        if recipe.lean && !parts.lean.is_active() {
            parts.lean.set_active(true);
            parts.lean_from_recipe.set(true);
        }
        let usual = Usual {
            cores: parts.cores.value() as u32,
            ram_gib: parts.ram.value() as u32,
            disk_gib: parts.disk.value() as u32,
        };
        let fitted = fit(&context, iso.kind, &recipe.settings, usual, self.recipe_vram_limit());
        parts.recipe_sized.set(true);
        parts.cores.set_value(fitted.cores.into());
        parts.ram.set_value(fitted.ram_gib.into());
        parts.disk.set_value(fitted.disk_gib.into());
        let origin = origin_text(entry.source, &entry.file.display().to_string());
        parts
            .recipe_row
            .set_subtitle([recipe.description.as_str(), &origin, &fitted.notes.join(" ")].join(" ").trim());
    }

    /// The most video memory the machine's card allows, for a recipe that caps it.
    fn recipe_vram_limit(&self) -> u32 {
        max_vram_gib_for(&self.0.window.context(), &self.chosen_card())
    }

    fn show_recipe(&self) {
        if let Some(entry) = self.chosen_recipe() {
            let text = what_it_will_run(&entry.recipe, entry.source, &entry.file.display().to_string());
            recipe_dialog::show(&self.0.dialog, &entry.recipe.name, &text);
        }
    }

    fn update(&self) {
        let parts = &self.0;
        let ready = parts.iso.borrow().is_some()
            && !parts.name.text().trim().is_empty()
            && !self.account_problems()
            && !parts.vm_running.get()
            && !parts.no_driver.get()
            && !parts.no_room.get();
        parts.create_button.set_sensitive(ready);
    }

    fn on_create(&self) {
        let parts = &self.0;
        if parts.macos.get() {
            return self.create_macos();
        }
        let Some(entry) = self.chosen_recipe() else { return self.create(None) };
        // A recipe of the user's own was read by nobody but them, so what it runs is shown first. The
        // app's own are shown by the row's button.
        if entry.source == Source::Yours && !entry.recipe.steps.is_empty() {
            let text = what_it_will_run(&entry.recipe, entry.source, &entry.file.display().to_string());
            let heading = entry.recipe.name.clone();
            let this = self.clone();
            return recipe_dialog::ask(&parts.dialog, &heading, &text, "Create", move || {
                this.create(Some(entry.clone()));
            });
        }
        self.create(Some(entry));
    }

    /// Makes the machine, with the recipe if one was chosen.
    fn create(&self, entry: Option<Entry>) {
        let parts = &self.0;
        let (Some(iso), Some(path)) = (parts.iso.borrow().clone(), parts.iso_path.borrow().clone()) else {
            return;
        };
        parts.create_button.set_sensitive(false);
        let new = NewMachine {
            name: parts.name.text().to_string(),
            iso: path,
            kind: iso.kind,
            windows_disc: iso.windows_disc,
            cores: parts.cores.value() as u32,
            ram_gib: parts.ram.value() as u32,
            disk_gib: parts.disk.value() as u32,
            refresh_hz: monitor_hz(parts.window.widget()),
            user: account_name(iso.kind, parts.user.text().trim()),
            password: parts.password.text().to_string(),
            amd_gpu: (iso.kind == Kind::Windows).then(|| self.chosen_card_pci()),
            disk_place: parts.disk_place.chosen(),
            gpu_maker: self.chosen_choice().map(|choice| choice.maker),
            access: Access {
                remote_desktop: Some(parts.remote_desktop.is_active()),
                ssh: Some(parts.ssh.is_active()),
            },
        };
        let context = parts.window.context();
        // The video memory cap is the one setting here that has no row, so it comes from the recipe,
        // held to what the card allows.
        let lean = iso.kind == Kind::Windows && parts.lean.is_active();
        let skipped = match &entry {
            Some(entry) if iso.kind == Kind::Windows && parts.skip_scanning.is_active() => {
                entry.recipe.scan_exclusions.clone()
            }
            _ => Vec::new(),
        };
        let extras = entry.map_or_else(Extras::default, |entry| {
            let asked =
                SettingsAsked { vram_gib: entry.recipe.settings.vram_gib, ..SettingsAsked::default() };
            let usual = Usual { cores: new.cores, ram_gib: new.ram_gib, disk_gib: new.disk_gib };
            let vram = fit(&context, iso.kind, &asked, usual, self.recipe_vram_limit()).vram_gib;
            let file = entry.file.display().to_string();
            Extras {
                vram_gib: vram,
                recipe: Some(Chosen { source: entry.source, file, recipe: entry.recipe }),
                ..Extras::default()
            }
        });
        let extras = Extras { lean, scan_exclusions: skipped, ..extras };
        let working = context.clone();
        let this = self.clone();
        in_background(
            &context,
            move || create_with(&working, &new, &extras),
            move |result| match result {
                Ok(machine) => {
                    let window = &this.0.window;
                    let action = if machine.installing { "install" } else { "start" };
                    window.add_machine(machine.clone());
                    window.launch(action, &machine, &[], None);
                    this.0.dialog.close();
                }
                Err(text) => {
                    this.0.window.problem(&text);
                    this.update();
                }
            },
        );
    }
}

/// The refresh rate of the monitor a window is on, in whole Hz. GTK gives it in thousandths, as a
/// monitor runs at 59.94 Hz or 143.98 Hz as often as at a round number.
fn monitor_hz(window: &impl IsA<gtk::Widget>) -> Option<u32> {
    let surface = window.native()?.surface()?;
    let monitor = surface.display().monitor_at_surface(&surface)?;
    let millihertz = u32::try_from(monitor.refresh_rate()).ok()?;
    Some((millihertz + 500) / 1000).filter(|hz| *hz > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_suggested_name_goes_with_its_disc() {
        assert!(keeps_following_the_disc("", ""));
        assert!(keeps_following_the_disc("win11-eval", "win11-eval"));
    }

    #[test]
    fn a_typed_name_stays() {
        assert!(!keeps_following_the_disc("My VM", "win11-eval"));
        assert!(!keeps_following_the_disc("My VM", ""));
    }
}
