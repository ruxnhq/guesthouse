//! The page that shows a guest's screen, with its toolbar, the mouse, fullscreen and the thumbnail.
//!
//! The bar at the top hides in fullscreen, and in a window too unless the machine says it stays. The
//! top edge brings it back: at once in fullscreen, where the pointer stops against the edge, and in a
//! window only once the pointer has rested there, as it also crosses that edge on its way out.

mod capture;
mod capture_state;
mod clipboard;
mod cursor_layer;
mod dbus_frames;
mod display_attach;
mod display_surface;
mod dmabuf_feedback;
mod forwarded_frames;
mod frames;
mod guest_cursor;
mod import;
mod input;
mod monitors;
mod overlay;
mod own_screen;
mod own_surface;
mod pointer_lock;
mod ring;
mod screen;
mod screen_frame;
mod slot_hold;
mod socket_frames;
mod window_size;

use crate::common::in_background;
use crate::rows::name_for_readers;
use crate::window::{MachineHandle, Window};
use adw::prelude::*;
use clipboard::SharedClipboard;
use frames::FrameRoute;
use gtk::{gdk, glib};
use guesthouse_app_core::library::save_thumbnail;
use guesthouse_app_core::log::write_log;
use guesthouse_app_core::screen_gpu::memory_frames;
use guesthouse_app_core::tasks::{State, Status, slot_of};
use monitors::Monitors;
use screen::Screen;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;
use window_size::WindowSize;

pub use own_screen::own_surface_takes_hdr;
pub use socket_frames::takes_hdr_frames;

pub const FULLSCREEN_KEYS: &str = "Ctrl+Alt+Return";
/// One key: it captures the mouse, and pressed again it lets go.
pub const CAPTURE_KEYS: &str = "Ctrl+Alt+M";
pub const OVERLAY_KEYS: &str = "Ctrl+Alt+P";
/// Pixels at the top of the screen that bring the header back in fullscreen.
const TOP_EDGE: f64 = 2.0;
/// The same in a window, where nothing stops the pointer at the edge.
const WINDOW_TOP_EDGE: f64 = 6.0;
/// How long the pointer rests at a window's top edge before the header comes back. Shorter, and
/// reaching for the top row of the guest's screen brings it too.
const REST: Duration = Duration::from_millis(300);
/// How long the header stays once the pointer has left the window, so it's not gone the moment the
/// pointer slips off it.
const LEFT: Duration = Duration::from_millis(600);
const BUTTONS: gdk::ModifierType = gdk::ModifierType::BUTTON1_MASK
    .union(gdk::ModifierType::BUTTON2_MASK)
    .union(gdk::ModifierType::BUTTON3_MASK);
const USUAL_HEADER_HEIGHT: i32 = 48;
/// Kept at twice the size it's shown, for screens that scale by two.
const THUMBNAIL_WIDTH: usize = 256;
const THUMBNAIL_EVERY: Duration = Duration::from_secs(30);

struct Parts {
    window: Window,
    machine: MachineHandle,
    page: adw::NavigationPage,
    title: adw::WindowTitle,
    toolbar: adw::ToolbarView,
    fullscreen_button: adw::SplitButton,
    capture_button: gtk::Button,
    screen: RefCell<Option<Screen>>,
    clipboard: RefCell<Option<SharedClipboard>>,
    window_size: RefCell<Option<WindowSize>>,
    monitors: RefCell<Option<Rc<Monitors>>>,
    fullscreen_handler: RefCell<Option<glib::SignalHandlerId>>,
    thumbnail_source: RefCell<Option<glib::SourceId>>,
    /// The pointer brought the header back, and hasn't left it since.
    header_called: Cell<bool>,
    /// Whether the header was hiding when last looked at, to say so once each time it starts to.
    header_hid: Cell<bool>,
    /// Runs out when the pointer has rested at the top edge, or has been gone from the window.
    header_wait: RefCell<Option<glib::SourceId>>,
}

#[derive(Clone)]
pub struct ScreenPage(Rc<Parts>);

