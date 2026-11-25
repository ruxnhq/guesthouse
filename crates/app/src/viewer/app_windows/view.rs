//! One host window or popup of a guest cell: a part of the guest's frame in a widget of its own, and
//! the pointer, keys and window events that go back to the guest. The frames and the input themselves
//! are the hub's, shared by every view.

use super::cell_picture::{CellPicture, Guest};
use super::framed::Framed;
use super::hub::Hub;
use super::plan::{HostChange, Order, Presence, follow_guest_state, follow_host_state, point_for};
use crate::viewer::input::button_name;
use crate::viewer::slot_hold::HeldFrame;
use crate::viewer::title_drag::{PRIMARY_BUTTON, is_move};
use adw::prelude::*;
use gtk::{gdk, glib, graphene};
use guesthouse_app_core::app_windows::{
    Cell, Fields, Point, Request, Size, State, Surface, crop, on_drag_strip, popup_place,
};
use std::cell::{Cell as Slot, RefCell};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

/// How long the window's size must stay before the guest is asked for it: a drag or a layout gives many.
const SIZE_STILL: Duration = Duration::from_millis(250);
/// How long after the compositor last gave the window a size of its own choosing the window is left at
/// it, rather than made the cell's size again: a drag of the window's edge goes on for a while.
const HOST_SIZE_HOLD: Duration = Duration::from_millis(800);
/// How long the guest has to answer a size before the window goes back to its cell's.
const GUEST_ANSWER: Duration = Duration::from_millis(3000);

/// A size asked of the guest, the cell's size then, and when.
type Asked = ((i32, i32), (i32, i32), Instant);

