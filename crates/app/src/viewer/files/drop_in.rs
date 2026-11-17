//! Files dropped on the guest's screen.
//!
//! Windows only takes a drop from a drag of its own. So the files are copied to the guest as soon
//! as the drag comes over the screen, the helper there puts a window under the guest's pointer, and
//! a press from here starts a real drag of them in Windows. From then on this PC's drag moves the
//! guest's: its windows light up as the pointer crosses them, and the drop is the release.

use super::super::screen::Screen;
use super::{Link, off_thread};
use crate::window::Window;
use adw::prelude::*;
use gtk::{gdk, gio, glib};
use guesthouse_app_core::guest_files::{DragState, Until, dropped_files, send};
use std::cell::{Cell, RefCell};
use std::io::ErrorKind;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

const LEFT: &str = "left";
/// The press reaches the helper in a few milliseconds if it reaches it at all.
const START_PATIENCE: Duration = Duration::from_secs(2);
/// What takes the files may ask the user something first, as Explorer does over a file of the
/// same name.
const END_PATIENCE: Duration = Duration::from_secs(20);
const CANCEL_PATIENCE: Duration = Duration::from_secs(1);
const BEFORE_RELEASE: Duration = Duration::from_millis(120);
const ON_DESKTOP: &str = "Windows didn't take the files there, so they are on its Desktop";
const NOT_READY: &str = "Windows isn't ready for files yet. Someone has to be logged in";
const NOTHING_TO_SEND: &str = "Only files and folders on this PC can be dropped on a guest";
const NO_ANSWER: &str = "Windows didn't say what became of the files";

static DROPS: AtomicU32 = AtomicU32::new(0);

/// One drag over the screen, from when it enters.
struct Delivery {
    /// The drop's name in the guest.
    drop: String,
    /// The copy to the guest has begun, which it does once GTK has read what the drag carries.
    started: Cell<bool>,
    /// Ends the copy to the guest.
    stop: Arc<AtomicBool>,
    /// Where the drag is over the picture.
    at: Cell<(f64, f64)>,
    /// The drag went away without a drop.
    left: Cell<bool>,
    dropped_at: Cell<Option<(f64, f64)>>,
    /// Wakes the delivery when the drag is dropped or leaves.
    decided: async_channel::Sender<()>,
    wake: async_channel::Receiver<()>,
}

impl Delivery {
    fn new(at: (f64, f64)) -> Rc<Self> {
        let number = DROPS.fetch_add(1, Ordering::Relaxed);
        let (decided, wake) = async_channel::unbounded();
        Rc::new(Self {
            drop: format!("d{}x{number}", std::process::id()),
            started: Cell::new(false),
            stop: Arc::default(),
            at: Cell::new(at),
            left: Cell::new(false),
            dropped_at: Cell::new(None),
            decided,
            wake,
        })
    }

    async fn decision(&self) {
        while !self.left.get() && self.dropped_at.get().is_none() {
            let _ = self.wake.recv().await;
        }
    }
}

pub(super) struct DropIn {
    window: Window,
    screen: Screen,
    link: Arc<Link>,
    target: gtk::DropTarget,
    current: RefCell<Option<Rc<Delivery>>>,
}

fn local_paths(value: &glib::Value) -> Vec<PathBuf> {
    let Ok(list) = value.get::<gdk::FileList>() else { return Vec::new() };
    list.files().iter().filter_map(gio::File::path).collect()
}

impl DropIn {
    pub(super) fn new(window: &Window, screen: &Screen, link: Arc<Link>) -> Rc<Self> {
        let target = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        // The files are wanted while the drag is still over the screen, not only at the drop.
        target.set_preload(true);
        let this = Rc::new(Self {
            window: window.clone(),
            screen: screen.clone(),
            link,
            target: target.clone(),
            current: RefCell::default(),
        });
        // Weak, as the screen's picture keeps the target, and this keeps the screen.
        let weak = Rc::downgrade(&this);
        target.connect_enter(move |_, x, y| {
            weak.upgrade().map_or(gdk::DragAction::empty(), |this| this.on_enter(x, y))
        });
        let weak = Rc::downgrade(&this);
        target.connect_motion(move |_, x, y| {
            weak.upgrade().map_or(gdk::DragAction::empty(), |this| this.on_motion(x, y))
        });
        let weak = Rc::downgrade(&this);
        target.connect_value_notify(move |_| {
            if let Some(this) = weak.upgrade() {
                this.on_value();
            }
        });
        let weak = Rc::downgrade(&this);
        target.connect_leave(move |_| {
            if let Some(this) = weak.upgrade() {
                this.on_leave();
            }
        });
        let weak = Rc::downgrade(&this);
        target
            .connect_drop(move |_, value, x, y| weak.upgrade().is_some_and(|this| this.on_drop(value, x, y)));
        screen.picture().add_controller(target);
        this
    }

