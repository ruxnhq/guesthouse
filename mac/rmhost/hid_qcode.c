/*
 * USB HID keyboard usages (page 7) to QEMU's key names (QKeyCode in
 * qapi/ui.json), by physical position.
 *
 * SPDX-License-Identifier: MIT
 * Copyright (c) 2026 RUXN LLC and contributors
 */
#include <stddef.h>
#include <stdint.h>

#include "qmp_input.h"

static const char *const keys[256] = {
    [0x04] = "a", [0x05] = "b", [0x06] = "c", [0x07] = "d", [0x08] = "e",
    [0x09] = "f", [0x0a] = "g", [0x0b] = "h", [0x0c] = "i", [0x0d] = "j",
    [0x0e] = "k", [0x0f] = "l", [0x10] = "m", [0x11] = "n", [0x12] = "o",
    [0x13] = "p", [0x14] = "q", [0x15] = "r", [0x16] = "s", [0x17] = "t",
    [0x18] = "u", [0x19] = "v", [0x1a] = "w", [0x1b] = "x", [0x1c] = "y",
    [0x1d] = "z",
    [0x1e] = "1", [0x1f] = "2", [0x20] = "3", [0x21] = "4", [0x22] = "5",
    [0x23] = "6", [0x24] = "7", [0x25] = "8", [0x26] = "9", [0x27] = "0",
    [0x28] = "ret", [0x29] = "esc", [0x2a] = "backspace", [0x2b] = "tab",
    [0x2c] = "spc", [0x2d] = "minus", [0x2e] = "equal",
    [0x2f] = "bracket_left", [0x30] = "bracket_right", [0x31] = "backslash",
    /* The key left of Return on ISO boards: QEMU calls it backslash too. */
    [0x32] = "backslash",
    [0x33] = "semicolon", [0x34] = "apostrophe", [0x35] = "grave_accent",
    [0x36] = "comma", [0x37] = "dot", [0x38] = "slash", [0x39] = "caps_lock",
    [0x3a] = "f1", [0x3b] = "f2", [0x3c] = "f3", [0x3d] = "f4",
    [0x3e] = "f5", [0x3f] = "f6", [0x40] = "f7", [0x41] = "f8",
    [0x42] = "f9", [0x43] = "f10", [0x44] = "f11", [0x45] = "f12",
    [0x46] = "print", [0x47] = "scroll_lock", [0x48] = "pause",
    [0x49] = "insert", [0x4a] = "home", [0x4b] = "pgup", [0x4c] = "delete",
    [0x4d] = "end", [0x4e] = "pgdn", [0x4f] = "right", [0x50] = "left",
    [0x51] = "down", [0x52] = "up",
    [0x53] = "num_lock", [0x54] = "kp_divide", [0x55] = "kp_multiply",
    [0x56] = "kp_subtract", [0x57] = "kp_add", [0x58] = "kp_enter",
    [0x59] = "kp_1", [0x5a] = "kp_2", [0x5b] = "kp_3", [0x5c] = "kp_4",
    [0x5d] = "kp_5", [0x5e] = "kp_6", [0x5f] = "kp_7", [0x60] = "kp_8",
    [0x61] = "kp_9", [0x62] = "kp_0", [0x63] = "kp_decimal",
    /* The extra key beside left Shift on ISO boards. */
    [0x64] = "less",
    /* The Application (context menu) key. */
    [0x65] = "compose",
    [0x66] = "power", [0x67] = "kp_equals",
    [0x68] = "f13", [0x69] = "f14", [0x6a] = "f15", [0x6b] = "f16",
    [0x6c] = "f17", [0x6d] = "f18", [0x6e] = "f19", [0x6f] = "f20",
    [0x70] = "f21", [0x71] = "f22", [0x72] = "f23", [0x73] = "f24",
    [0x75] = "help", [0x76] = "menu", [0x77] = "front", [0x78] = "stop",
    [0x79] = "again", [0x7a] = "undo", [0x7b] = "cut", [0x7c] = "copy",
    [0x7d] = "paste", [0x7e] = "find", [0x7f] = "audiomute",
    [0x80] = "volumeup", [0x81] = "volumedown",
    [0x85] = "kp_comma", [0x87] = "ro", [0x88] = "katakanahiragana",
    [0x89] = "yen", [0x8a] = "henkan", [0x8b] = "muhenkan",
    [0x90] = "lang1", [0x91] = "lang2",
    [0xe0] = "ctrl", [0xe1] = "shift", [0xe2] = "alt", [0xe3] = "meta_l",
    [0xe4] = "ctrl_r", [0xe5] = "shift_r", [0xe6] = "alt_r", [0xe7] = "meta_r",
};

const char *hid_usage_to_qcode(uint16_t usage)
{
    return usage < 256 ? keys[usage] : NULL;
}
