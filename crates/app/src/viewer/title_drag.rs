//! Lab only: moving a crop window by the guest program's own title bar, which is in the picture.
//!
//! The crop window has no title bar of the host's. A press with the primary button on the strip the
//! caller names is held back from the guest: let go where it was, it reaches the guest as a click,
//! so the program's own title bar still takes clicks and double clicks. Dragged further than a few
//! pixels, it becomes a move of the host's window that the compositor drives, and the guest hears
//! nothing of it, so no button is left down there. The caption buttons are left out of the strip, and
//! every press on them goes to the guest at once.

use super::screen::Screen;
use adw::prelude::*;
use gtk::{gdk, graphene};

/// How far the pointer goes with the button down, in the window's logical pixels, before a press on
/// the title bar moves the window rather than clicks. A hand that clicks moves a pixel or two.
const MOVE_AFTER: f64 = 4.0;
const PRIMARY_BUTTON: u32 = 1;

/// The part of the rectangle that moves the window: this high from its top, short of this much at
/// its right for the caption buttons. In the guest's pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DragStrip {
    pub height: i32,
    pub right: i32,
}

impl DragStrip {
    /// Whether a point of the rectangle, from its corner in the guest's pixels, is on the strip.
    pub fn holds(self, (x, y): (f64, f64), rectangle_width: i32) -> bool {
        let end = f64::from(rectangle_width - self.right);
        (0.0..f64::from(self.height)).contains(&y) && (0.0..end).contains(&x)
    }
}

/// Whether the pointer has gone far enough from the press to be a move.
fn is_move((from_x, from_y): (f64, f64), (to_x, to_y): (f64, f64)) -> bool {
    (to_x - from_x).hypot(to_y - from_y) > MOVE_AFTER
}

/// A press on the strip that the guest hasn't been told of.
struct HeldPress {
    at: (f64, f64),
    device: gdk::Device,
    time: u32,
    gesture: gtk::GestureClick,
}

#[derive(Default)]
pub(super) struct TitleDrag {
    strip: Option<DragStrip>,
    held: Option<HeldPress>,
    /// The last press moved the window, so its release is no one's.
    moved_window: bool,
}

/// What a release means once the title bar has had its say.
pub(super) enum TitleRelease {
    /// A press that was held back and didn't move: the guest gets it now, where it was.
    Click((f64, f64)),
    /// The press moved the window.
    Swallowed,
    /// Nothing of the title bar's.
    Not,
}

impl Screen {
    /// Lab only: lets a press on this strip of the rectangle move the window, or `None` for none.
    pub fn set_title_drag(&self, strip: Option<DragStrip>) {
        let mut drag = self.0.title_drag.borrow_mut();
        drag.strip = strip;
        drag.held = None;
    }

    /// Holds a press back from the guest if it's on the strip, and says so.
    pub(super) fn hold_title_press(&self, gesture: &gtk::GestureClick, button: u32, x: f64, y: f64) -> bool {
        let mut drag = self.0.title_drag.borrow_mut();
        drag.moved_window = false;
        drag.held = None;
        let Some(strip) = drag.strip.filter(|_| button == PRIMARY_BUTTON) else { return false };
        let Some(crop) = self.0.aspect.shown_crop() else { return false };
        let point = graphene::Point::new(x as f32, y as f32);
        let on_frame = self.0.picture.compute_point(&self.0.aspect, &point);
        let in_rectangle = on_frame.and_then(|point| self.0.aspect.in_rectangle(&point));
        if !in_rectangle.is_some_and(|point| strip.holds(point, crop.width)) {
            return false;
        }
        let Some(event) = gesture.current_event() else { return false };
        let Some(device) = event.device() else { return false };
        drag.held = Some(HeldPress { at: (x, y), device, time: event.time(), gesture: gesture.clone() });
        true
    }

    /// The pointer moved with a press held back. Far enough, and the window moves with it, and the
    /// guest never hears of the press. Says whether the motion was the title bar's.
    pub(super) fn title_press_moved(&self, x: f64, y: f64) -> bool {
        let mut drag = self.0.title_drag.borrow_mut();
        let Some(held) = drag.held.take_if(|held| is_move(held.at, (x, y))) else {
            // Still a click, or no press held at all: the guest's pointer may follow.
            return false;
        };
        drag.moved_window = true;
        drop(drag);
        let Some(native) = self.0.picture.native() else { return true };
        let at = graphene::Point::new(held.at.0 as f32, held.at.1 as f32);
        let Some(on_surface) = self.0.picture.compute_point(&native, &at) else { return true };
        let (shift_x, shift_y) = native.surface_transform();
        if let Some(toplevel) = self.toplevel() {
            toplevel.begin_move(
                &held.device,
                PRIMARY_BUTTON as i32,
                f64::from(on_surface.x()) + shift_x,
                f64::from(on_surface.y()) + shift_y,
                held.time,
            );
        }
        // The compositor has the button from here on, and the release never reaches GTK. The click
        // gesture would wait for it, and take no press again: on KWin every click after a move was
        // lost.
        held.gesture.reset();
        true
    }

    pub(super) fn title_press_released(&self) -> TitleRelease {
        let mut drag = self.0.title_drag.borrow_mut();
        if let Some(held) = drag.held.take() {
            return TitleRelease::Click(held.at);
        }
        if std::mem::take(&mut drag.moved_window) {
            return TitleRelease::Swallowed;
        }
        TitleRelease::Not
    }

    /// A press held back is forgotten, as when the window loses the focus before it's let go.
    pub(super) fn forget_title_press(&self) {
        self.0.title_drag.borrow_mut().held = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Calculator's at 100%: a title bar 32 high, its caption buttons 138 wide.
    const CALCULATOR: DragStrip = DragStrip { height: 32, right: 138 };

    #[test]
    fn the_strip_is_the_title_bar_short_of_the_caption_buttons() {
        let width = 320;
        assert!(CALCULATOR.holds((0.0, 0.0), width));
        assert!(CALCULATOR.holds((181.9, 31.9), width));
        assert!(!CALCULATOR.holds((182.0, 10.0), width), "the minimise button");
        assert!(!CALCULATOR.holds((300.0, 10.0), width), "the close button");
        assert!(!CALCULATOR.holds((50.0, 32.0), width), "the program under its title bar");
        assert!(!CALCULATOR.holds((-1.0, 10.0), width));
    }

    #[test]
    fn a_rectangle_narrower_than_its_buttons_has_no_strip() {
        assert!(!CALCULATOR.holds((10.0, 10.0), 100));
    }

    #[test]
    fn a_hand_that_clicks_is_not_a_move() {
        assert!(!is_move((10.0, 10.0), (12.0, 13.0)));
        assert!(is_move((10.0, 10.0), (15.0, 10.0)));
        assert!(is_move((10.0, 10.0), (10.0, 4.0)));
    }
}
