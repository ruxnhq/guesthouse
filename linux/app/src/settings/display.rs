//! The screen: its size, rate and HDR, the driver behind them, and the viewer's overlay. A change
//! reaches a running machine at once if it's on the display driver, and otherwise the next start. The
//! switches to turn off when a picture goes wrong are on the Advanced page.

use super::Shared;
use crate::rows::{choice_row, explain, row_button};
use crate::viewer::{FULLSCREEN_KEYS, OVERLAY_KEYS};
use adw::prelude::*;
use guesthouse_app_core::display::{
    DisplayState, Driver, DriverPackage, forget_note, read_state, screen_hdr,
};
use guesthouse_app_core::machine::{
    Kind, MAX_REFRESH_HZ, MIN_REFRESH_HZ, REFRESH_RATES, SCREENS, read_refresh,
};
use guesthouse_app_core::slots;
use guesthouse_app_core::tasks::{State, Status};
use std::cell::RefCell;
use std::rc::Rc;

const RATE_NOTE: &str = "Set at each start";
const BASIC_NOTE: &str = "Windows is on its basic display, which has one rate";
const RATE_INFO: &str = "Windows gets the rate each time the machine starts, or at once if it runs on the display \
                         driver. Its basic display has only one.";
const HDR_NOTE: &str = "At the next start, or at once if it's running";
const WINDOW_SPEED_SHORT: &str = "Off if a program's window flickers";
const WINDOW_SPEED_NOTE: &str = "Windows copies every frame of a program that draws in a window, unless it counts \
                                 the program as a game. On, no program outside the Windows folder is held back \
                                 that way, which can double its frame rate. On NVIDIA it also has Vulkan and \
                                 OpenGL present through DXGI, without their copy of every frame. Turn it off if a \
                                 program's window flickers or stays blank. Takes effect when the machine starts.";
const DRIVER_ROW: &str = "Display Driver";
const DRIVER_IN: &str = "In Windows";
const DRIVER_NEXT: &str = "Goes into Windows at the next start";
const DRIVER_NOTE: &str = "Guesthouse's display driver is signed on each machine with a certificate made for it, \
                           which Windows trusts as a root. Its private key is deleted before the install.";
const DIRECT_SHORT: &str = "Off if the picture goes wrong";
const DIRECT_NOTE: &str = "Hands the guest's frames to your desktop as they are, so they can reach the display \
                           sooner, and HDR can where the desktop shows it. Turn it off if the picture goes wrong.";
const DRIVER_NONE: &str = "Not in this build, so Windows stays on its basic display";
const OVERLAY_NOTE: &str = "Shows frames a second and the host GPU's load over the screen.";
const BAR_NOTE: &str =
    "The bar over the screen hides, and comes back when the pointer rests at the top edge of the window.";
const MATCH_NOTE: &str =
    "Windows' screen takes the size of the viewer's window. The screen size above is what it starts with.";

struct Parts {
    shared: Shared,
    group: adw::PreferencesGroup,
    screen: adw::ComboRow,
    rate: adw::ComboRow,
    custom: adw::EntryRow,
    hdr: adw::SwitchRow,
    driver: adw::ActionRow,
    install: gtk::Button,
    status: RefCell<Status>,
}

#[derive(Clone)]
pub struct Display(Rc<Parts>);

