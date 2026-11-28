// SPDX-License-Identifier: GPL-2.0-or-later
//! The Mac's keys as Linux knows them, and QEMU's names for them.
//!
//! The window hands on a key by the Mac's virtual key code, and QEMU takes it by its own name for
//! the key, which its virtio keyboard turns into the Linux key code. The table is made from the
//! data of keycodemapdb, the key code database QEMU builds its own tables from, which may be used
//! under the GPL 2 or later or the 3-clause BSD licence.

/// A key as the guest gets it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    /// Its Linux key code, as the guest's `/dev/input` reports it.
    pub linux: u16,
    /// QEMU's name for it, as `input-send-event` takes it.
    pub qcode: &'static str,
}

/// The Mac's names for the key beside the left Shift and the key left of 1 on a keyboard of the ISO
/// kind are each other's.
const ISO_SECTION: u16 = 0x0a;
const ANSI_GRAVE: u16 = 0x32;
/// Caps Lock, which the window hands on as a press and a release at each change.
#[cfg(test)]
pub const CAPS_LOCK: u16 = 0x39;

/// The key with the Mac's virtual key code `code`, on a keyboard of the ISO kind or not, if the
/// guest has one.
pub fn key(code: u16, iso: bool) -> Option<Key> {
    let code = match code {
        ISO_SECTION if iso => ANSI_GRAVE,
        ANSI_GRAVE if iso => ISO_SECTION,
        other => other,
    };
    let index = KEYS.binary_search_by_key(&code, |&(mac, _, _)| mac).ok()?;
    let (_, linux, qcode) = KEYS[index];
    Some(Key { linux, qcode })
}

