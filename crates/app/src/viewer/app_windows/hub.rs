//! The machine's app windows in one process. The hub owns what there is one of for the machine, the
//! frames and the input (in a screen that is in no window), and the link to the guest's helper. It
//! keeps one view for each cell that is shown, and hands every frame to all of them.

use super::cell_picture::Guest;
use super::plan::{ScaleFollow, SizeFollow, Step, focus_request, scale_to_follow, steps};
use super::secure::SecureWindow;
use super::view::{View, add_style};
use crate::viewer::screen::Screen;
use crate::viewer::slot_hold::HeldFrame;
use crate::viewer::window_size::GuestSizer;
use adw::prelude::*;
use gtk::{gdk, gio, glib};
use guesthouse_app_core::app_windows::{
    Cell, Desktop, Fit, MachineLink, Point, Request, Size, Update, WINDOWS_STEPS,
};
use guesthouse_app_core::context::Context;
use guesthouse_app_core::log::write_log;
use std::cell::{Cell as Slot, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// How often the link to the helper is looked at. Each look is a drain of a channel.
const LOOK: Duration = Duration::from_millis(15);
/// How often the machine is asked whether it still runs.
const CHECK_RUNNING: Duration = Duration::from_secs(2);

pub struct Hub {
    application: adw::Application,
    context: Arc<Context>,
    machine: String,
    screen: Screen,
    link: RefCell<Option<MachineLink>>,
    views: RefCell<BTreeMap<String, Rc<View>>>,
    hosted: RefCell<BTreeMap<String, bool>>,
    /// The cell the guest was last told has the focus.
    focus_sent: RefCell<Option<String>>,
    trace: bool,
    ended: Slot<bool>,
    hold: RefCell<Option<gio::ApplicationHoldGuard>>,
    mismatch_said: Slot<bool>,
    /// The guest's input desktop is the normal one, where the cells mean what they say.
    input_allowed: Slot<bool>,
    /// What the views were last told of Windows' scale.
    guest: RefCell<Guest>,
    fit: Fit,
    scale_follow: RefCell<ScaleFollow>,
    size_follow: RefCell<SizeFollow>,
    sizer: Sizer,
    /// The floor and ceiling the helper was last told, cleared when the link comes up again.
    screen_said: RefCell<Option<(Size, Size)>>,
    /// The window that last had the focus.
    focus_id: RefCell<Option<String>>,
    /// The monitor scale Windows was last made to follow, for the log.
    focus_scale: Slot<Option<f64>>,
    /// The guest's screen as the helper last said it, so a new mode is seen.
    screen_seen: Slot<Option<Size>>,
    /// The newest frame and the guest's SDR white with it, kept so a window that opens or comes back
    /// into sight shows it at once on a still guest. It holds its slot until the next frame.
    newest: RefCell<Option<(HeldFrame, Option<u32>)>>,
    /// The one window with the guest's whole frame, while its input desktop is not the normal one.
    secure: RefCell<Option<Rc<SecureWindow>>>,
}

use std::sync::Arc;

impl Hub {
    pub fn start(
        application: &adw::Application,
        context: Arc<Context>,
        machine: String,
        screen: Screen,
        link: MachineLink,
        sizer: Sizer,
        running: impl Fn() -> bool + 'static,
    ) -> Rc<Self> {
        let hub = Rc::new(Self {
            application: application.clone(),
            context,
            machine,
            screen,
            link: RefCell::new(Some(link)),
            views: RefCell::default(),
            hosted: RefCell::default(),
            focus_sent: RefCell::default(),
            trace: std::env::var_os("GUESTHOUSE_APP_WINDOWS_TRACE").is_some(),
            ended: Slot::new(false),
            hold: RefCell::new(Some(application.hold())),
            mismatch_said: Slot::new(false),
            input_allowed: Slot::new(true),
            guest: RefCell::default(),
            fit: fit_from_environment(),
            scale_follow: RefCell::default(),
            size_follow: RefCell::default(),
            sizer,
            screen_said: RefCell::default(),
            focus_id: RefCell::default(),
            focus_scale: Slot::new(None),
            screen_seen: Slot::new(None),
            newest: RefCell::default(),
            secure: RefCell::new(None),
        });
        add_style(&gdk::Display::default().expect("a display"));
        let (weak, ticking) = (Rc::downgrade(&hub), Rc::downgrade(&hub));
        hub.screen.start_headless(
            move |frame, sdr_white_nits| {
                if let Some(hub) = weak.upgrade() {
                    hub.on_frame(frame, sdr_white_nits);
                }
            },
            move || {
                if let Some(hub) = ticking.upgrade() {
                    hub.tick();
                }
            },
        );
        let weak = Rc::downgrade(&hub);
        glib::timeout_add_local(LOOK, move || match weak.upgrade() {
            Some(hub) if !hub.ended.get() => {
                hub.look();
                glib::ControlFlow::Continue
            }
            _ => glib::ControlFlow::Break,
        });
        let weak = Rc::downgrade(&hub);
        glib::timeout_add_local(CHECK_RUNNING, move || match weak.upgrade() {
            Some(hub) if !hub.ended.get() => {
                if !running() {
                    hub.note("the machine has stopped, so the app windows end");
                    hub.end();
                    return glib::ControlFlow::Break;
                }
                glib::ControlFlow::Continue
            }
            _ => glib::ControlFlow::Break,
        });
        hub.note(&format!("app windows for {}: started", hub.machine));
        hub
    }

    pub fn application(&self) -> &adw::Application {
        &self.application
    }

    /// Windows' scale as the views are to go by.
    pub fn guest(&self) -> Guest {
        self.guest.borrow().clone()
    }

    /// What the helper says of Windows' scale now.
    fn guest_from_model(&self) -> Guest {
        let link = self.link.borrow();
        let model = link.as_ref().map(|link| link.client.model());
        let percent = model.map_or(100, |model| model.scale_percent());
        let offered = model.and_then(|model| model.steps()).map_or(WINDOWS_STEPS.to_vec(), <[u32]>::to_vec);
        Guest { percent, offered, fit: self.fit }
    }

    /// Asks Windows for the step the focused window's monitor wants, once it has stayed there; with no
    /// focus, the step of the monitor the windows are on (`scale_to_follow`). Only a helper that
    /// reported the steps it offers is asked.
    fn follow_scale(&self) {
        let windows: Vec<Rc<View>> =
            self.views.borrow().values().filter(|view| !view.is_popup()).cloned().collect();
        if let Some(active) = windows.iter().find(|view| view.is_active()) {
            self.focus_id.replace(Some(active.id.clone()));
        }
        let focused = self.focus_id.borrow().as_ref().and_then(|id| self.views.borrow().get(id).cloned());
        let shown: Vec<f64> = windows.iter().filter_map(|view| view.known_scale()).collect();
        let scale =
            scale_to_follow(focused.and_then(|view| view.known_scale()), &shown, largest_monitor_scale());
        self.focus_scale.set(scale);
        let (guest, offered, screen) = {
            let link = self.link.borrow();
            let Some(model) = link.as_ref().map(|link| link.client.model()) else { return };
            (model.scale_percent(), model.steps().map(<[u32]>::to_vec).unwrap_or_default(), model.screen())
        };
        if self.screen_seen.replace(Some(screen)).is_some_and(|seen| seen != screen) {
            self.scale_follow.borrow_mut().screen_changed();
        }
        let step =
            self.scale_follow.borrow_mut().look(self.focus_scale.get(), guest, &offered, Instant::now());
        if let Some(percent) = step {
            self.note(&format!(
                "the windows are to be right for a monitor at {:?}: Windows is asked for {percent} %",
                self.focus_scale.get()
            ));
            self.request(Request::Scale { percent });
        }
    }

    /// Tells every view when Windows' scale changed.
    fn follow_guest(&self) {
        let now = self.guest_from_model();
        if *self.guest.borrow() == now {
            return;
        }
        self.note(&format!("Windows' scale is {} %", now.percent));
        self.guest.replace(now.clone());
        let views: Vec<Rc<View>> = self.views.borrow().values().cloned().collect();
        for view in views {
            view.set_guest(now.clone());
        }
        if let Some(secure) = self.secure.borrow().as_ref() {
            secure.set_guest(now.clone());
        }
    }

    /// Tells the helper the smallest and largest screen the host wants, when they change, and sets the
    /// screen it then says its windows need.
    fn follow_screen(&self) {
        let focused = self.views.borrow().values().find(|view| view.is_active()).cloned();
        let Some((floor, ceiling)) = screen_bounds(focused.as_deref()) else { return };
        if *self.screen_said.borrow() != Some((floor, ceiling)) {
            self.screen_said.replace(Some((floor, ceiling)));
            self.note(&format!(
                "the guest's screen may be from {}x{} to {}x{}",
                floor.w, floor.h, ceiling.w, ceiling.h
            ));
            self.request(Request::Screen { floor, ceiling });
        }
        let (need, screen) = {
            let link = self.link.borrow();
            let Some(model) = link.as_ref().map(|link| link.client.model()) else { return };
            (model.need(), model.screen())
        };
        let Some(size) = self.size_follow.borrow_mut().look(need, screen, Instant::now()) else { return };
        self.note(&format!("the guest's windows need a screen of {}x{}", size.w, size.h));
        self.sizer.ask(&self.context, size);
    }

    /// A view's surface was given another scale by the compositor: a monitor's scale changed, or the
    /// window went to a monitor of another scale. Its popups go by its ratio.
    pub fn scale_changed(&self, id: &str) {
        let Some(view) = self.views.borrow().get(id).cloned() else { return };
        let children: Vec<Rc<View>> = view
            .children
            .borrow()
            .iter()
            .filter_map(|child| self.views.borrow().get(child).cloned())
            .collect();
        for child in children {
            child.replace_popup(&view);
        }
    }

    /// For the log, and for a lab that reads what the process says as it goes.
    pub fn note(&self, text: &str) {
        let _ = write_log(&self.context, &format!("app windows: {text}"));
        if self.trace {
            eprintln!("app windows: {text}");
        }
    }

    /// For a lab only: what an event did, a line each.
    pub fn trace(&self, text: &str) {
        if self.trace {
            eprintln!("app windows: {text}");
        }
    }

    pub fn screen_size(&self) -> Size {
        let link = self.link.borrow();
        link.as_ref().map_or(Size { w: 1920, h: 1080 }, |link| link.client.model().screen())
    }

    pub fn request(&self, request: Request) {
        let kind = request.kind();
        let sent = self.link.borrow_mut().as_mut().map(|link| link.client.request(request));
        self.trace(&format!("request {kind} sent as {sent:?}"));
    }

    /// A host window got the focus, so the guest's window becomes the foreground one.
    pub fn focus(&self, cell: &Cell) {
        // The window with the whole frame has the guest's input while it is there.
        if self.secure.borrow().is_some() {
            return;
        }
        let request = focus_request(self.focus_sent.borrow().as_deref(), cell);
        if let Some(request) = request {
            *self.focus_sent.borrow_mut() = Some(cell.id.clone());
            self.request(request);
        }
    }

    /// A host window lost the focus: nothing it held down in the guest stays down.
    pub fn lost_focus(&self, id: &str) {
        if self.focus_sent.borrow().as_deref() == Some(id) {
            *self.focus_sent.borrow_mut() = None;
        }
        self.screen.release_everything();
    }

    pub fn send_pointer(&self, point: Point) {
        if !self.input_allowed.get() {
            return;
        }
        self.screen.send_pointer(point, self.screen_size());
    }

    pub fn send_button(&self, name: &'static str, down: bool) {
        if !self.input_allowed.get() && down {
            return;
        }
        self.screen.send_button(name, down);
    }

    pub fn send_key(&self, keycode: u32, down: bool) {
        if !self.input_allowed.get() && down {
            return;
        }
        self.screen.send_key(keycode, down);
    }

    /// The pointer, buttons and keys of the window that shows the secure desktop: the only window the
    /// guest takes input from while its input desktop is not the normal one.
    pub fn secure_pointer(&self, point: Point) {
        self.screen.send_pointer(point, self.screen_size());
    }

    pub fn secure_button(&self, name: &'static str, down: bool) {
        self.screen.send_button(name, down);
    }

    pub fn secure_key(&self, keycode: u32, down: bool) {
        self.screen.send_key(keycode, down);
    }

    pub fn scroll_controller(&self) -> gtk::EventControllerScroll {
        self.screen.scroll_controller(gtk::PropagationPhase::Bubble)
    }

    /// Every view puts the new frame on its own surface, one after the other with nothing between,
    /// so the windows of one program change together. None holds an old one back from the producer.
    fn on_frame(&self, frame: HeldFrame, sdr_white_nits: Option<u32>) {
        let screen = self.screen_size();
        let size = frame.size();
        if screen.w > 0 && size != (screen.w, screen.h) && !self.mismatch_said.replace(true) {
            self.note(&format!(
                "the frames are {}x{} and the helper says the screen is {}x{}",
                size.0, size.1, screen.w, screen.h
            ));
        }
        let cursor = self.screen.guest_pointer_cursor();
        let views: Vec<Rc<View>> = self.views.borrow().values().cloned().collect();
        for view in &views {
            view.show(self, &frame, sdr_white_nits);
            view.set_cursor(cursor.as_ref());
        }
        if let Some(secure) = self.secure.borrow().as_ref() {
            secure.show(self, &frame, sdr_white_nits);
            secure.set_cursor(cursor.as_ref());
        }
        self.newest.replace(Some((frame, sdr_white_nits)));
    }

    /// Each view takes in what the compositor gave back, and one that is behind shows the newest frame.
    fn tick(&self) {
        let views: Vec<Rc<View>> = self.views.borrow().values().cloned().collect();
        let newest = self.newest.borrow();
        let (frame, sdr_white_nits) = match newest.as_ref() {
            Some((frame, nits)) => (Some(frame), *nits),
            None => (None, None),
        };
        for view in views {
            view.tick(self, frame, sdr_white_nits);
        }
        let secure = self.secure.borrow().clone();
        if let Some(secure) = secure {
            secure.tick(self, frame, sdr_white_nits);
        }
    }

    /// One look at what the helper has said.
    fn look(self: &Rc<Self>) {
        let updates = match self.link.borrow_mut().as_mut() {
            Some(link) => link.client.poll(),
            None => return,
        };
        self.follow_guest();
        self.follow_scale();
        self.follow_screen();
        for update in updates {
            if matches!(update, Update::Status(_)) {
                self.screen_said.replace(None);
            }
            match update {
                Update::Status(status) => self.note(&format!("the link is {status:?}")),
                Update::Note(text) => self.note(&text),
                Update::Answered { n, result } => {
                    if let Err(failure) = result {
                        self.note(&format!("request {n} failed: {failure:?}"));
                    }
                }
                Update::Changes(changes) => {
                    let found = {
                        let link = self.link.borrow();
                        let Some(link) = link.as_ref() else { return };
                        steps(&changes, link.client.model(), &mut self.hosted.borrow_mut())
                    };
                    for step in found {
                        self.apply(step);
                    }
                }
            }
        }
    }

    fn cell_now(&self, id: &str) -> Option<Cell> {
        self.link.borrow().as_ref()?.client.model().get(id).cloned()
    }

    fn apply(self: &Rc<Self>, step: Step) {
        match step {
            Step::Restart => {
                self.note("the helper started again, so every window goes");
                self.close_all();
                // What desktop it is on is said again after its hello.
                self.show_desktop(Desktop::Normal);
            }
            Step::Open(id) => {
                let Some(cell) = self.cell_now(&id) else { return };
                let parent = if cell.popup { self.views.borrow().get(&cell.parent).cloned() } else { None };
                if cell.popup && parent.is_none() {
                    return;
                }
                let view = View::open(self, &cell, parent.as_ref());
                self.note(&format!(
                    "opened {} {} at {:?} titled {:?}",
                    if cell.popup { "popup" } else { "window" },
                    id,
                    cell.cell,
                    cell.title
                ));
                view.take_pointer(self.secure.borrow().is_none());
                self.views.borrow_mut().insert(id, view);
            }
            Step::Close(id) => self.close(&id),
            Step::Update { id, what } => {
                let (Some(view), Some(cell)) = (self.views.borrow().get(&id).cloned(), self.cell_now(&id))
                else {
                    return;
                };
                let parent = self.views.borrow().get(&cell.parent).cloned().filter(|_| cell.popup);
                view.update(self, &cell, what, parent.as_ref());
            }
            Step::Screen(size) => {
                self.note(&format!("the guest's screen is {}x{}", size.w, size.h));
                let views: Vec<Rc<View>> = self.views.borrow().values().cloned().collect();
                for view in views {
                    view.recrop(size);
                }
                if let Some(secure) = self.secure.borrow().as_ref() {
                    secure.recrop(size);
                }
            }
            Step::Desktop(desktop) => self.show_desktop(desktop),
        }
    }

    /// The guest's input desktop is `desktop`. Not the normal one: one window shows the whole frame and
    /// has the input, and the app windows stay as they are and take none. The normal one again: that
    /// window goes and the app window that had the focus has it again.
    fn show_desktop(self: &Rc<Self>, desktop: Desktop) {
        self.input_allowed.set(desktop == Desktop::Normal);
        self.screen.release_everything();
        let views: Vec<Rc<View>> = self.views.borrow().values().cloned().collect();
        for view in &views {
            view.take_pointer(desktop == Desktop::Normal);
        }
        let old = self.secure.borrow_mut().take();
        let came_from = old
            .as_ref()
            .map(|old| old.came_from.clone())
            .unwrap_or_else(|| views.iter().find(|view| view.is_active()).map(|view| view.id.clone()));
        if let Some(old) = &old {
            old.close();
        }
        if desktop == Desktop::Normal {
            if old.is_some() {
                self.note("the guest's input desktop is normal again");
            }
            if old.is_some()
                && let Some(view) = came_from.and_then(|id| self.views.borrow().get(&id).cloned())
            {
                view.bring_forward();
            }
            return;
        }
        self.note(&format!(
            "the guest's input desktop is {desktop:?}: one window shows the whole screen, the app windows take no input"
        ));
        let window = SecureWindow::open(self, desktop, None, came_from);
        *self.secure.borrow_mut() = Some(window);
    }

    /// The view and the popups that hang on it, popups first.
    fn close(&self, id: &str) {
        let Some(view) = self.views.borrow().get(id).cloned() else { return };
        let children: Vec<String> = view.children.borrow().clone();
        for child in children {
            self.close(&child);
        }
        self.views.borrow_mut().remove(id);
        self.lost_focus_of_closing(id);
        view.destroy();
        self.note(&format!("closed {id}"));
    }

    fn lost_focus_of_closing(&self, id: &str) {
        // What the closing window held down in the guest is let go of with it.
        if self.focus_sent.borrow().as_deref() == Some(id) {
            *self.focus_sent.borrow_mut() = None;
        }
        self.screen.release_everything();
    }

    fn close_all(&self) {
        let roots: Vec<String> = self
            .views
            .borrow()
            .values()
            .filter(|view| !view.is_popup())
            .map(|view| view.id.clone())
            .collect();
        for id in roots {
            self.close(&id);
        }
        self.views.borrow_mut().clear();
        self.hosted.borrow_mut().clear();
    }

    /// Ends the process's work: whatever is held in the guest is let go, the windows go, and so does
    /// the application, which had been held so that it lives with no window.
    pub fn end(&self) {
        if self.ended.replace(true) {
            return;
        }
        self.screen.release_everything();
        if let Some(secure) = self.secure.borrow_mut().take() {
            secure.close();
        }
        self.close_all();
        self.newest.borrow_mut().take();
        self.screen.end_headless();
        self.link.borrow_mut().take();
        self.hold.borrow_mut().take();
        self.application.quit();
    }
}

/// Lab only: `GUESTHOUSE_APP_WINDOWS_FIT=size` shows a window on a monitor whose scale Windows is
/// not exactly at with the size Windows means, scaled by the compositor, rather than sharp.
fn fit_from_environment() -> Fit {
    match std::env::var("GUESTHOUSE_APP_WINDOWS_FIT").as_deref() {
        Ok("size") => Fit::Size,
        _ => Fit::Sharp,
    }
}

/// Where a size the guest needs goes.
pub enum Sizer {
    /// The route the whole-desktop viewer sizes the guest by.
    Guest(GuestSizer),
    /// Lab only: the size is written to this file, for a stand-in to take.
    File(std::path::PathBuf),
    /// A machine with no display driver keeps its screen.
    Not,
}

impl Sizer {
    fn ask(&self, context: &Context, size: Size) {
        let wanted = (size.w.max(0) as u32, size.h.max(0) as u32);
        match self {
            Self::Guest(sizer) => {
                let (w, h) = sizer.ask(wanted);
                let _ =
                    write_log(context, &format!("app windows: the guest's screen is asked to be {w}x{h}"));
            }
            Self::File(path) => {
                let _ = std::fs::write(path, format!("{} {}", wanted.0, wanted.1));
            }
            Self::Not => {}
        }
    }
}

/// The scale of the largest monitor, for when no window is on a surface yet.
fn largest_monitor_scale() -> Option<f64> {
    let monitors = gdk::Display::default()?.monitors();
    (0..monitors.n_items())
        .filter_map(|index| monitors.item(index).and_then(|item| item.downcast::<gdk::Monitor>().ok()))
        .max_by_key(|monitor| {
            let geometry = monitor.geometry();
            (f64::from(geometry.width()) * f64::from(geometry.height()) * monitor.scale() * monitor.scale())
                as i64
        })
        .map(|monitor| monitor.scale())
}

/// The largest the screen is wanted (every monitor side by side, in its own pixels, within what the
/// driver takes) and the smallest (the monitor of the window that has the focus, or the largest, in
/// its own pixels): a window maximized in the guest is then one monitor's worth, and Windows offers
/// the steps it would offer on that monitor.
fn screen_bounds(focused: Option<&View>) -> Option<(Size, Size)> {
    let display = gdk::Display::default()?;
    let monitors = display.monitors();
    let mut all = Vec::new();
    for index in 0..monitors.n_items() {
        let Some(monitor) = monitors.item(index).and_then(|item| item.downcast::<gdk::Monitor>().ok()) else {
            continue;
        };
        all.push(monitor);
    }
    let device = |monitor: &gdk::Monitor| {
        let geometry = monitor.geometry();
        let scale = monitor.scale();
        Size {
            w: (f64::from(geometry.width()) * scale).round() as i32,
            h: (f64::from(geometry.height()) * scale).round() as i32,
        }
    };
    let largest = all.iter().map(device).max_by_key(|size| i64::from(size.w) * i64::from(size.h))?;
    let floor = focused.and_then(|view| view.monitor()).map_or(largest, |monitor| device(&monitor));
    let ceiling = Size {
        w: all.iter().map(|monitor| device(monitor).w).sum::<i32>().min(LARGEST_SCREEN.w),
        h: all.iter().map(|monitor| device(monitor).h).max().unwrap_or(floor.h).min(LARGEST_SCREEN.h),
    };
    Some((floor, ceiling))
}

/// What the display driver and Windows take at the most.
const LARGEST_SCREEN: Size = Size { w: 7680, h: 4320 };
