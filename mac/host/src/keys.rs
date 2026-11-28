// SPDX-License-Identifier: GPL-2.0-or-later
//! macOS key codes (the `kVK_` constants of Carbon's `Events.h`, which `NSEvent.keyCode`
//! gives) to QEMU's key names (`QKeyCode` in QEMU's `qapi/ui.json`). Both name a key by its
//! place on the keyboard, not by the letter printed on it, so the guest's own layout decides
//! what a key types.

/// The Caps Lock key. macOS reports it as one change of state per press, so it is sent to the
/// guest as a press and a release each time.
pub const CAPS_LOCK: u16 = 0x39;

/// QEMU's name for the key with macOS key code `code`, or `None` for a key the guest has no
/// name for (such as Fn).
pub fn qcode(code: u16) -> Option<&'static str> {
    let name = match code {
        0x00 => "a",
        0x01 => "s",
        0x02 => "d",
        0x03 => "f",
        0x04 => "h",
        0x05 => "g",
        0x06 => "z",
        0x07 => "x",
        0x08 => "c",
        0x09 => "v",
        // The extra key of ISO boards. Apple's ISO boards swap it with the key left of 1,
        // which then reports as 0x32; macOS cannot tell us which board this is.
        0x0A => "less",
        0x0B => "b",
        0x0C => "q",
        0x0D => "w",
        0x0E => "e",
        0x0F => "r",
        0x10 => "y",
        0x11 => "t",
        0x12 => "1",
        0x13 => "2",
        0x14 => "3",
        0x15 => "4",
        0x16 => "6",
        0x17 => "5",
        0x18 => "equal",
        0x19 => "9",
        0x1A => "7",
        0x1B => "minus",
        0x1C => "8",
        0x1D => "0",
        0x1E => "bracket_right",
        0x1F => "o",
        0x20 => "u",
        0x21 => "bracket_left",
        0x22 => "i",
        0x23 => "p",
        0x24 => "ret",
        0x25 => "l",
        0x26 => "j",
        0x27 => "apostrophe",
        0x28 => "k",
        0x29 => "semicolon",
        0x2A => "backslash",
        0x2B => "comma",
        0x2C => "slash",
        0x2D => "n",
        0x2E => "m",
        0x2F => "dot",
        0x30 => "tab",
        0x31 => "spc",
        0x32 => "grave_accent",
        0x33 => "backspace",
        0x35 => "esc",
        // Command is the Windows key.
        0x36 => "meta_r",
        0x37 => "meta_l",
        0x38 => "shift",
        CAPS_LOCK => "caps_lock",
        0x3A => "alt",
        0x3B => "ctrl",
        0x3C => "shift_r",
        0x3D => "alt_r",
        0x3E => "ctrl_r",
        0x40 => "f17",
        0x41 => "kp_decimal",
        0x43 => "kp_multiply",
        0x45 => "kp_add",
        // Clear, where a PC keyboard has Num Lock.
        0x47 => "num_lock",
        0x48 => "volumeup",
        0x49 => "volumedown",
        0x4A => "audiomute",
        0x4B => "kp_divide",
        0x4C => "kp_enter",
        0x4E => "kp_subtract",
        0x4F => "f18",
        0x50 => "f19",
        0x51 => "kp_equals",
        0x52 => "kp_0",
        0x53 => "kp_1",
        0x54 => "kp_2",
        0x55 => "kp_3",
        0x56 => "kp_4",
        0x57 => "kp_5",
        0x58 => "kp_6",
        0x59 => "kp_7",
        0x5A => "f20",
        0x5B => "kp_8",
        0x5C => "kp_9",
        0x5D => "yen",
        0x5E => "ro",
        0x5F => "kp_comma",
        0x60 => "f5",
        0x61 => "f6",
        0x62 => "f7",
        0x63 => "f3",
        0x64 => "f8",
        0x65 => "f9",
        0x66 => "lang2",
        0x67 => "f11",
        0x68 => "lang1",
        0x69 => "f13",
        // Print Screen, which a Mac keyboard has no key for: a code of ours, for Send Keys.
        PRINT_SCREEN => "print",
        0x6A => "f16",
        0x6B => "f14",
        0x6D => "f10",
        // The context menu key of PC keyboards.
        0x6E => "compose",
        0x6F => "f12",
        0x71 => "f15",
        // Help sits where a PC keyboard has Insert.
        0x72 => "insert",
        0x73 => "home",
        0x74 => "pgup",
        0x75 => "delete",
        0x76 => "f4",
        0x77 => "end",
        0x78 => "f2",
        0x79 => "pgdn",
        0x7A => "f1",
        0x7B => "left",
        0x7C => "right",
        0x7D => "down",
        0x7E => "up",
        _ => return None,
    };
    Some(name)
}

/// The code Send Keys uses for Print Screen. macOS key codes stop at 0x7F or so; this is not one.
pub const PRINT_SCREEN: u16 = 0xF001;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_and_modifiers() {
        assert_eq!(qcode(0x00), Some("a"));
        assert_eq!(qcode(0x0C), Some("q"));
        assert_eq!(qcode(0x37), Some("meta_l"));
        assert_eq!(qcode(0x3B), Some("ctrl"));
        assert_eq!(qcode(0x3E), Some("ctrl_r"));
        assert_eq!(qcode(CAPS_LOCK), Some("caps_lock"));
    }

    #[test]
    fn function_and_arrow_keys() {
        assert_eq!(qcode(0x7A), Some("f1"));
        assert_eq!(qcode(0x6F), Some("f12"));
        assert_eq!(qcode(0x7E), Some("up"));
        assert_eq!(qcode(0x75), Some("delete"));
    }

    #[test]
    fn print_screen_is_our_own_code() {
        assert_eq!(qcode(PRINT_SCREEN), Some("print"));
    }

    #[test]
    fn fn_and_gaps_have_no_name() {
        // Fn, and codes macOS leaves unused.
        assert_eq!(qcode(0x3F), None);
        assert_eq!(qcode(0x34), None);
        assert_eq!(qcode(0x80), None);
    }

    #[test]
    fn no_two_keys_share_a_name() {
        let mut names: Vec<_> = (0..=0x7F).filter_map(qcode).collect();
        let all = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), all);
    }
}