impl Display {
    /// The expert switches go in `graphics`, on the Advanced page.
    pub fn new(shared: &Shared, page: &adw::PreferencesPage, graphics: &adw::PreferencesGroup) -> Self {
        let machine = shared.machine.borrow().clone();
        let group = adw::PreferencesGroup::builder().title("Display").build();
        group.set_visible(machine.kind == Kind::Windows);
        page.add(&group);
        let screen = choice_row("Screen Size", &SCREENS);
        screen.set_selected(SCREENS.iter().position(|size| *size == machine.screen).unwrap_or(0) as u32);
        group.add(&screen);
        let mut names: Vec<String> = REFRESH_RATES.iter().map(|hz| format!("{hz} Hz")).collect();
        names.push("Custom".to_owned());
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let rate = choice_row("Refresh Rate", &names);
        explain(&rate, RATE_INFO);
        let current = REFRESH_RATES.iter().position(|hz| *hz == machine.refresh_hz);
        rate.set_selected(current.unwrap_or(REFRESH_RATES.len()) as u32);
        group.add(&rate);
        let custom = adw::EntryRow::builder()
            .title("Custom Rate")
            .text(machine.refresh_hz.to_string())
            .show_apply_button(true)
            .input_purpose(gtk::InputPurpose::Digits)
            .build();
        group.add(&custom);
        let hdr = adw::SwitchRow::builder().title("HDR").active(machine.hdr).build();
        group.add(&hdr);
        let window_speed = adw::SwitchRow::builder()
            .title("Faster Programs in a Window")
            .active(machine.window_speed())
            .subtitle(WINDOW_SPEED_SHORT)
            .build();
        explain(&window_speed, WINDOW_SPEED_NOTE);
        let saving = shared.clone();
        window_speed.connect_active_notify(move |row| {
            saving.machine.borrow_mut().window_speed_off = !row.is_active();
            saving.save();
        });
        if machine.kind == Kind::Windows {
            graphics.add(&window_speed);
        }
        let driver = adw::ActionRow::builder().title(DRIVER_ROW).build();
        explain(&driver, DRIVER_NOTE);
        let install = row_button("Install Driver");
        driver.add_suffix(&install);
        group.add(&driver);
        let status = RefCell::default();
        let this = Self(Rc::new(Parts {
            shared: shared.clone(),
            group,
            screen,
            rate,
            custom,
            hdr,
            driver,
            install,
            status,
        }));
        this.show_custom_rate();
        this.connect();
        this.add_viewer_group(page, graphics);
        this
    }

    fn add_viewer_group(&self, page: &adw::PreferencesPage, graphics: &adw::PreferencesGroup) {
        let group = adw::PreferencesGroup::builder().title("Viewer").build();
        page.add(&group);
        let shared = &self.0.shared;
        let overlay = adw::SwitchRow::builder()
            .title("Performance Overlay")
            .active(shared.machine.borrow().overlay)
            .subtitle(format!("{OVERLAY_KEYS} turns it on and off"))
            .build();
        explain(&overlay, OVERLAY_NOTE);
        let saving = shared.clone();
        overlay.connect_active_notify(move |row| {
            saving.machine.borrow_mut().overlay = row.is_active();
            saving.save();
        });
        group.add(&overlay);
        let direct = adw::SwitchRow::builder()
            .title("Direct Frames")
            .active(shared.machine.borrow().direct_frames())
            .subtitle(DIRECT_SHORT)
            .build();
        explain(&direct, DIRECT_NOTE);
        let saving = shared.clone();
        direct.connect_active_notify(move |row| {
            saving.machine.borrow_mut().direct_frames_off = !row.is_active();
            saving.save();
        });
        graphics.add(&direct);
        let bar = adw::SwitchRow::builder()
            .title("Hide the Bar")
            .active(shared.machine.borrow().bar_hides())
            .subtitle("It comes back at the window's top edge")
            .build();
        explain(&bar, BAR_NOTE);
        let saving = shared.clone();
        bar.connect_active_notify(move |row| {
            saving.machine.borrow_mut().bar_stays = !row.is_active();
            saving.save();
        });
        group.add(&bar);
        let fullscreen = adw::SwitchRow::builder()
            .title("Open in Fullscreen")
            .active(shared.machine.borrow().opens_fullscreen)
            .subtitle(format!("{FULLSCREEN_KEYS} leaves it"))
            .build();
        let saving = shared.clone();
        fullscreen.connect_active_notify(move |row| {
            saving.machine.borrow_mut().opens_fullscreen = row.is_active();
            saving.save();
        });
        group.add(&fullscreen);
        // A draft Windows machine gets its GPU once it's made and installed.
        let gpu = {
            let machine = shared.machine.borrow();
            machine.has_gpu() || (shared.is_draft() && machine.kind == Kind::Windows)
        };
        let window_size = adw::SwitchRow::builder()
            .title("Match the Window")
            .active(shared.machine.borrow().sizes_to_window())
            .visible(gpu)
            .subtitle("Windows' screen follows the window's size")
            .build();
        explain(&window_size, MATCH_NOTE);
        let saving = shared.clone();
        window_size.connect_active_notify(move |row| {
            saving.machine.borrow_mut().window_size_off = !row.is_active();
            saving.save();
        });
        group.add(&window_size);
    }

