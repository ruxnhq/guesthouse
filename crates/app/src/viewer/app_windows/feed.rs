//! The feed's side of the input: what the views ask of the screen that owns the machine's keyboard and
//! pointer. The screen is in no window, so these take guest pixels and never a widget's point.

use super::plan::tablet_value;
use crate::viewer::screen::Screen;
use guesthouse_app_core::app_windows::{Point, Size};

impl Screen {
    /// Puts the guest's pointer on a pixel of its screen.
    pub fn send_pointer(&self, point: Point, screen: Size) {
        if let Some(input) = self.0.forwarded.borrow_mut().as_mut() {
            input.move_to(f64::from(point.x) + 0.5, f64::from(point.y) + 0.5);
            return;
        }
        let position = (tablet_value(point.x, screen.w), tablet_value(point.y, screen.h));
        // The same pixel again is not worth the guest's time.
        if self.0.pointer.get() != Some(position)
            && let Some(guest) = self.0.guest.borrow_mut().as_mut()
        {
            self.0.pointer.set(Some(position));
            guest.move_to(position.0, position.1);
        }
    }

    pub fn send_button(&self, name: &'static str, down: bool) {
        self.press(name, down);
    }

    pub fn send_key(&self, keycode: u32, down: bool) {
        self.on_key(keycode, down);
    }

    /// Lets go of every key and button held down in the guest.
    pub fn release_everything(&self) {
        self.let_go();
    }
}