const BACK: u32 = 8;
const FORWARD: u32 = 9;
/// GTK draws nothing in a window or popup: the picture is on the view's own surface over it, and
/// what the window has past the picture (under a logical unit at the right and bottom) is clear.
const STYLE: &str = "window.app-window-view { background: #000000; }
popover.app-window-popup, popover.app-window-popup > contents {
  padding: 0; margin: 0; border: none; border-radius: 0; box-shadow: none; background: #000000; min-width: 0; min-height: 0; }
popover.app-window-popup > arrow { min-width: 0; min-height: 0; }";

/// A press on the title bar that the guest has not been told of: let go where it was, it is a click.
struct HeldPress {
    at: (f64, f64),
    device: gdk::Device,
    time: u32,
    gesture: gtk::GestureClick,
}

enum Host {
    Window(gtk::Window),
    Popup(gtk::Popover),
}

pub struct View {
    pub id: String,
    host: Host,
    frame: CellPicture,
    cell: RefCell<Cell>,
    presence: Slot<Presence>,
    held: RefCell<Option<HeldPress>>,
    moved_window: Slot<bool>,
    /// The popups hosted against this one, as ids.
    pub children: RefCell<Vec<String>>,
    /// The picture, on a surface of the view's own.
    framed: Framed,
    /// The window's size the compositor last chose (not the cell's), and when, which the guest is
    /// asked for once it has stayed `SIZE_STILL`.
    host_size: Slot<Option<((i32, i32), Instant)>>,
    /// The guest size last asked for, the cell's size then, and when: a size the guest would not
    /// take is not asked for again while the cell stays, and the window waits for the answer.
    asked: Slot<Option<Asked>>,
    /// The sizes the cell lately had at the ratio of the moment, newest last. A window given one of
    /// them was given what it asked for when it asked (its size when it opened, before the compositor
    /// said its scale), not a size the compositor chose.
    wanted_lately: RefCell<Vec<(i32, i32)>>,
}

pub fn add_style(display: &gdk::Display) {
    let style = gtk::CssProvider::new();
    style.load_from_string(STYLE);
    gtk::style_context_add_provider_for_display(display, &style, gtk::STYLE_PROVIDER_PRIORITY_USER + 1);
}

impl View {
    /// A toplevel for `cell`, or a popup against `parent`. Nothing is shown until a frame is given.
    pub fn open(hub: &Rc<Hub>, cell: &Cell, parent: Option<&Rc<View>>) -> Rc<View> {
        let frame = CellPicture::default();
        frame.set_focusable(false);
        frame.set_guest(hub.guest());
        let host = match parent {
            None => Host::Window(
                gtk::Window::builder()
                    .application(hub.application())
                    .title(&cell.title)
                    .decorated(false)
                    .resizable(cell.resizable)
                    .child(&frame)
                    .css_classes(["app-window-view"])
                    .build(),
            ),
            Some(_) => Host::Popup(
                gtk::Popover::builder()
                    .autohide(false)
                    .has_arrow(false)
                    .focusable(false)
                    .can_focus(false)
                    .child(&frame)
                    .css_classes(["app-window-popup"])
                    .build(),
            ),
        };
        let framed = Framed::new(&format!("view {}", cell.id), &frame);
        let view = Rc::new(View {
            id: cell.id.clone(),
            host,
            frame,
            cell: RefCell::new(cell.clone()),
            presence: Slot::new(Presence::Up),
            held: RefCell::default(),
            moved_window: Slot::new(false),
            children: RefCell::default(),
            framed,
            host_size: Slot::new(None),
            asked: Slot::new(None),
            wanted_lately: RefCell::default(),
        });
        view.recrop(hub.screen_size());
        view.add_input(hub);
        view.watch_scale(hub);
        view.watch_size();
        match &view.host {
            Host::Window(window) => {
                view.watch_window(hub, window);
                if let Some(id) = super::identity::app_id_for(cell) {
                    super::identity::set_when_mapped(window, &id);
                }
            }
            Host::Popup(popover) => {
                if let Some(parent) = parent {
                    popover.set_parent(&parent.frame);
                    parent.children.borrow_mut().push(view.id.clone());
                    view.place_popup(parent);
                }
            }
        }
        match &view.host {
            Host::Window(window) => {
                window.present();
                // A window that is minimized in the guest from its first layout opens that way.
                if cell.state == State::Minimized {
                    view.presence.set(Presence::Minimizing);
                    window.minimize();
                }
            }
            Host::Popup(popover) => match parent {
                // A menu that is open when the viewer starts comes in the same layout as its window,
                // which is not on the screen yet: its place against the window is not known until it
                // is, so it is shown then (measured: it landed at 0,50 instead).
                Some(parent) if !parent.frame.is_mapped() => {
                    let (popover, weak) = (popover.clone(), Rc::downgrade(&view));
                    let weak_parent = Rc::downgrade(parent);
                    parent.frame.connect_map(move |_| {
                        if let (Some(view), Some(parent)) = (weak.upgrade(), weak_parent.upgrade()) {
                            view.place_popup(&parent);
                            popover.popup();
                        }
                    });
                }
                _ => popover.popup(),
            },
        }
        view
    }

    /// Whether the picture takes the pointer. Not while the guest's input desktop is not the normal one.
    pub fn take_pointer(&self, on: bool) {
        self.frame.set_can_target(on);
    }

    /// Gives the host window the focus.
    pub fn bring_forward(&self) {
        if let Host::Window(window) = &self.host {
            window.present();
        }
    }

    pub fn is_popup(&self) -> bool {
        matches!(self.host, Host::Popup(_))
    }

    /// A new frame, which every view shows: its buffer goes on the view's own surface, cut to the
    /// cell. A window out of sight shows none and holds no slot.
    pub fn show(&self, hub: &Hub, frame: &HeldFrame, sdr_white_nits: Option<u32>) {
        self.framed.show(hub, frame, sdr_white_nits, self.out_of_sight());
    }

    /// Takes in what the compositor gave back, and shows `newest` if this view is behind.
    pub fn tick(&self, hub: &Hub, newest: Option<&HeldFrame>, sdr_white_nits: Option<u32>) {
        self.framed.tick(hub, newest, sdr_white_nits, self.out_of_sight());
    }

    fn out_of_sight(&self) -> bool {
        matches!(&self.host, Host::Window(window) if window.is_suspended())
    }

    /// The window's size against its cell's. A size the compositor chose (a tiling layout, a maximize,
    /// the user's drag of an edge) is asked of the guest once it has stayed; the picture meanwhile is
    /// cut or in the middle, never stretched. A cell that changed size (the guest's own resize, or its
    /// answer) makes the window that size when the window is its own to size: not maximized, full
    /// screen or tiled, and not just sized by the compositor.
    pub fn keep_size(&self, hub: &Hub) {
        let Host::Window(window) = &self.host else { return };
        let Some(allocated) = self.frame.allocated() else { return };
        let wanted = self.frame.wanted_size();
        {
            let mut lately = self.wanted_lately.borrow_mut();
            if lately.last() != Some(&wanted) {
                lately.push(wanted);
                let extra = lately.len().saturating_sub(4);
                lately.drain(..extra);
            }
        }
        if let Some((size, since)) = self.host_size.get() {
            if size != allocated || allocated == wanted {
                self.host_size.set((allocated != wanted).then(|| (allocated, Instant::now())));
                return;
            }
            if since.elapsed() >= SIZE_STILL {
                self.ask_size(hub, allocated);
            }
            let cell = {
                let cell = self.cell.borrow();
                (cell.cell.w, cell.cell.h)
            };
            let answering =
                self.asked.get().is_some_and(|(_, then, at)| then == cell && at.elapsed() < GUEST_ANSWER);
            if since.elapsed() < HOST_SIZE_HOLD || answering || !self.free(window) {
                return;
            }
            // The compositor's size has stood long enough: the cell, which follows it as far as the
            // guest would go, is the window's size from here on.
            self.host_size.set(None);
        }
        if allocated != wanted && self.free(window) {
            hub.trace(&format!(
                "view {}: the window is made its cell's size {wanted:?}, from {allocated:?}",
                self.id
            ));
            window.set_default_size(wanted.0, wanted.1);
        }
    }

    /// Notes a size the widget was given that is not its cell's.
    fn watch_size(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.frame.connect_size(move |width, height| {
            let Some(view) = weak.upgrade() else { return };
            let ours = (width, height) == view.frame.wanted_size()
                || view.wanted_lately.borrow().contains(&(width, height));
            if !ours {
                view.host_size.set(Some(((width, height), Instant::now())));
            }
        });
    }

    /// Asks the guest for the window's size, in its pixels at the ratio now, once for each size.
    fn ask_size(&self, hub: &Hub, (width, height): (i32, i32)) {
        let cell = self.cell.borrow();
        if !cell.resizable {
            return;
        }
        let ratio = self.frame.ratio();
        let size = ((f64::from(width) * ratio).round() as i32, (f64::from(height) * ratio).round() as i32);
        let now = (cell.cell.w, cell.cell.h);
        if size == now || self.asked.get().is_some_and(|(asked, then, _)| asked == size && then == now) {
            return;
        }
        self.asked.set(Some((size, now, Instant::now())));
        hub.note(&format!(
            "view {}: the window is {width}x{height}, so the guest's is asked to be {}x{}",
            self.id, size.0, size.1
        ));
        hub.request(Request::Resize { id: self.id.clone(), w: size.0, h: size.1 });
    }

    /// Whether the window's size is its own to choose: not maximized, full screen or tiled.
    fn free(&self, window: &gtk::Window) -> bool {
        let Some(toplevel) = window.surface().and_then(|surface| surface.downcast::<gdk::Toplevel>().ok())
        else {
            return false;
        };
        let held = gdk::ToplevelState::MAXIMIZED
            | gdk::ToplevelState::FULLSCREEN
            | gdk::ToplevelState::TILED
            | gdk::ToplevelState::TOP_TILED
            | gdk::ToplevelState::BOTTOM_TILED
            | gdk::ToplevelState::LEFT_TILED
            | gdk::ToplevelState::RIGHT_TILED;
        !toplevel.state().intersects(held)
    }

    /// Windows' scale or the steps it offers changed.
    pub fn set_guest(&self, guest: Guest) {
        self.frame.set_guest(guest);
    }

    /// The scale of the surface the window is on, once it is on one the compositor mapped.
    pub fn known_scale(&self) -> Option<f64> {
        let surface = self.frame.native()?.surface()?;
        surface.is_mapped().then(|| surface.scale())
    }

    /// The monitor the host window is on, as the compositor says.
    pub fn monitor(&self) -> Option<gdk::Monitor> {
        let surface = self.frame.native()?.surface()?;
        gdk::Display::default()?.monitor_at_surface(&surface)
    }

    /// Whether the host window has the focus.
    pub fn is_active(&self) -> bool {
        matches!(&self.host, Host::Window(window) if window.is_active())
    }

    /// A popup follows its parent's ratio when the parent's surface changes scale.
    fn watch_scale(self: &Rc<Self>, hub: &Rc<Hub>) {
        let weak = (Rc::downgrade(self), Rc::downgrade(hub));
        self.frame.connect_scale(move |scale| {
            let Some((view, hub)) = both(&weak) else { return };
            hub.trace(&format!("view {} is on a surface at scale {scale}", view.id));
            hub.scale_changed(&view.id);
        });
    }

    /// Places the popups again, as the ratio they go by may have changed.
    pub fn replace_popup(&self, parent: &Rc<View>) {
        self.frame.check_ratio();
        self.place_popup(parent);
    }

    pub fn set_cursor(&self, cursor: Option<&gdk::Cursor>) {
        self.frame.set_cursor(cursor);
    }

    /// What of the cell is on the guest's screen, as a rectangle of the frame. A cell with none keeps
    /// the picture it had: a minimized window is out of sight and has no size to take.
    pub fn recrop(&self, screen: Size) {
        let cell = self.cell.borrow();
        let Some(shown) = crop(cell.cell, screen) else { return };
        self.frame.set_cell((cell.cell.w, cell.cell.h), shown.source, shown.offset);
    }

    /// The helper changed something of the cell. `now` is how it stands.
    pub fn update(self: &Rc<Self>, hub: &Rc<Hub>, now: &Cell, what: Fields, parent: Option<&Rc<View>>) {
        *self.cell.borrow_mut() = now.clone();
        if what.cell {
            self.recrop(hub.screen_size());
        }
        if let Host::Window(window) = &self.host {
            if what.title {
                window.set_title(Some(&now.title));
            }
            if what.other
                && let Some(id) = super::identity::app_id_for(now)
            {
                super::identity::set_now(window, &id);
            }
            if what.state {
                let (presence, order) = follow_guest_state(self.presence.get(), now.state);
                self.presence.set(presence);
                match order {
                    Order::Minimize => window.minimize(),
                    Order::Present => window.present(),
                    Order::Nothing => {}
                }
            }
        }
        if (what.cell || what.popup_link)
            && let Some(parent) = parent
        {
            self.place_popup(parent);
        }
    }

    /// Where a popup goes against its parent's host surface.
    fn place_popup(&self, parent: &Rc<View>) {
        let Host::Popup(popover) = &self.host else { return };
        let parent_cell = parent.cell.borrow();
        let place = popup_place(&self.cell.borrow(), &parent_cell, parent.surface());
        // The popup's top left corner is at `place`: its top edge is the bottom of an empty rectangle
        // there, and its width is the rectangle's, so the centres agree.
        popover.set_position(gtk::PositionType::Bottom);
        // Measured in a nested KWin: GTK puts the surface one pixel below the rectangle's edge.
        popover.set_offset(0, -1);
        popover.set_pointing_to(Some(&gdk::Rectangle::new(place.x, place.y, place.w.max(1), 0)));
    }

    /// The cell as it is shown, in the units a pointer comes in.
    fn surface(&self) -> Surface {
        Surface::shown(&self.cell.borrow(), self.frame.ratio())
    }

    /// Takes the host window away. A popup goes with its parent, so a parent's popups are the
    /// caller's to take first.
    pub fn destroy(&self) {
        self.framed.end();
        match &self.host {
            Host::Window(window) => window.destroy(),
            Host::Popup(popover) => {
                popover.popdown();
                popover.unparent();
            }
        }
    }

    /// The guest point for a point of this surface, and where it was sent.
    fn guest_point(&self, hub: &Hub, x: f64, y: f64) -> Point {
        let screen = hub.screen_size();
        // The widget holds the whole cell, the part off the screen black, so no offset is added.
        let (shift_x, shift_y) = self.frame.picture_shift();
        point_for(
            &self.cell.borrow(),
            self.surface(),
            Point::default(),
            x - f64::from(shift_x),
            y - f64::from(shift_y),
            screen,
        )
    }

    fn send_pointer(&self, hub: &Hub, x: f64, y: f64) -> Point {
        let point = self.guest_point(hub, x, y);
        hub.send_pointer(point);
        point
    }

    fn add_input(self: &Rc<Self>, hub: &Rc<Hub>) {
        let weak = (Rc::downgrade(self), Rc::downgrade(hub));
        let motion = gtk::EventControllerMotion::new();
        let this = weak.clone();
        motion.connect_motion(move |_, x, y| {
            if let Some((view, hub)) = both(&this) {
                view.on_motion(&hub, x, y);
            }
        });
        // A pointer that comes in is at a point too, and may not move again before it clicks.
        let this = weak.clone();
        motion.connect_enter(move |_, x, y| {
            if let Some((view, hub)) = both(&this) {
                view.on_motion(&hub, x, y);
            }
        });
        self.frame.add_controller(motion);
        let click = gtk::GestureClick::builder().button(0).build();
        let this = weak.clone();
        click.connect_pressed(move |gesture, _, x, y| {
            if let Some((view, hub)) = both(&this) {
                view.on_button(&hub, gesture, x, y, true);
            }
        });
        let this = weak.clone();
        click.connect_released(move |gesture, _, x, y| {
            if let Some((view, hub)) = both(&this) {
                view.on_button(&hub, gesture, x, y, false);
            }
        });
        self.frame.add_controller(click);
        self.frame.add_controller(hub.scroll_controller());
        if let Host::Window(window) = &self.host {
            let keys = gtk::EventControllerKey::new();
            let this = weak.clone();
            keys.connect_key_pressed(move |_, _, code, _| {
                if let Some((_, hub)) = both(&this) {
                    hub.send_key(code, true);
                }
                glib::Propagation::Stop
            });
            let this = weak;
            keys.connect_key_released(move |_, _, code, _| {
                if let Some((_, hub)) = both(&this) {
                    hub.send_key(code, false);
                }
            });
            window.add_controller(keys);
        }
    }

    fn on_motion(&self, hub: &Hub, x: f64, y: f64) {
        let moved = self.held.borrow().as_ref().map(|held| is_move(held.at, (x, y)));
        match moved {
            // Still a click, which the guest gets whole when it is let go.
            Some(false) => {}
            Some(true) => self.start_window_move(),
            None => {
                let point = self.send_pointer(hub, x, y);
                hub.trace(&format!(
                    "motion in {} at ({x:.3},{y:.3}) is the guest's ({},{})",
                    self.id, point.x, point.y
                ));
            }
        }
    }

    /// The pointer went far enough with a press held on the title bar: the compositor moves the window
    /// from here on, and the guest never hears of the press.
    fn start_window_move(&self) {
        let Some(held) = self.held.borrow_mut().take() else { return };
        self.moved_window.set(true);
        let Host::Window(_) = &self.host else { return };
        let Some(native) = self.frame.native() else { return };
        let at = graphene::Point::new(held.at.0 as f32, held.at.1 as f32);
        let Some(on_surface) = self.frame.compute_point(&native, &at) else { return };
        let (shift_x, shift_y) = native.surface_transform();
        if let Some(toplevel) = native.surface().and_then(|surface| surface.downcast::<gdk::Toplevel>().ok())
        {
            toplevel.begin_move(
                &held.device,
                PRIMARY_BUTTON as i32,
                f64::from(on_surface.x()) + shift_x,
                f64::from(on_surface.y()) + shift_y,
                held.time,
            );
        }
        // The compositor has the button from here on, and the release never reaches GTK. The click
        // gesture would wait for it and take no press again: on KWin every click after a move was lost.
        held.gesture.reset();
    }

    fn on_button(&self, hub: &Hub, gesture: &gtk::GestureClick, x: f64, y: f64, down: bool) {
        let button = gesture.current_button();
        if down {
            hub.focus(&self.cell.borrow());
            let strip = !self.is_popup() && button == PRIMARY_BUTTON && {
                let (shift_x, shift_y) = self.frame.picture_shift();
                on_drag_strip(
                    &self.cell.borrow(),
                    self.surface(),
                    x - f64::from(shift_x),
                    y - f64::from(shift_y),
                )
            };
            if strip
                && let Some(event) = gesture.current_event()
                && let Some(device) = event.device()
            {
                self.moved_window.set(false);
                *self.held.borrow_mut() =
                    Some(HeldPress { at: (x, y), device, time: event.time(), gesture: gesture.clone() });
                return;
            }
            self.moved_window.set(false);
            *self.held.borrow_mut() = None;
        } else {
            if let Some(held) = self.held.borrow_mut().take() {
                // A press on the title bar that did not move the window is a click, where it was.
                self.send_pointer(hub, held.at.0, held.at.1);
                if let Some(name) = button_name(button) {
                    hub.send_button(name, true);
                    hub.send_button(name, false);
                }
                return;
            }
            if self.moved_window.replace(false) {
                return;
            }
        }
        if let Some(name) = button_name(button) {
            // Left unclaimed, back would also be the desktop's.
            if matches!(button, BACK | FORWARD) {
                gesture.set_state(gtk::EventSequenceState::Claimed);
            }
            let point = self.send_pointer(hub, x, y);
            if down {
                hub.trace(&format!(
                    "press {name} in {} at ({x:.1},{y:.1}) is the guest's ({},{})",
                    self.id, point.x, point.y
                ));
            }
            hub.send_button(name, down);
        }
    }

    /// The host window's own life: the focus, the restore of a window the guest minimized, and the
    /// compositor's close.
    fn watch_window(self: &Rc<Self>, hub: &Rc<Hub>, window: &gtk::Window) {
        let weak = (Rc::downgrade(self), Rc::downgrade(hub));
        let follow = {
            let weak = weak.clone();
            move |window: &gtk::Window| {
                let Some((view, hub)) = both(&weak) else { return };
                let (active, suspended) = (window.is_active(), window.is_suspended());
                let (presence, change) = follow_host_state(view.presence.get(), active, suspended);
                view.presence.set(presence);
                hub.trace(&format!(
                    "host window {}: active {active}, out of sight {suspended}, {presence:?}",
                    view.id
                ));
                match change {
                    HostChange::Restore => hub.request(Request::Restore { id: view.id.clone() }),
                    HostChange::Nothing => {}
                }
                if active && !suspended {
                    hub.focus(&view.cell.borrow());
                } else if !active {
                    hub.lost_focus(&view.id);
                }
            }
        };
        let on_active = follow.clone();
        window.connect_is_active_notify(move |window| on_active(window));
        window.connect_suspended_notify(move |window| follow(window));
        window.connect_close_request(move |_| {
            // The window goes when its cell does, and not before: a program may ask to save first.
            if let Some((view, hub)) = both(&weak) {
                hub.request(Request::Close { id: view.id.clone() });
            }
            glib::Propagation::Stop
        });
    }
}

fn both(pair: &(Weak<View>, Weak<Hub>)) -> Option<(Rc<View>, Rc<Hub>)> {
    Some((pair.0.upgrade()?, pair.1.upgrade()?))
}