    fn connect(&self) {
        let this = self.clone();
        self.0.screen.connect_selected_notify(move |row| {
            let wanted = SCREENS[row.selected() as usize];
            if wanted != this.0.shared.machine.borrow().screen {
                this.0.shared.machine.borrow_mut().screen = wanted.to_owned();
                this.changed();
            }
        });
        let this = self.clone();
        self.0.hdr.connect_active_notify(move |row| {
            this.0.shared.machine.borrow_mut().hdr = row.is_active();
            this.changed();
        });
        let this = self.clone();
        self.0.install.connect_clicked(move |_| this.launch("install_display"));
        let this = self.clone();
        self.0.rate.connect_selected_notify(move |row| {
            this.show_custom_rate();
            if let Some(hz) = REFRESH_RATES.get(row.selected() as usize) {
                this.set_rate(*hz);
            }
        });
        let this = self.clone();
        self.0.custom.connect_changed(move |row| {
            row.remove_css_class("error");
            this.show_custom_rate();
        });
        let this = self.clone();
        self.0.custom.connect_apply(move |row| this.on_custom_rate(row));
    }

    fn show_custom_rate(&self) {
        let custom = self.0.rate.selected() as usize == REFRESH_RATES.len();
        self.0.custom.set_visible(custom);
        let text =
            format!("From {MIN_REFRESH_HZ} to {MAX_REFRESH_HZ} Hz. The guest may not reach a high rate.");
        self.0.group.set_description(custom.then_some(text.as_str()));
    }

    fn on_custom_rate(&self, row: &adw::EntryRow) {
        match read_refresh(&row.text()) {
            Err(failure) => {
                row.add_css_class("error");
                // A toast would cover this row in a narrow window.
                self.0.group.set_description(Some(&failure.0));
            }
            // Selecting it sets the rate.
            Ok(hz) => match REFRESH_RATES.iter().position(|named| *named == hz) {
                Some(index) => self.0.rate.set_selected(index as u32),
                None => self.set_rate(hz),
            },
        }
    }

    fn set_rate(&self, hz: u32) {
        let shared = &self.0.shared;
        if hz == shared.machine.borrow().refresh_hz {
            return;
        }
        shared.machine.borrow_mut().refresh_hz = hz;
        self.0.custom.set_text(&hz.to_string());
        self.changed();
    }

    fn launch(&self, action: &str) {
        let shared = &self.0.shared;
        let slot = self.0.status.borrow().slot.map(slots::number);
        shared.window.launch(action, &shared.machine.borrow(), &[], slot);
    }

    fn state(&self) -> DisplayState {
        let shared = &self.0.shared;
        if shared.is_draft() {
            return DisplayState::default();
        }
        read_state(&shared.window.context(), &shared.machine.borrow())
    }

    fn on_driver_now(&self) -> bool {
        self.state().running.is_some_and(|screen| screen.on_driver)
    }

    /// Saves a change, and takes it to Windows if Windows can take it now.
    fn changed(&self) {
        self.0.shared.save();
        let shared = &self.0.shared;
        if shared.is_draft() {
            return;
        }
        if let Err(failure) = forget_note(&shared.window.context(), &shared.machine.borrow()) {
            shared.window.problem(&failure.0);
        }
        let status = self.0.status.borrow().clone();
        if status.state == State::Running && !status.busy && self.on_driver_now() {
            self.launch("apply_display");
        }
    }

    fn driver_note(&self, state: &DisplayState, has_package: bool) -> String {
        match state.driver {
            Driver::Installed => DRIVER_IN.to_owned(),
            Driver::Failed => state.reason.clone(),
            Driver::Absent if has_package => DRIVER_NEXT.to_owned(),
            Driver::Absent => DRIVER_NONE.to_owned(),
        }
    }

    pub fn set_status(&self, status: &Status) {
        *self.0.status.borrow_mut() = status.clone();
        let parts = &self.0;
        let state = self.state();
        let context = parts.shared.window.context();
        let running = status.state == State::Running;
        let can_change = !status.busy;
        parts.screen.set_sensitive(can_change);
        parts.rate.set_sensitive(can_change);
        parts.custom.set_sensitive(can_change);
        parts.hdr.set_sensitive(can_change);
        let note = if status.busy {
            status.step.clone()
        } else if !status.screen.is_empty() {
            format!("Now {}.", status.screen)
        } else if running {
            BASIC_NOTE.to_owned()
        } else {
            RATE_NOTE.to_owned()
        };
        parts.rate.set_subtitle(&note);
        let direct = parts.shared.machine.borrow().direct_frames();
        parts.hdr.set_subtitle(screen_hdr(&context).sdr_reason(direct).unwrap_or(HDR_NOTE));
        let package = DriverPackage::find(&context);
        parts.driver.set_subtitle(&self.driver_note(&state, package.is_some()));
        let missing = state.driver != Driver::Installed && package.is_some();
        parts.install.set_visible(missing && running && !status.busy);
    }
}
