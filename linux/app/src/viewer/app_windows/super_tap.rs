//! The Super key, between the desktop and the guest's app windows. App windows sit among the
//! desktop's own, so a Super tapped on its own is the desktop's: its launcher or overview. The guest
//! would open its Start menu on the same tap. So a Super press is held back until the next key
//! says what it is: with a key or a button while it's down it's a combination, and the guest gets
//! Super and then that key at once; let go alone, the guest never hears of it.
//!
//! The viewer of a machine's whole screen is not this: there the keyboard can be the guest's
//! outright, and Super goes to it as any key does.

/// GDK's hardware keycodes of the two Super keys: evdev's `KEY_LEFTMETA` and `KEY_RIGHTMETA`,
/// plus the 8 that X keycodes are offset by.
const SUPER_KEYS: [u32; 2] = [133, 134];

/// A key or button going to the guest: its keycode, and whether it went down.
pub type KeyEvent = (u32, bool);

#[derive(Debug, Default)]
pub struct SuperTap {
    /// The Super key that is down and not yet sent.
    held: Option<u32>,
    /// The held Super has gone to the guest, as part of a combination.
    sent: bool,
}

pub fn is_super(keycode: u32) -> bool {
    SUPER_KEYS.contains(&keycode)
}

impl SuperTap {
    /// What the guest gets for this key.
    pub fn key(&mut self, keycode: u32, down: bool) -> Vec<KeyEvent> {
        match (self.held, is_super(keycode), down) {
            // The first Super down is held back. Its repeats, and the other Super, add nothing.
            (None, true, true) => {
                self.held = Some(keycode);
                Vec::new()
            }
            (Some(_), true, true) => Vec::new(),
            (Some(held), true, false) if held == keycode => {
                self.held = None;
                // Sent as part of a combination, the guest has it down and gets it up. Alone, it was
                // a tap, and the desktop's.
                if std::mem::take(&mut self.sent) { vec![(held, false)] } else { Vec::new() }
            }
            (Some(_), true, false) => Vec::new(),
            (Some(_), false, true) => {
                let mut events = self.combination();
                events.push((keycode, true));
                events
            }
            _ => vec![(keycode, down)],
        }
    }

    /// A button pressed with Super down makes a combination too. What goes to the guest first.
    pub fn button(&mut self) -> Vec<KeyEvent> {
        self.combination()
    }

    /// The held Super, sent down once: the key or button after it makes it a combination.
    fn combination(&mut self) -> Vec<KeyEvent> {
        match self.held {
            Some(held) if !self.sent => {
                self.sent = true;
                vec![(held, true)]
            }
            _ => Vec::new(),
        }
    }

    /// The window lost the focus, and everything the guest had down is let go: a Super held back
    /// is forgotten with it.
    pub fn forget(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUPER: u32 = 133;
    const RIGHT_SUPER: u32 = 134;
    const E: u32 = 26;
    const SHIFT: u32 = 50;
    const S: u32 = 39;

    fn run(tap: &mut SuperTap, keys: &[KeyEvent]) -> Vec<KeyEvent> {
        keys.iter().flat_map(|(code, down)| tap.key(*code, *down)).collect()
    }

    #[test]
    fn super_alone_is_the_desktops() {
        let mut tap = SuperTap::default();
        assert_eq!(run(&mut tap, &[(SUPER, true), (SUPER, false)]), []);
        // Held long enough to repeat, it's still a tap.
        assert_eq!(run(&mut tap, &[(SUPER, true), (SUPER, true), (SUPER, true), (SUPER, false)]), []);
    }

    #[test]
    fn a_combination_reaches_the_guest_at_the_second_key() {
        let mut tap = SuperTap::default();
        assert_eq!(tap.key(SUPER, true), []);
        assert_eq!(tap.key(E, true), [(SUPER, true), (E, true)]);
        assert_eq!(tap.key(E, false), [(E, false)]);
        assert_eq!(tap.key(SUPER, false), [(SUPER, false)]);
    }

    #[test]
    fn super_shift_s_goes_whole() {
        let mut tap = SuperTap::default();
        let keys = [(SUPER, true), (SHIFT, true), (S, true), (S, false), (SHIFT, false), (SUPER, false)];
        let sent = [(SUPER, true), (SHIFT, true), (S, true), (S, false), (SHIFT, false), (SUPER, false)];
        assert_eq!(run(&mut tap, &keys), sent);
    }

    #[test]
    fn two_combinations_in_one_hold_send_super_once() {
        let mut tap = SuperTap::default();
        let keys = [(SUPER, true), (E, true), (E, false), (S, true), (S, false), (SUPER, false)];
        let sent = [(SUPER, true), (E, true), (E, false), (S, true), (S, false), (SUPER, false)];
        assert_eq!(run(&mut tap, &keys), sent);
    }

    #[test]
    fn a_key_held_before_super_is_let_go_as_usual() {
        let mut tap = SuperTap::default();
        let keys = [(E, true), (SUPER, true), (E, false), (SUPER, false)];
        assert_eq!(run(&mut tap, &keys), [(E, true), (E, false)]);
    }

    #[test]
    fn the_other_super_while_one_is_held_adds_nothing() {
        let mut tap = SuperTap::default();
        let keys = [(SUPER, true), (RIGHT_SUPER, true), (RIGHT_SUPER, false), (SUPER, false)];
        assert_eq!(run(&mut tap, &keys), []);
    }

    #[test]
    fn a_click_with_super_down_is_a_combination() {
        let mut tap = SuperTap::default();
        assert_eq!(tap.key(SUPER, true), []);
        assert_eq!(tap.button(), [(SUPER, true)]);
        assert_eq!(tap.button(), []);
        assert_eq!(tap.key(SUPER, false), [(SUPER, false)]);
        assert_eq!(tap.button(), []);
    }

    #[test]
    fn losing_the_focus_forgets_a_held_super() {
        let mut tap = SuperTap::default();
        assert_eq!(tap.key(SUPER, true), []);
        tap.forget();
        assert_eq!(tap.key(E, true), [(E, true)]);
        assert_eq!(tap.key(SUPER, false), [(SUPER, false)]);
    }
}
