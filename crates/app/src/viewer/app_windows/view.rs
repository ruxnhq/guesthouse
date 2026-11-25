//! One host window or popup of a guest cell: a part of the guest's frame in a widget of its own, and
//! the pointer, keys and window events that go back to the guest. The frames and the input themselves
//! are the hub's, shared by every view.

use super::cell_picture::{CellPicture, Guest};
use super::hub::Hub;
use super::plan::{HostChange, Order, Presence, follow_guest_state, follow_host_state, point_for};
use crate::viewer::input::button_name;
use crate::viewer::title_drag::{PRIMARY_BUTTON, is_move};
use adw::prelude::*;
use gtk::{gdk, glib, graphene};
use guesthouse_app_core::app_windows::{
    Cell, Fields, Point, Request, Size, State, Surface, crop, on_drag_strip, popup_place,
};
use std::cell::{Cell as Slot, RefCell};
use std::rc::{Rc, Weak};

const BACK: u32 = 8;
const FORWARD: u32 = 9;
/// The window under a popup's picture has nothing of its own to draw, and neither has the popup.
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
                    .resizable(false)
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
        let view = Rc::new(View {
            id: cell.id.clone(),
            host,
            frame,
            cell: RefCell::new(cell.clone()),
            presence: Slot::new(Presence::Up),
            held: RefCell::default(),
            moved_window: Slot::new(false),
            children: RefCell::default(),
        });
        view.recrop(hub.screen_size());
        view.add_input(hub);
        view.watch_scale(hub);
        match &view.host {
            Host::Window(window) => view.watch_window(hub, window),
            Host::Popup(popover) => {
                if let Some(parent) = parent {
                    popover.set_parent(&parent.frame);
                    parent.children.borrow_mut().push(view.id.clone());
                    view.place_popup(parent);
                }
            }
        }
        if let Some(texture) = hub.latest() {
            view.set_texture(&texture);
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
            Host::Popup(popover) => popover.popup(),
        }
        view
    }

    pub fn is_popup(&self) -> bool {
        matches!(self.host, Host::Popup(_))
    }

    /// A new frame, which every view shows.
    pub fn set_texture(&self, texture: &gdk::Texture) {
        self.frame.set_texture(texture);
    }

    /// Windows' scale or the steps it offers changed.
    pub fn set_guest(&self, guest: Guest) {
        self.frame.set_guest(guest);
    }

    /// The scale of the surface the view is on, as the compositor gave it.
    pub fn surface_scale(&self) -> f64 {
        self.frame.surface_scale()
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
        point_for(&self.cell.borrow(), self.surface(), Point::default(), x, y, screen)
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
            let strip = !self.is_popup()
                && button == PRIMARY_BUTTON
                && on_drag_strip(&self.cell.borrow(), self.surface(), x, y);
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

    /// The host window's own life: the focus, minimize and restore, and the compositor's close.
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
                    HostChange::Minimize => hub.request(Request::Minimize { id: view.id.clone() }),
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
