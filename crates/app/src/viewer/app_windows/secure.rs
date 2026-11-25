//! The guest's secure desktop (UAC) and its lock screen. The helper cannot see or draw there, so while
//! the guest's input desktop is not the normal one the host shows one window with the guest's whole
//! frame (or, when the guest says where the prompt is, a rectangle of it), takes the pointer and the
//! keys for the guest 1:1, and gives the focus back to the app window that had it when the normal
//! desktop returns. The picture is one more cut of the hub's frame, drawn by the same widget as a
//! window cell's, so it follows whatever that widget does with frames.

use super::cell_picture::CellPicture;
use super::framed::Framed;
use super::hub::Hub;
use super::plan::point_for;
use crate::viewer::input::button_name;
use crate::viewer::slot_hold::HeldFrame;
use adw::prelude::*;
use gtk::{gdk, glib};
use guesthouse_app_core::app_windows::{Caption, Cell, Desktop, Point, Rect, Size, State, Surface, crop};
use std::cell::RefCell;
use std::rc::{Rc, Weak};

/// What the window says. The prompt is the guest's, so the words only say that it waits.
pub fn title(desktop: Desktop) -> &'static str {
    match desktop {
        Desktop::Locked => "Windows is locked",
        _ => "Windows needs your answer",
    }
}

/// The part of the guest's screen the window shows: the rectangle the guest named if it lies on the
/// screen, else the whole screen.
pub fn region(wanted: Option<Rect>, screen: Size) -> Rect {
    let whole = Rect { x: 0, y: 0, w: screen.w, h: screen.h };
    match wanted {
        Some(rect) if crop(rect, screen).is_some_and(|shown| shown.source == rect) => rect,
        _ => whole,
    }
}

/// The cell the window stands for. Its id is no window's of the guest, so nothing asks the helper
/// about it.
fn cell_of(region: Rect) -> Cell {
    Cell {
        id: "secure-desktop".to_owned(),
        pid: 0,
        exe: String::new(),
        cls: String::new(),
        title: String::new(),
        cell: region,
        frame: region,
        popup: false,
        parent: String::new(),
        rel: Point::default(),
        state: State::Shown,
        overflow: false,
        focus: true,
        caption: Caption::default(),
        resizable: false,
        app: String::new(),
        name: String::new(),
        corner: 0,
    }
}

pub struct SecureWindow {
    window: gtk::Window,
    frame: CellPicture,
    /// The picture, on a surface of the window's own.
    framed: Framed,
    cell: RefCell<Cell>,
    wanted: Option<Rect>,
    /// The app window that had the focus when this one came, to be given it back.
    pub came_from: Option<String>,
}

impl SecureWindow {
    pub fn open(
        hub: &Rc<Hub>,
        desktop: Desktop,
        wanted: Option<Rect>,
        came_from: Option<String>,
    ) -> Rc<Self> {
        let frame = CellPicture::default();
        frame.set_focusable(false);
        frame.set_guest(hub.guest());
        let window = gtk::Window::builder()
            .application(hub.application())
            .title(title(desktop))
            .resizable(false)
            .child(&frame)
            .css_classes(["app-window-view"])
            .build();
        let framed = Framed::new("the secure desktop's window", &frame);
        let this = Rc::new(Self {
            window,
            frame,
            framed,
            cell: RefCell::new(cell_of(region(wanted, hub.screen_size()))),
            wanted,
            came_from,
        });
        this.recrop(hub.screen_size());
        this.add_input(hub);
        this.window.present();
        this
    }

    /// A new frame, its rectangle shown with no copy, as an app window's is.
    pub fn show(&self, hub: &Hub, frame: &HeldFrame, sdr_white_nits: Option<u32>) {
        self.framed.show(hub, frame, sdr_white_nits, self.window.is_suspended());
    }

    /// Takes in what the compositor gave back, and shows `newest` while this is behind.
    pub fn tick(&self, hub: &Hub, newest: Option<&HeldFrame>, sdr_white_nits: Option<u32>) {
        self.framed.tick(hub, newest, sdr_white_nits, self.window.is_suspended());
    }

    pub fn set_cursor(&self, cursor: Option<&gdk::Cursor>) {
        self.frame.set_cursor(cursor);
    }

    pub fn set_guest(&self, guest: super::cell_picture::Guest) {
        self.frame.set_guest(guest);
    }

    pub fn recrop(&self, screen: Size) {
        let region = region(self.wanted, screen);
        *self.cell.borrow_mut() = cell_of(region);
        let Some(shown) = crop(region, screen) else { return };
        self.frame.set_cell((region.w, region.h), shown.source, shown.offset);
    }