    /// A captured mouse moves the guest's pointer by steps, so there's no saying where a drop
    /// would land. One drop at a time, too.
    fn takes_drags(&self) -> bool {
        !self.screen.captured() && self.screen.has_input()
    }

    fn on_enter(self: &Rc<Self>, x: f64, y: f64) -> gdk::DragAction {
        if !self.takes_drags() || self.current.borrow().is_some() {
            return gdk::DragAction::empty();
        }
        *self.current.borrow_mut() = Some(Delivery::new((x, y)));
        self.screen.on_motion(x, y);
        self.on_value();
        gdk::DragAction::COPY
    }

    fn on_motion(&self, x: f64, y: f64) -> gdk::DragAction {
        match self.waiting() {
            Some(delivery) => {
                delivery.at.set((x, y));
                self.screen.on_motion(x, y);
                gdk::DragAction::COPY
            }
            None => gdk::DragAction::empty(),
        }
    }

    /// The delivery of the drag that is over the screen now.
    fn waiting(&self) -> Option<Rc<Delivery>> {
        self.current.borrow().clone().filter(|delivery| delivery.dropped_at.get().is_none())
    }

    /// GTK has read what the drag carries.
    fn on_value(self: &Rc<Self>) {
        let Some(delivery) = self.waiting() else { return };
        let paths = self.target.value().map(|value| local_paths(&value)).unwrap_or_default();
        if !paths.is_empty() && !delivery.started.replace(true) {
            glib::spawn_future_local(self.clone().deliver(delivery, paths));
        }
    }

    fn on_leave(&self) {
        // GTK also says a drag has left once it was dropped.
        let Some(delivery) = self.waiting() else { return };
        delivery.left.set(true);
        delivery.stop.store(true, Ordering::Relaxed);
        let _ = delivery.decided.try_send(());
        if !delivery.started.get() {
            self.current.borrow_mut().take();
        }
    }

    fn on_drop(self: &Rc<Self>, value: &glib::Value, x: f64, y: f64) -> bool {
        let Some(delivery) = self.waiting() else { return false };
        let paths = local_paths(value);
        if !delivery.started.get() && paths.is_empty() {
            self.current.borrow_mut().take();
            self.window.toast(NOTHING_TO_SEND, adw::ToastPriority::Normal);
            return false;
        }
        delivery.at.set((x, y));
        delivery.dropped_at.set(Some((x, y)));
        let _ = delivery.decided.try_send(());
        if !delivery.started.replace(true) {
            glib::spawn_future_local(self.clone().deliver(delivery, paths));
        }
        true
    }

    async fn deliver(self: Rc<Self>, delivery: Rc<Delivery>, paths: Vec<PathBuf>) {
        let outcome = self.carry_out(&delivery, paths).await;
        self.screen.hold_pointer(false);
        self.current.borrow_mut().take();
        match outcome {
            Ok(Some(text)) => self.window.toast(text, adw::ToastPriority::Normal),
            Ok(None) => {}
            Err(text) => self.window.toast(&text, adw::ToastPriority::Normal),
        }
    }

    /// Forgets a drop whose drag hasn't started.
    fn forget(&self, delivery: &Delivery) {
        let (link, drop) = (self.link.clone(), delivery.drop.clone());
        gio::spawn_blocking(move || link.helper.cancel(&drop));
    }

