//! Files dragged out of the guest's screen.
//!
//! When the pointer leaves the screen with the button held, the helper in the guest is asked what
//! is being dragged there. If it is files, the guest's drag is ended and one of this PC's begins in
//! its place, which any program here can take. The files are fetched while it is carried, and a
//! program that takes the drop before they are all here waits for the rest.

use super::super::screen::Screen;
use super::dragged_files::DraggedFiles;
use super::{ESCAPE, Link, off_thread};
use crate::window::Window;
use adw::prelude::*;
use gtk::{gdk, glib};
use guesthouse_app_core::guest_files::{Entry, fetch};
use std::cell::{Cell, RefCell};
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

const LEFT: &str = "left";
/// How long the helper looks for a drag. Windows tells its window of one within a few frames.
const CATCH_TIME: Duration = Duration::from_millis(150);
const LABEL_ASIDE: i32 = 16;
const BEFORE_RELEASE: Duration = Duration::from_millis(150);
/// Under the cache folder: one folder a machine, and in it one for each drag.
const DRAGGED_FOLDER: &str = "guesthouse/dragged";

static DRAGS: AtomicU32 = AtomicU32::new(0);

pub(super) struct DragOut {
    window: Window,
    screen: Screen,
    link: Arc<Link>,
    motion: gtk::EventControllerMotion,
    /// The guest was asked about this press already.
    asked: Cell<bool>,
    /// This PC's drag, kept until it ends.
    carried: RefCell<Option<gdk::Drag>>,
}

fn title(entries: &[Entry]) -> String {
    let dragged: Vec<&Entry> = entries.iter().filter(|entry| entry.is_dragged_itself()).collect();
    match dragged.as_slice() {
        [one] => one.name.clone(),
        many => format!("{} files", many.len()),
    }
}

impl DragOut {
    pub(super) fn new(window: &Window, screen: &Screen, link: Arc<Link>) -> Rc<Self> {
        let motion = gtk::EventControllerMotion::new();
        let this = Rc::new(Self {
            window: window.clone(),
            screen: screen.clone(),
            link,
            motion: motion.clone(),
            asked: Cell::new(false),
            carried: RefCell::default(),
        });
        this.remove_old_drags();
        // Weak, as the screen's picture keeps the controller, and this keeps the screen.
        let weak = Rc::downgrade(&this);
        motion.connect_motion(move |motion, x, y| {
            if let Some(this) = weak.upgrade() {
                this.on_motion(motion, x, y);
            }
        });
        screen.picture().add_controller(motion);
        this
    }

    fn folder(&self) -> PathBuf {
        glib::user_cache_dir().join(DRAGGED_FOLDER).join(&self.link.machine.id)
    }

    /// What earlier drags left. A program may have been given a file where it lies, so they are
    /// kept until the screen is next opened.
    fn remove_old_drags(&self) {
        let _ = fs::remove_dir_all(self.folder());
    }

    /// A press keeps the pointer, so its motion comes here from outside the picture too.
    fn on_motion(self: &Rc<Self>, motion: &gtk::EventControllerMotion, x: f64, y: f64) {
        if !self.screen.holds_button(LEFT) {
            self.asked.set(false);
            return;
        }
        let picture = self.screen.picture();
        let inside =
            (0.0..f64::from(picture.width())).contains(&x) && (0.0..f64::from(picture.height())).contains(&y);
        if inside || self.screen.captured() || !self.link.is_ready() || self.asked.replace(true) {
            return;
        }
        if let Some(device) = motion.current_event_device() {
            glib::spawn_future_local(self.clone().ask_guest(device));
        }
    }

    async fn ask_guest(self: Rc<Self>, device: gdk::Device) {
        // The helper's window is under the guest's pointer, which has to stay there meanwhile.
        self.screen.hold_pointer(true);
        let link = self.link.clone();
        let caught = off_thread(move || link.helper.catch(CATCH_TIME)).await;
        self.screen.hold_pointer(false);
        self.link.log(&format!(
            "a drag left the screen, the guest's carries: {:?}",
            caught.as_ref().map(|caught| caught.as_ref().map(|files| files.as_ref().map(Vec::len)))
        ));
        match caught {
            Some(Ok(Some(entries))) if !entries.is_empty() && self.screen.holds_button(LEFT) => {
                self.carry(entries, &device);
            }
            Some(Ok(_)) => {}
            _ => self.link.start_again(),
        }
    }

    fn carry(self: &Rc<Self>, entries: Vec<Entry>, device: &gdk::Device) {
        let Some(surface) = self.screen.picture().native().and_then(|native| native.surface()) else {
            return;
        };
        let (content, landing) = DraggedFiles::new();
        let Some(drag) = gdk::Drag::begin(&surface, device, &content, gdk::DragAction::COPY, 0.0, 0.0) else {
            return;
        };
        // This PC carries the files now, so Windows lets go of them. The button stays down until
        // Windows has seen the key, or the files would be dropped where the pointer left.
        self.screen.tap_key(ESCAPE);
        let screen = self.screen.clone();
        glib::timeout_add_local_once(BEFORE_RELEASE, move || screen.press(LEFT, false));
        let label = gtk::Label::new(Some(&title(&entries)));
        label.add_css_class("osd");
        // Beside the pointer, not under it.
        label.set_margin_start(LABEL_ASIDE);
        label.set_margin_top(LABEL_ASIDE);
        gtk::DragIcon::for_drag(&drag).set_child(Some(&label));
        let stop = Arc::new(AtomicBool::new(false));
        let folder = self.folder().join(DRAGS.fetch_add(1, Ordering::Relaxed).to_string());
        // Dropped on nothing, so nothing wants the files, whether they are all here or not.
        let (stopping, unwanted) = (stop.clone(), folder.clone());
        drag.connect_cancel(move |_, _| {
            stopping.store(true, Ordering::Relaxed);
            let _ = fs::remove_dir_all(&unwanted);
        });
        *self.carried.borrow_mut() = Some(drag);
        let this = self.clone();
        glib::spawn_future_local(async move {
            let (link, into, stopped) = (this.link.clone(), folder.clone(), stop.clone());
            let fetched = off_thread(move || fetch(&link.helper, &entries, &into, &stopped)).await;
            let fetched = fetched.unwrap_or_else(|| Err(std::io::ErrorKind::Other.into()));
            this.link.log(&format!("dragged out: {fetched:?}"));
            if stop.load(Ordering::Relaxed) {
                // What was written after the drag ended.
                let _ = fs::remove_dir_all(&folder);
            } else if let Err(error) = &fetched {
                this.window.toast(
                    &format!("The files didn't come out of Windows: {error}"),
                    adw::ToastPriority::Normal,
                );
            }
            landing.finish(fetched.map_err(|error| error.to_string()));
            this.carried.borrow_mut().take();
        });
    }
}

impl Drop for DragOut {
    fn drop(&mut self) {
        self.screen.picture().remove_controller(&self.motion);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, is_folder: bool) -> Entry {
        Entry { name: name.to_owned(), is_folder, size: 0 }
    }

    #[test]
    fn one_dragged_thing_goes_by_its_name_and_more_by_their_number() {
        let folder = [entry("trip", true), entry("trip/a.txt", false), entry("trip/b.txt", false)];
        assert_eq!(title(&folder), "trip");
        let two = [entry("a.txt", false), entry("b.txt", false)];
        assert_eq!(title(&two), "2 files");
    }
}