    pub fn close(&self) {
        self.framed.end();
        self.window.destroy();
    }

    fn point(&self, hub: &Hub, x: f64, y: f64) -> Point {
        let cell = self.cell.borrow();
        let surface = Surface::shown(&cell, self.frame.ratio());
        point_for(&cell, surface, Point::default(), x, y, hub.screen_size())
    }

    fn add_input(self: &Rc<Self>, hub: &Rc<Hub>) {
        let weak: (Weak<Self>, Weak<Hub>) = (Rc::downgrade(self), Rc::downgrade(hub));
        let pair = |weak: &(Weak<Self>, Weak<Hub>)| Some((weak.0.upgrade()?, weak.1.upgrade()?));
        let motion = gtk::EventControllerMotion::new();
        let this = weak.clone();
        motion.connect_motion(move |_, x, y| {
            if let Some((view, hub)) = pair(&this) {
                hub.secure_pointer(view.point(&hub, x, y));
            }
        });
        let this = weak.clone();
        motion.connect_enter(move |_, x, y| {
            if let Some((view, hub)) = pair(&this) {
                hub.secure_pointer(view.point(&hub, x, y));
            }
        });
        self.frame.add_controller(motion);
        let click = gtk::GestureClick::builder().button(0).build();
        for down in [true, false] {
            let this = weak.clone();
            let handler = move |gesture: &gtk::GestureClick, _: i32, x: f64, y: f64| {
                let Some((view, hub)) = pair(&this) else { return };
                if let Some(name) = button_name(gesture.current_button()) {
                    hub.secure_pointer(view.point(&hub, x, y));
                    hub.secure_button(name, down);
                }
            };
            if down {
                click.connect_pressed(handler);
            } else {
                click.connect_released(handler);
            }
        }
        self.frame.add_controller(click);
        self.frame.add_controller(hub.scroll_controller());
        let keys = gtk::EventControllerKey::new();
        let this = weak.clone();
        keys.connect_key_pressed(move |_, _, code, _| {
            if let Some((_, hub)) = pair(&this) {
                hub.secure_key(code, true);
            }
            glib::Propagation::Stop
        });
        let this = weak;
        keys.connect_key_released(move |_, _, code, _| {
            if let Some((_, hub)) = pair(&this) {
                hub.secure_key(code, false);
            }
        });
        self.window.add_controller(keys);
        // A key or button held down as the window loses the keyboard would stay down in the guest.
        let this = Rc::downgrade(hub);
        self.window.connect_is_active_notify(move |window| {
            if !window.is_active()
                && let Some(hub) = this.upgrade()
            {
                hub.release_held();
            }
        });
        // The prompt is answered in the guest, so the host's close does nothing.
        self.window.connect_close_request(|_| glib::Propagation::Stop);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_whole_screen_unless_the_guest_names_a_rectangle_on_it() {
        let screen = Size { w: 1920, h: 1080 };
        let whole = Rect { x: 0, y: 0, w: 1920, h: 1080 };
        assert_eq!(region(None, screen), whole);
        let prompt = Rect { x: 700, y: 300, w: 520, h: 400 };
        assert_eq!(region(Some(prompt), screen), prompt);
        assert_eq!(
            region(Some(Rect { x: 1800, y: 300, w: 520, h: 400 }), screen),
            whole,
            "hangs over the edge"
        );
        assert_eq!(region(Some(Rect::default()), screen), whole, "empty");
    }

    #[test]
    fn the_title_says_which_desktop_waits() {
        assert_eq!(title(Desktop::Secure), "Windows needs your answer");
        assert_eq!(title(Desktop::Locked), "Windows is locked");
    }

    #[test]
    fn the_pointer_is_one_to_one_over_the_whole_screen() {
        let screen = Size { w: 1920, h: 1080 };
        let cell = cell_of(region(None, screen));
        let at = point_for(&cell, Surface::shown(&cell, 1.0), Point::default(), 100.5, 200.5, screen);
        assert_eq!(at, Point { x: 100, y: 200 });
        let at = point_for(&cell, Surface::shown(&cell, 2.0), Point::default(), 100.5, 200.5, screen);
        assert_eq!(at, Point { x: 201, y: 401 });
        let corner = point_for(&cell, Surface::shown(&cell, 1.0), Point::default(), 5000.0, 5000.0, screen);
        assert_eq!(corner, Point { x: 1919, y: 1079 });
    }
}