    /// What to tell the user, if anything.
    async fn carry_out(
        &self,
        delivery: &Delivery,
        paths: Vec<PathBuf>,
    ) -> Result<Option<&'static str>, String> {
        let (link, drop, stop) = (self.link.clone(), delivery.drop.clone(), delivery.stop.clone());
        let sent = off_thread(move || {
            let files = dropped_files(&paths);
            if files.is_empty() {
                return Err(NOTHING_TO_SEND.to_owned());
            }
            if !link.ensure() {
                return Err(NOT_READY.to_owned());
            }
            match send(&link.helper, &drop, &files, &stop) {
                Err(error) if error.kind() != ErrorKind::Interrupted => {
                    Err(format!("The files didn't reach Windows: {error}"))
                }
                _ => Ok(()),
            }
        });
        let sent = sent.await.unwrap_or_else(|| Err(NO_ANSWER.to_owned()));
        self.link.log(&format!("drop {} sent: {sent:?}", delivery.drop));
        if delivery.left.get() || sent.is_err() {
            self.forget(delivery);
            return sent.map(|()| None);
        }
        let dragging = self.start_guest_drag(delivery).await;
        self.link.log(&format!("drop {} dragging in the guest: {dragging}", delivery.drop));
        if delivery.left.get() && !dragging {
            self.forget(delivery);
            return Ok(None);
        }
        if dragging {
            self.screen.hold_pointer(false);
            self.stir(delivery);
        }
        delivery.decision().await;
        if !dragging {
            return self.keep_on_desktop(delivery).await;
        }
        let Some((x, y)) = delivery.dropped_at.get() else {
            // The drag left this window, so the guest's ends too, and the helper forgets the files.
            // Windows asks the helper whether to go on when the pointer moves. The button stays
            // down until it has, or this would be a drop.
            let (link, drop) = (self.link.clone(), delivery.drop.clone());
            let _ = off_thread(move || link.helper.cancel(&drop)).await;
            self.stir(delivery);
            let link = self.link.clone();
            let ended = off_thread(move || link.helper.wait(Until::Ended, CANCEL_PATIENCE)).await;
            self.link.log(&format!("drop {} left the screen: {ended:?}", delivery.drop));
            self.screen.press(LEFT, false);
            return Ok(None);
        };
        delivery.at.set((x, y));
        self.stir(delivery);
        // Windows drops on what the drag was last over, so it has to see the pointer arrive
        // before it sees the button come up.
        glib::timeout_future(BEFORE_RELEASE).await;
        self.screen.press(LEFT, false);
        let link = self.link.clone();
        let ended = off_thread(move || link.helper.wait(Until::Ended, END_PATIENCE)).await;
        self.link.log(&format!("drop {} ended: {ended:?}", delivery.drop));
        match ended {
            Some(Ok(DragState::OnDesktop)) => Ok(Some(ON_DESKTOP)),
            // Still dragging means what took the drop is asking the user something.
            Some(Ok(DragState::Taken | DragState::Cancelled | DragState::Dragging)) => Ok(None),
            _ => Err(NO_ANSWER.to_owned()),
        }
    }

    /// Windows only looks at what is under a drag when the pointer moves, so this moves it a
    /// pixel aside and back to where this PC's drag is.
    fn stir(&self, delivery: &Delivery) {
        let (x, y) = delivery.at.get();
        self.screen.on_motion(x - 1.0, y);
        self.screen.on_motion(x, y);
    }

    /// Says whether the guest is dragging the files now. The helper's window goes where the
    /// guest's pointer is, so the pointer stays put until the press has landed on it.
    async fn start_guest_drag(&self, delivery: &Delivery) -> bool {
        self.screen.hold_pointer(true);
        let (link, drop) = (self.link.clone(), delivery.drop.clone());
        let shown = off_thread(move || link.helper.drag(&drop)).await;
        self.link.log(&format!("drop {} offered under the pointer: {shown:?}", delivery.drop));
        if delivery.left.get() || !matches!(shown, Some(Ok(true))) {
            return false;
        }
        self.screen.press(LEFT, true);
        let link = self.link.clone();
        let state = off_thread(move || link.helper.wait(Until::Started, START_PATIENCE)).await;
        let dragging = matches!(state, Some(Ok(DragState::Dragging)));
        if !dragging {
            self.screen.press(LEFT, false);
        }
        dragging
    }

    /// For a guest that can't be given a drag just now, like one at its lock screen: the files
    /// are there, so they go where the user will find them.
    async fn keep_on_desktop(&self, delivery: &Delivery) -> Result<Option<&'static str>, String> {
        if delivery.left.get() {
            self.forget(delivery);
            return Ok(None);
        }
        let (link, drop) = (self.link.clone(), delivery.drop.clone());
        match off_thread(move || link.helper.keep(&drop)).await {
            Some(Ok(())) => Ok(Some(ON_DESKTOP)),
            _ => Err(NO_ANSWER.to_owned()),
        }
    }
}

impl Drop for DropIn {
    fn drop(&mut self) {
        self.screen.picture().remove_controller(&self.target);
    }
}
