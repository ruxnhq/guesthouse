// SPDX-License-Identifier: GPL-2.0-or-later
//! The device against a guest played in this process's own memory. These call Apple's framework,
//! so they are a Mac's, and run where its build has a profile.
#![cfg(target_os = "macos")]

mod guest;

use guest::{Guest, LOWER, UPPER};
use guesthouse_pvg_sys::profile::PAGE_BYTES;
use guesthouse_pvg_sys::{Mode, Profile, host_build};

/// Whether this Mac's build is one the crate knows. Elsewhere the device refuses to be made, and
/// there is nothing to play against.
fn build_is_known() -> bool {
    let known = host_build().is_some_and(|build| Profile::for_build(&build).is_ok());
    if !known {
        eprintln!("this macOS build has no profile: nothing was run");
    }
    known
}

fn pixel(frame: &guesthouse_pvg_sys::Frame, x: u32, y: u32) -> u32 {
    let at = (y * frame.width + x) as usize * 4;
    u32::from_le_bytes(frame.pixels[at..at + 4].try_into().expect("four bytes"))
}

#[test]
fn a_picture_the_guest_drew_is_read_back_from_the_display() {
    if !build_is_known() {
        return;
    }
    let mut guest = Guest::new();
    let (width, height) = (1920, 1080);
    let frame = guest::present(&mut guest, width, height).expect("the display has a picture");

    for y in 0..height {
        let wanted = if y < height / 2 { UPPER } else { LOWER };
        for x in 0..width {
            assert_eq!(pixel(&frame, x, y), wanted, "the pixel at {x}, {y}");
        }
    }
    assert_eq!(guest.refusals(), [], "the guard passed all of it");
    // One for each of the three commands, and one from the mapper.
    assert!(guest.interrupts() >= 4, "{} interrupts", guest.interrupts());
    assert_eq!(guest.device.presents().0, 1);
}

#[test]
fn the_early_framebuffer_shows_a_block_of_guest_memory_with_no_ring() {
    if !build_is_known() {
        return;
    }
    let mut guest = Guest::new();
    let (width, height, colour) = (640, 480, 0xFF33_6699);
    let bytes = u64::from(width * height * 4);
    guest.fill(guest::SURFACE, bytes, colour);
    let page = guest::page_number(guest::SURFACE);

    // Off, the bank takes nothing, and the guard says so.
    guest.device.write(guest::EARLY_PAGE, page);
    assert_eq!(guest.refusals().len(), 1);
    guest.device.write(guest::EARLY_ON, 1);
    assert_eq!(guest.device.read(guest::EARLY_MODE_COUNT), 3, "the display's three modes");
    guest.device.write(guest::EARLY_PAGE, page);
    guest.device.write(guest::EARLY_BYTES, bytes as u32);
    guest.device.write(guest::EARLY_DEPTH, 32);
    // A picture larger than the block, and one outside the guest's memory: neither is passed.
    guest.device.write(guest::EARLY_MODE, 768 << 16 | 1024);
    guest.device.write(guest::EARLY_PAGE, guest::page_number(guest::MEMORY_AT + guest::MEMORY_BYTES));
    assert_eq!(guest.refusals().len(), 3);
    assert_eq!(guest.device.read(guest::EARLY_PAGE), page, "the page before stands");
    guest.device.write(guest::EARLY_MODE, height << 16 | width);

    let frame = guest.frame(width, height).expect("the display has a picture");
    assert!(frame.pixels.as_chunks::<4>().0.iter().all(|pixel| u32::from_le_bytes(*pixel) == colour));
    assert_eq!(guest.refusals().len(), 3, "and the driver's own writes all passed");
    guest.device.write(guest::EARLY_ON, 0);
    assert_eq!(guest.device.read(guest::EARLY_PAGE), 0, "off, the bank keeps nothing");
}

// What follows sends the real device what would end this process, were it passed on.

#[test]
fn a_mapper_ring_of_a_broken_length_never_reaches_the_mapper() {
    if !build_is_known() {
        return;
    }
    let mut guest = Guest::new();
    guest.start_mapper();
    guest.device.mapper_write(guest::MAPPER_SLOTS, 0x4010);
    let refusals = guest.refusals();
    assert_eq!(refusals.len(), 1, "{refusals:?}");
    assert_eq!(guest.device.mapper_read(guest::MAPPER_SLOTS), guest::SLOTS, "the length before stands");
    assert!(guest::present(&mut guest, 640, 480).is_some(), "and the device goes on");
}

#[test]
fn too_many_modes_in_the_guest_s_state_page_are_nothing_the_device_reads() {
    if !build_is_known() {
        return;
    }
    let mut guest = Guest::new();
    let modes = [Mode { width: 800, height: 600, hertz: 60 }];
    let count_at = guest::STATE_PAGE + 0x208;
    guest.start_ring();
    assert!(guest.define_kernel_task().stamped);
    assert!(guest.hand_over_state_page().stamped);
    assert_eq!(guest.memory.word(count_at), Some(3), "the device's three modes, copied to the guest's page");
    assert!(guest.command(guest::ACKNOWLEDGE, &[0, 1]).stamped);

    // A device that read this back from the guest's page would end the process at the next
    // acknowledgement and at the next list. This one reads a page of the host's.
    guest.put(count_at, &[129]);
    let (taken, heard) = std::sync::mpsc::channel();
    let done = Box::new(move |was_taken, _took| {
        let _ = taken.send(was_taken);
    });
    guest.device.set_modes(&modes, done).expect("the list is handed over");
    let was_taken = heard.recv_timeout(std::time::Duration::from_secs(5));
    assert_eq!(was_taken, Ok(true), "the framework took the list on the host's queue");
    guest.wait_for(|guest| guest.memory.word(count_at) == Some(1));
    assert_eq!(guest.memory.word(count_at), Some(1), "and the guest's page has the new list's count");
    guest.put(count_at, &[129]);
    assert!(guest.command(guest::ACKNOWLEDGE, &[0, 2]).stamped);
    assert_eq!(guest.refusals(), []);
}

