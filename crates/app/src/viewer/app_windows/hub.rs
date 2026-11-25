//! The machine's app windows in one process. The hub owns what there is one of for the machine, the
//! frames and the input (in a screen that is in no window), and the link to the guest's helper. It
//! keeps one view for each cell that is shown, and hands every frame to all of them.

use super::cell_picture::Guest;
use super::plan::{ScaleFollow, Step, focus_request, steps};
use super::view::{View, add_style};
use crate::viewer::screen::Screen;
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
    /// The scale of the surface of the window that last had the focus.
    focus_scale: Slot<Option<f64>>,
}

use std::sync::Arc;

impl Hub {
    pub fn start(
        application: &adw::Application,
        context: Arc<Context>,
        machine: String,
        screen: Screen,
        link: MachineLink,
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
            focus_scale: Slot::new(None),
        });
        add_style(&gdk::Display::default().expect("a display"));
        let weak = Rc::downgrade(&hub);
        hub.screen.start_headless(move |texture| {
            if let Some(hub) = weak.upgrade() {
                hub.on_texture(texture);
            }
        });
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

    /// Asks Windows for the step the focused window's monitor wants, once it has stayed there. Only a
    /// helper that reported the steps it offers is asked.
    fn follow_scale(&self) {
        let views: Vec<Rc<View>> = self.views.borrow().values().cloned().collect();
        if let Some(active) = views.iter().find(|view| view.is_active()) {
            self.focus_scale.set(Some(active.surface_scale()));
        }
        let (guest, offered) = {
            let link = self.link.borrow();
            let Some(model) = link.as_ref().map(|link| link.client.model()) else { return };
            (model.scale_percent(), model.steps().map(<[u32]>::to_vec).unwrap_or_default())
        };
        let step =
            self.scale_follow.borrow_mut().look(self.focus_scale.get(), guest, &offered, Instant::now());
        if let Some(percent) = step {
            self.note(&format!(
                "the focused window is on a monitor at {:?}: Windows is asked for {percent} %",
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

    pub fn latest(&self) -> Option<gdk::Texture> {
        self.screen.latest()
    }

    pub fn request(&self, request: Request) {
        let kind = request.kind();
        let sent = self.link.borrow_mut().as_mut().map(|link| link.client.request(request));
        self.trace(&format!("request {kind} sent as {sent:?}"));
    }

    /// A host window got the focus, so the guest's window becomes the foreground one.
    pub fn focus(&self, cell: &Cell) {
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

    pub fn scroll_controller(&self) -> gtk::EventControllerScroll {
        self.screen.scroll_controller(gtk::PropagationPhase::Bubble)
    }

    /// Every view shows the new frame, so none holds an old one back from the producer.
    fn on_texture(&self, texture: &gdk::Texture) {
        let screen = self.screen_size();
        if screen.w > 0
            && (texture.width(), texture.height()) != (screen.w, screen.h)
            && !self.mismatch_said.replace(true)
        {
            self.note(&format!(
                "the frames are {}x{} and the helper says the screen is {}x{}",
                texture.width(),
                texture.height(),
                screen.w,
                screen.h
            ));
        }
        let cursor = self.screen.guest_pointer_cursor();
        let views: Vec<Rc<View>> = self.views.borrow().values().cloned().collect();
        for view in views {
            view.set_texture(texture);
            view.set_cursor(cursor.as_ref());
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
        for update in updates {
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
            }
            Step::Desktop(desktop) => {
                // First milestone: the windows stay as they are and take what comes, which for a
                // secure desktop is a picture that no longer matches them. Said, not hidden.
                self.input_allowed.set(desktop == Desktop::Normal);
                if desktop != Desktop::Normal {
                    self.screen.release_everything();
                    self.note(&format!(
                        "the guest's input desktop is {desktop:?}: the windows stay as they are, and take no pointer"
                    ));
                } else {
                    self.note("the guest's input desktop is normal again");
                }
            }
        }
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
        self.close_all();
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