// Made from keycodemapdb's data/keymaps.csv at f5772a62ec52.
// The Mac's virtual key code, the Linux key code, and QEMU's name for the key.
pub const KEYS: [(u16, u16, &str); 118] = [
    (0x00, 30, "a"),             // ANSI_A
    (0x01, 31, "s"),             // ANSI_S
    (0x02, 32, "d"),             // ANSI_D
    (0x03, 33, "f"),             // ANSI_F
    (0x04, 35, "h"),             // ANSI_H
    (0x05, 34, "g"),             // ANSI_G
    (0x06, 44, "z"),             // ANSI_Z
    (0x07, 45, "x"),             // ANSI_X
    (0x08, 46, "c"),             // ANSI_C
    (0x09, 47, "v"),             // ANSI_V
    (0x0a, 86, "less"),          // ISO_Section
    (0x0b, 48, "b"),             // ANSI_B
    (0x0c, 16, "q"),             // ANSI_Q
    (0x0d, 17, "w"),             // ANSI_W
    (0x0e, 18, "e"),             // ANSI_E
    (0x0f, 19, "r"),             // ANSI_R
    (0x10, 21, "y"),             // ANSI_Y
    (0x11, 20, "t"),             // ANSI_T
    (0x12, 2, "1"),              // ANSI_1
    (0x13, 3, "2"),              // ANSI_2
    (0x14, 4, "3"),              // ANSI_3
    (0x15, 5, "4"),              // ANSI_4
    (0x16, 7, "6"),              // ANSI_6
    (0x17, 6, "5"),              // ANSI_5
    (0x18, 13, "equal"),         // ANSI_Equal
    (0x19, 10, "9"),             // ANSI_9
    (0x1a, 8, "7"),              // ANSI_7
    (0x1b, 12, "minus"),         // ANSI_Minus
    (0x1c, 9, "8"),              // ANSI_8
    (0x1d, 11, "0"),             // ANSI_0
    (0x1e, 27, "bracket_right"), // ANSI_RightBracket
    (0x1f, 24, "o"),             // ANSI_O
    (0x20, 22, "u"),             // ANSI_U
    (0x21, 26, "bracket_left"),  // ANSI_LeftBracket
    (0x22, 23, "i"),             // ANSI_I
    (0x23, 25, "p"),             // ANSI_P
    (0x24, 28, "ret"),           // Return
    (0x25, 38, "l"),             // ANSI_L
    (0x26, 36, "j"),             // ANSI_J
    (0x27, 40, "apostrophe"),    // ANSI_Quote
    (0x28, 37, "k"),             // ANSI_K
    (0x29, 39, "semicolon"),     // ANSI_Semicolon
    (0x2a, 43, "backslash"),     // ANSI_Backslash
    (0x2b, 51, "comma"),         // ANSI_Comma
    (0x2c, 53, "slash"),         // ANSI_Slash
    (0x2d, 49, "n"),             // ANSI_N
    (0x2e, 50, "m"),             // ANSI_M
    (0x2f, 52, "dot"),           // ANSI_Period
    (0x30, 15, "tab"),           // Tab
    (0x31, 57, "spc"),           // Space
    (0x32, 41, "grave_accent"),  // ANSI_Grave
    (0x33, 14, "backspace"),     // Delete
    (0x35, 1, "esc"),            // Escape
    (0x36, 126, "meta_r"),       // RightCommand
    (0x37, 125, "meta_l"),       // Command
    (0x38, 42, "shift"),         // Shift
    (0x39, 58, "caps_lock"),     // CapsLock
    (0x3a, 56, "alt"),           // Option
    (0x3b, 29, "ctrl"),          // Control
    (0x3c, 54, "shift_r"),       // RightShift
    (0x3d, 100, "alt_r"),        // RightOption
    (0x3e, 97, "ctrl_r"),        // RightControl
    (0x40, 187, "f17"),          // F17
    (0x41, 83, "kp_decimal"),    // ANSI_KeypadDecimal
    (0x43, 55, "asterisk"),      // ANSI_KeypadMultiply
    (0x45, 78, "kp_add"),        // ANSI_KeypadPlus
    (0x47, 69, "num_lock"),      // ANSI_KeypadClear
    (0x48, 115, "volumeup"),     // VolumeUp
    (0x49, 114, "volumedown"),   // VolumeDown
    (0x4a, 113, "audiomute"),    // Mute
    (0x4b, 98, "kp_divide"),     // ANSI_KeypadDivide
    (0x4c, 96, "kp_enter"),      // ANSI_KeypadEnter
    (0x4e, 74, "kp_subtract"),   // ANSI_KeypadMinus
    (0x4f, 188, "f18"),          // F18
    (0x50, 189, "f19"),          // F19
    (0x51, 117, "kp_equals"),    // ANSI_KeypadEquals
    (0x52, 82, "kp_0"),          // ANSI_Keypad0
    (0x53, 79, "kp_1"),          // ANSI_Keypad1
    (0x54, 80, "kp_2"),          // ANSI_Keypad2
    (0x55, 81, "kp_3"),          // ANSI_Keypad3
    (0x56, 75, "kp_4"),          // ANSI_Keypad4
    (0x57, 76, "kp_5"),          // ANSI_Keypad5
    (0x58, 77, "kp_6"),          // ANSI_Keypad6
    (0x59, 71, "kp_7"),          // ANSI_Keypad7
    (0x5a, 190, "f20"),          // F20
    (0x5b, 72, "kp_8"),          // ANSI_Keypad8
    (0x5c, 73, "kp_9"),          // ANSI_Keypad9
    (0x5d, 124, "yen"),          // JIS_Yen
    (0x5e, 89, "ro"),            // JIS_Underscore
    (0x60, 63, "f5"),            // F5
    (0x61, 64, "f6"),            // F6
    (0x62, 65, "f7"),            // F7
    (0x63, 61, "f3"),            // F3
    (0x64, 66, "f8"),            // F8
    (0x65, 67, "f9"),            // F9
    (0x66, 123, "lang2"),        // JIS_Eisu
    (0x67, 87, "f11"),           // F11
    (0x68, 122, "lang1"),        // JIS_Kana
    (0x69, 183, "f13"),          // F13
    (0x6a, 186, "f16"),          // F16
    (0x6b, 184, "f14"),          // F14
    (0x6d, 68, "f10"),           // F10
    (0x6e, 127, "compose"),      // KEY_COMPOSE
    (0x6f, 88, "f12"),           // F12
    (0x71, 185, "f15"),          // F15
    (0x72, 138, "help"),         // Help
    (0x73, 102, "home"),         // Home
    (0x74, 104, "pgup"),         // PageUp
    (0x75, 111, "delete"),       // ForwardDelete
    (0x76, 62, "f4"),            // F4
    (0x77, 107, "end"),          // End
    (0x78, 60, "f2"),            // F2
    (0x79, 109, "pgdn"),         // PageDown
    (0x7a, 59, "f1"),            // F1
    (0x7b, 105, "left"),         // LeftArrow
    (0x7c, 106, "right"),        // RightArrow
    (0x7d, 108, "down"),         // DownArrow
    (0x7e, 103, "up"),           // UpArrow
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_in_order_with_one_row_a_key() {
        assert!(KEYS.windows(2).all(|pair| pair[0].0 < pair[1].0));
    }

    #[test]
    fn keys_are_linux_codes_and_qemu_names() {
        assert_eq!(key(0x00, false), Some(Key { linux: 30, qcode: "a" }));
        assert_eq!(key(0x24, false), Some(Key { linux: 28, qcode: "ret" }));
        assert_eq!(key(0x37, false), Some(Key { linux: 125, qcode: "meta_l" }));
        assert_eq!(key(0x3a, false), Some(Key { linux: 56, qcode: "alt" }));
        assert_eq!(key(0x3d, false), Some(Key { linux: 100, qcode: "alt_r" }));
        assert_eq!(key(CAPS_LOCK, false), Some(Key { linux: 58, qcode: "caps_lock" }));
        assert_eq!(key(0x7e, false), Some(Key { linux: 103, qcode: "up" }));
        // Fn has no key in a guest.
        assert_eq!(key(0x3f, false), None);
        assert_eq!(key(0xff, false), None);
    }

    #[test]
    fn an_iso_keyboard_swaps_the_two_keys_the_mac_names_the_other_way() {
        assert_eq!(key(ISO_SECTION, false).map(|key| key.linux), Some(86));
        assert_eq!(key(ANSI_GRAVE, false).map(|key| key.linux), Some(41));
        assert_eq!(key(ISO_SECTION, true).map(|key| key.linux), Some(41));
        assert_eq!(key(ANSI_GRAVE, true).map(|key| key.linux), Some(86));
        assert_eq!(key(0x00, true).map(|key| key.linux), Some(30));
    }
}