#[test]
fn a_state_page_that_is_not_empty_is_refused_and_the_display_stays_whole() {
    if !build_is_known() {
        return;
    }
    let mut guest = Guest::new();
    guest.start_ring();
    // With every word set and no kernel task, the device itself fails this hand-over half way,
    // and the next one ends the process.
    guest.fill(guest::STATE_PAGE, PAGE_BYTES, u32::MAX);
    let sent = guest.hand_over_state_page();
    assert!(sent.faulted && !sent.stamped, "{sent:?}");
    guest.resume();
    assert!(!guest.device.spoiled());

    guest.fill(guest::STATE_PAGE, PAGE_BYTES, 0);
    assert!(guest.hand_over_state_page().stamped, "the display takes an empty page after");
    guest.device.reset().expect("and the device resets");
    // Dropping the guest lets the display go, which is the other thing that would end the process.
}

#[test]
fn a_cursor_picture_outside_the_kernel_tasks_table_is_refused() {
    if !build_is_known() {
        return;
    }
    let mut guest = Guest::new();
    guest.start_ring();
    assert!(guest.define_kernel_task().stamped);
    assert!(guest.hand_over_state_page().stamped);
    guest.fill(guest::GLYPH, 32 * 32 * 4, 0x0101_0101);
    // The address and the mapped length in 64 bits each, then the stride; width and height; the
    // hot spot; the sum of the picture's words.
    let glyph = |address: u32| {
        [0, 0, address, 0, 4096, 0, 128, 0, 32 | 32 << 16, 0, 0x0101_0101u32.wrapping_mul(1024)]
    };
    let sent = guest.command(guest::CURSOR_GLYPH, &glyph(0));
    assert!(sent.stamped, "a picture the table maps is taken: {sent:?}");
    let sent = guest.command(guest::CURSOR_GLYPH, &glyph(PAGE_BYTES as u32));
    assert!(sent.faulted && !sent.stamped, "{sent:?}");
    assert_eq!(guest.refusals().len(), 1);
}

#[test]
fn a_position_below_the_one_before_never_reaches_the_device() {
    if !build_is_known() {
        return;
    }
    let mut guest = Guest::new();
    guest.start_ring();
    for _ in 0..3 {
        assert!(guest.command(guest::NOTHING, &[]).stamped);
    }
    let interrupts = guest.interrupts();
    guest.device.write(guest::WRITTEN, 12);
    assert_eq!(guest.device.read(0x100C), 36, "the device has read no further, and no command again");
    assert_eq!(guest.interrupts(), interrupts);
    assert_eq!(guest.refusals().len(), 1);
    assert!(guest.command(guest::NOTHING, &[]).stamped, "and the ring goes on");
}

#[test]
fn a_reset_puts_the_device_and_the_mapper_back_to_the_start() {
    if !build_is_known() {
        return;
    }
    let mut guest = Guest::new();
    assert!(guest::present(&mut guest, 640, 480).is_some());
    assert_eq!(guest.device.mapper_read(guest::MAPPER_DONE), 1);

    guest.device.reset().expect("the reset");
    assert_eq!(guest.device.mapper_read(guest::MAPPER_DONE), 0, "the mapper's count starts again");
    assert_eq!(guest.device.read(guest::FAULT), 0);
    // A guest that starts afresh, as after a reboot, finds a device that takes all of it again:
    // the task, the page, and the same mapping id.
    guest.fill(guest::STATE_PAGE, PAGE_BYTES, 0);
    assert!(guest::present(&mut guest, 1024, 768).is_some(), "a picture after the reset");
    assert_eq!(guest.refusals(), []);
}

#[test]
fn a_command_the_host_does_not_pass_is_a_refused_command_to_the_guest() {
    if !build_is_known() {
        return;
    }
    let mut guest = Guest::new();
    guest.start_ring();
    // The command that makes the device wait for ever.
    let sent = guest.command(61, &[0, 0]);
    assert!(sent.faulted && !sent.stamped, "{sent:?}");
    guest.resume();
    assert_eq!(guest.device.read(guest::FAULT), 0);
    assert!(guest.command(guest::NOTHING, &[]).stamped, "and the ring goes on");
}

#[test]
fn a_surface_that_is_no_plain_picture_never_reaches_the_mapper() {
    if !build_is_known() {
        return;
    }
    let mut guest = Guest::new();
    guest.start_mapper();
    // A description with a plane, which a picture of this kind doesn't have.
    let outcome =
        guest.map_surface_with(1, 640, 480, |guest, description| guest.put(description + 0x24, &[1]));
    assert_eq!(outcome, guest::UNSUPPORTED, "the guest finds a command the mapper doesn't support");
    assert_eq!(guest.refusals().len(), 1);
    assert_eq!(guest.map_surface(1, 640, 480), 0, "and the plain picture is mapped after");
}