impl ScreenPage {
    pub fn new(window: &Window, machine: MachineHandle) -> Self {
        let title = adw::WindowTitle::new(&machine.borrow().name, "");
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&title));
        header.add_css_class("screen-header");
        let fullscreen_button = adw::SplitButton::builder()
            .icon_name("view-fullscreen-symbolic")
            .tooltip_text(format!("Fullscreen ({FULLSCREEN_KEYS})"))
            .build();
        name_for_readers(&fullscreen_button, "Fullscreen");
        header.pack_end(&fullscreen_button);
        // A button that took the focus from the screen would end the capture it starts.
        let capture_button =
            gtk::Button::builder().icon_name("input-mouse-symbolic").focus_on_click(false).build();
        name_for_readers(&capture_button, "Capture Mouse");
        header.pack_end(&capture_button);
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        let page = adw::NavigationPage::builder()
            .title(&machine.borrow().name)
            .tag("screen")
            .child(&toolbar)
            .build();
        let this = Self(Rc::new(Parts {
            window: window.clone(),
            machine,
            page,
            title,
            toolbar,
            fullscreen_button,
            capture_button,
            screen: RefCell::default(),
            clipboard: RefCell::default(),
            window_size: RefCell::default(),
            monitors: RefCell::default(),
            fullscreen_handler: RefCell::default(),
            thumbnail_source: RefCell::default(),
            header_called: Cell::default(),
            header_hid: Cell::default(),
            header_wait: RefCell::default(),
        }));
        this.make_screen();
        *this.0.monitors.borrow_mut() = Some(Monitors::new(window, &this.0.fullscreen_button));
        this.connect();
        this
    }

    fn memory_frames(&self) -> bool {
        let machine = self.0.machine.borrow();
        machine.has_gpu() && memory_frames(&self.0.window.context(), &machine)
    }

    fn run_folder(&self) -> std::path::PathBuf {
        let context = self.0.window.context();
        let slot = slot_of(&context, &self.0.machine.borrow());
        context.run_folder(slot)
    }

    fn make_screen(&self) {
        let weak = Rc::downgrade(&self.0);
        let on_capture = move || {
            if let Some(parts) = weak.upgrade() {
                ScreenPage(parts).on_capture_changed();
            }
        };
        let (route, overlay, direct) = {
            let machine = self.0.machine.borrow();
            (FrameRoute::of(&machine), machine.overlay, machine.direct_frames())
        };
        let context = self.0.window.context();
        // What the screen does with the frames, for a report. A log that can't be written is no
        // reason to stop showing them.
        let log = move |text: &str| {
            let _ = write_log(&context, text);
        };
        let screen =
            Screen::new(self.run_folder(), route, self.memory_frames(), overlay, direct, on_capture, log);
        self.0.toolbar.set_content(Some(screen.widget()));
        *self.0.screen.borrow_mut() = Some(screen);
    }

    /// A clone, so nothing stays borrowed while the screen calls back here.
    fn screen(&self) -> Option<Screen> {
        self.0.screen.borrow().clone()
    }

    fn connect(&self) {
        let parts = &self.0;
        let this = self.clone();
        parts.fullscreen_button.connect_clicked(move |_| this.toggle_fullscreen());
        let this = self.clone();
        parts.capture_button.connect_clicked(move |_| this.toggle_capture());
        let this = self.clone();
        let handler = parts.window.widget().connect_fullscreened_notify(move |_| this.on_fullscreened());
        *parts.fullscreen_handler.borrow_mut() = Some(handler);
        let motion = gtk::EventControllerMotion::new();
        let this = self.clone();
        motion.connect_motion(move |motion, _, y| {
            this.on_motion(y, motion.current_event_state().intersects(BUTTONS));
        });
        let this = self.clone();
        motion.connect_leave(move |_| this.on_pointer_left());
        parts.toolbar.add_controller(motion);
        let this = self.clone();
        parts.page.connect_shown(move |_| this.on_shown());
        let this = self.clone();
        parts.page.connect_hiding(move |_| this.on_hiding());
        let this = self.clone();
        parts.page.connect_hidden(move |_| this.on_hidden());
    }

    pub fn widget(&self) -> &adw::NavigationPage {
        &self.0.page
    }

    /// How many frames have been shown, for the page checks.
    pub fn frames_shown(&self) -> u64 {
        self.screen().map_or(0, |screen| screen.frames_shown())
    }

    /// What kind of texture is on screen, for the page checks.
    pub fn texture_kind(&self) -> String {
        self.screen().map(|screen| screen.texture_kind()).unwrap_or_default()
    }

    /// What QEMU's display has called, for the page checks.
    pub fn display_calls(&self) -> Vec<(String, u64)> {
        self.screen().map(|screen| screen.display_calls()).unwrap_or_default()
    }

    /// Whether the guest is taking input, for the page checks.
    pub fn has_input(&self) -> bool {
        self.screen().is_some_and(|screen| screen.has_input())
    }

    /// Whether the bar is over the screen, for the page checks.
    pub fn header_shown(&self) -> bool {
        self.0.toolbar.reveals_top_bars()
    }

    /// The pointer moving with no button held, this far below the top edge, for the page checks.
    pub fn move_pointer(&self, y: f64) {
        self.on_motion(y, false);
    }

    pub fn machine_id(&self) -> String {
        self.0.machine.borrow().id.clone()
    }

    pub fn toggle_fullscreen(&self) {
        let window = self.0.window.widget();
        if window.is_fullscreen() {
            window.unfullscreen();
            return;
        }
        let monitor = self.0.monitors.borrow().as_ref().and_then(|monitors| monitors.chosen());
        match monitor {
            Some(monitor) => window.fullscreen_on_monitor(&monitor),
            None => window.fullscreen(),
        }
        if let Some(screen) = self.screen() {
            screen.focus();
        }
    }

    pub fn toggle_capture(&self) {
        let Some(screen) = self.screen() else { return };
        match screen.capture_problem() {
            Some(problem) if !screen.capture_asked() => {
                self.0.window.toast(problem, adw::ToastPriority::Normal)
            }
            _ => screen.toggle_capture(),
        }
    }

    pub fn toggle_overlay(&self) {
        let wanted = {
            let mut machine = self.0.machine.borrow_mut();
            machine.overlay = !machine.overlay;
            machine.overlay
        };
        if let Some(screen) = self.screen() {
            screen.set_overlay(wanted);
        }
        self.0.window.save_machine(&self.0.machine.borrow());
    }

    fn on_capture_changed(&self) {
        if self.screen().is_some_and(|screen| screen.captured()) {
            // In fullscreen this is all the user sees of the way out, so it waits for no other toast.
            self.0
                .window
                .toast(&format!("Mouse captured. {CAPTURE_KEYS} releases it."), adw::ToastPriority::High);
            self.call_header(false);
        }
        self.show_hint(None);
    }

    fn on_fullscreened(&self) {
        let parts = &self.0;
        let fullscreen = parts.window.widget().is_fullscreen();
        self.call_header(false);
        let button = &parts.fullscreen_button;
        button.set_icon_name(if fullscreen { "view-restore-symbolic" } else { "view-fullscreen-symbolic" });
        button.set_tooltip_text(Some(&if fullscreen {
            format!("Leave Fullscreen ({FULLSCREEN_KEYS})")
        } else {
            format!("Fullscreen ({FULLSCREEN_KEYS})")
        }));
        if fullscreen {
            let text = format!("{FULLSCREEN_KEYS} leaves fullscreen. So does the top edge of the screen.");
            parts.window.toast(&text, adw::ToastPriority::Normal);
        }
    }

    /// Whether the header is out of the way unless the pointer calls it. Not before there's a screen
    /// to show: until then the header is all there is to read.
    fn header_hides(&self) -> bool {
        let fullscreen = self.0.window.widget().is_fullscreen();
        let has_frames = self.screen().is_some_and(|screen| screen.has_frames());
        fullscreen || (self.0.machine.borrow().bar_hides() && has_frames)
    }

    /// Shows the header or not, by what the machine and the window say and where the pointer was.
    /// The screen always reaches the top edge when the header can hide, so the picture stays where
    /// it is as the header comes and goes.
    fn follow_header(&self) {
        let parts = &self.0;
        let hides = self.header_hides();
        if !hides {
            parts.header_called.set(false);
            self.forget_header_wait();
        }
        let fullscreen = parts.window.widget().is_fullscreen();
        if hides && !fullscreen && !parts.header_hid.get() {
            let text = "The bar hides. Rest the pointer at the top edge of the window to bring it back.";
            parts.window.toast(text, adw::ToastPriority::Normal);
        }
        parts.header_hid.set(hides);
        if parts.toolbar.is_extend_content_to_top_edge() != hides {
            parts.toolbar.set_extend_content_to_top_edge(hides);
        }
        let reveal = !hides || parts.header_called.get();
        if parts.toolbar.reveals_top_bars() != reveal {
            parts.toolbar.set_reveal_top_bars(reveal);
        }
    }

    fn call_header(&self, called: bool) {
        self.forget_header_wait();
        self.0.header_called.set(called);
        self.follow_header();
    }

    fn forget_header_wait(&self) {
        if let Some(source) = self.0.header_wait.borrow_mut().take() {
            source.remove();
        }
    }

    /// Calls the header, or sends it away, once `after` has passed with nothing else happening.
    fn call_header_after(&self, after: Duration, called: bool) {
        self.forget_header_wait();
        let weak = Rc::downgrade(&self.0);
        let source = glib::timeout_add_local_once(after, move || {
            if let Some(parts) = weak.upgrade() {
                // It has run, so there is nothing left to remove.
                parts.header_wait.borrow_mut().take();
                ScreenPage(parts).call_header(called);
            }
        });
        *self.0.header_wait.borrow_mut() = Some(source);
    }

    fn on_motion(&self, y: f64, button_held: bool) {
        let parts = &self.0;
        if !self.header_hides() {
            return;
        }
        if parts.header_called.get() {
            let height = parts.toolbar.top_bar_height();
            let top_bar = if height > 0 { height } else { USUAL_HEADER_HEIGHT };
            if y > f64::from(top_bar) {
                self.call_header(false);
            } else {
                // Back on the header after leaving the window.
                self.forget_header_wait();
            }
            return;
        }
        let fullscreen = parts.window.widget().is_fullscreen();
        let captured = self.screen().is_some_and(|screen| screen.captured());
        // A held button is a drag in the guest, which the header would land in the way of.
        if captured || (button_held && !fullscreen) {
            self.forget_header_wait();
        } else if fullscreen {
            if y <= TOP_EDGE {
                self.call_header(true);
            }
        } else if y > WINDOW_TOP_EDGE {
            self.forget_header_wait();
        } else if parts.header_wait.borrow().is_none() {
            self.call_header_after(REST, true);
        }
    }

    /// The pointer left the window, or went into one of the header's menus.
    fn on_pointer_left(&self) {
        let parts = &self.0;
        if !parts.header_called.get() {
            self.forget_header_wait();
            return;
        }
        let menu_open = parts.fullscreen_button.popover().is_some_and(|menu| menu.is_visible());
        // In fullscreen the pointer has nowhere to go but back.
        if !menu_open && !parts.window.widget().is_fullscreen() {
            self.call_header_after(LEFT, false);
        }
    }

    fn on_shown(&self) {
        if let Some(screen) = self.screen() {
            screen.page_shown();
        }
        self.show_hint(None);
        let machine = self.0.machine.borrow().clone();
        if machine.sizes_to_window()
            && let Some(screen) = self.screen()
        {
            let context = self.0.window.context();
            let slot = slot_of(&context, &machine);
            *self.0.window_size.borrow_mut() = WindowSize::new(screen, context, machine.clone(), slot);
        }
        if machine.clipboard {
            let context = self.0.window.context();
            let slot = slot_of(&context, &machine);
            let shared = SharedClipboard::new(self.0.window.widget(), context, machine, slot);
            *self.0.clipboard.borrow_mut() = Some(shared);
        }
        let this = self.clone();
        let source = glib::timeout_add_local(THUMBNAIL_EVERY, move || {
            this.save_thumbnail();
            glib::ControlFlow::Continue
        });
        *self.0.thumbnail_source.borrow_mut() = Some(source);
    }

    fn thumbnail(&self) -> Option<Vec<u8>> {
        self.screen()?.thumbnail(THUMBNAIL_WIDTH)
    }

    /// A thumbnail isn't worth interrupting anyone for, so what goes wrong only goes in the log.
    fn save_thumbnail(&self) {
        let Some(png) = self.thumbnail() else { return };
        let context = self.0.window.context();
        let machine = self.0.machine.borrow().clone();
        let working = context.clone();
        let window = self.0.window.clone();
        let id = machine.id.clone();
        in_background(
            &context,
            move || save_thumbnail(&working, &machine, &png),
            move |result| match result {
                Ok(()) => window.show_thumbnail(&id),
                Err(text) => {
                    let _ = write_log(&window.context(), &format!("thumbnail: {text}"));
                }
            },
        );
    }

    /// For the app closing, when a thread might not get to finish.
    pub fn save_thumbnail_now(&self) {
        if let Some(png) = self.thumbnail() {
            let context = self.0.window.context();
            if let Err(failure) = save_thumbnail(&context, &self.0.machine.borrow(), &png) {
                let _ = write_log(&context, &format!("thumbnail: {failure}"));
            }
        }
    }

    /// The frame is still there now, and it's gone once the page is hidden.
    fn on_hiding(&self) {
        if let Some(screen) = self.screen() {
            screen.page_leaving();
        }
        if let Some(source) = self.0.thumbnail_source.borrow_mut().take() {
            source.remove();
        }
        self.save_thumbnail();
    }

    fn on_hidden(&self) {
        let parts = &self.0;
        parts.clipboard.borrow_mut().take();
        self.forget_header_wait();
        parts.window_size.borrow_mut().take();
        parts.window.widget().unfullscreen();
        if let Some(handler) = parts.fullscreen_handler.borrow_mut().take() {
            parts.window.widget().disconnect(handler);
        }
        if let Some(monitors) = parts.monitors.borrow_mut().take() {
            monitors.close();
        }
        parts.screen.borrow_mut().take();
        parts.window.screen_closed();
    }

    /// What is going on, under the machine's name.
    pub fn show_hint(&self, status: Option<&Status>) {
        let Some(screen) = self.screen() else { return };
        if let Some(status) = status {
            screen.set_memory_frames(self.memory_frames());
            screen.set_direct_frames(self.0.machine.borrow().direct_frames());
            screen.follow_machine(self.run_folder(), FrameRoute::now(&self.0.machine.borrow(), status));
        }
        let hint = match status {
            Some(status) if status.state == State::Installing => status.step.clone(),
            _ if !screen.has_frames() => "Waiting for the screen".to_owned(),
            _ if !screen.has_input() => "Another viewer has the keyboard and mouse".to_owned(),
            _ if screen.captured() => format!("Mouse captured. {CAPTURE_KEYS} releases it."),
            _ if screen.capture_asked() => "Move the pointer into the window to capture it".to_owned(),
            _ if screen.shortcuts_inhibited() => {
                "Keys go to the guest. Another window takes them back".to_owned()
            }
            _ => "Click the screen to send it keys".to_owned(),
        };
        if hint != self.0.title.subtitle() {
            self.0.title.set_subtitle(&hint);
        }
        self.follow_header();
        let problem = screen.capture_problem();
        self.0.capture_button.set_sensitive(problem.is_none());
        let tooltip = format!("Capture Mouse ({CAPTURE_KEYS})");
        self.0.capture_button.set_tooltip_text(Some(problem.unwrap_or(&tooltip)));
    }
}
