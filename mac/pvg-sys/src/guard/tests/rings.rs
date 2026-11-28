// SPDX-License-Identifier: GPL-2.0-or-later
//! The guard's rings: what the copy of a ring may reach, and child rings.

use super::*;

#[test]
fn the_first_ring_s_copy_reaches_no_further_than_a_ring_past_the_device_s_read_position() {
    let mut play = Play::with_ring();
    let nothing = play.profile.commands.nothing.number;
    // A refused command at the start, where the device stops, and ten after it.
    play.queue(99, &[]);
    for _ in 0..10 {
        play.queue(nothing, &[]);
    }
    play.kick().expect("the kick is passed");
    play.why();
    play.read = Some(0);
    let passed = play.written;
    assert_eq!(play.ring_word(12), nothing);

    // A position a whole ring past what was passed would have the copy go over what the device
    // was told of and has not read.
    play.put(RING + u64::from(COMMANDS_AT + 12), &[61, 12, 0]);
    play.put(RING + u64::from(COMMANDS_AT + passed), &[nothing, 0, 0]);
    play.written = passed + ROOM;
    let steps = play.kick().expect("the kick is passed");
    let stopped = steps.last().and_then(|step| step.stopped).map(|refusal| refusal.why);
    assert_eq!(stopped, Some(Why::PositionTooFar));
    assert_eq!(steps.last().map(|step| step.upto), Some(passed));
    assert_eq!(play.ring_word(12), nothing, "what the device has yet to read is what was judged");

    // Up to a ring past the device's read position is the guest's to fill.
    play.written = passed;
    let left = (ROOM - passed - HEADER) / 4;
    play.queue(nothing, &vec![0; left as usize]);
    assert_eq!(play.written, ROOM);
    let steps = play.kick().expect("the kick is passed");
    assert_eq!(steps.last().map(|step| (step.upto, step.stopped)), Some((ROOM, None)));
    assert_eq!(play.ring_word(0), play.profile.commands.refused);
    assert_eq!(play.ring_word(12), nothing);
}

#[test]
fn a_child_ring_s_copy_reaches_no_further_than_a_ring_past_the_device_s_read_position() {
    let mut play = Play::with_ring();
    play.define_ring(1);
    let nothing = play.profile.commands.nothing.number;
    let mut words = vec![99, 12, 0];
    for _ in 0..10 {
        words.extend([nothing, 12, 0]);
    }
    assert_eq!(play.wake(1, 0, &words), Ok(()));
    play.why();
    play.child_read.push((1, 0));
    let passed = words.len() as u32 * 4;
    assert_eq!(play.child_word(1, 12), nothing);

    // The guest's words over what the device has not read, and a position a whole ring on.
    let theirs =
        |at: u32| CHILD_RING + [0u64, 2, 1, 5][(at / 0x1000) as usize] * PAGE_BYTES + u64::from(at % 0x1000);
    play.put(theirs(12), &[61, 12, 0]);
    play.put(theirs(passed), &[nothing, 0, 0]);
    play.put(ROOT_PAGE + 0x400, &[passed + 0x1_0000]);
    assert_eq!(play.wake_written(1), Err(Why::PositionTooFar));
    assert_eq!(play.child_word(1, 12), nothing, "what the device has yet to read is what was judged");
    assert_eq!(play.shadow.memory().word(play.child_descriptor(1)), Some(passed));

    // Once the device has read on, the room it read is the guest's again.
    play.child_read.clear();
    play.put(theirs(passed), &[nothing, 12, 0]);
    play.put(ROOT_PAGE + 0x400, &[passed + 12]);
    assert_eq!(play.wake_written(1), Ok(()));
    assert_eq!(play.refused, []);
}

// Child rings.

#[test]
fn a_child_ring_s_commands_are_copied_and_judged_as_the_first_ring_s() {
    let mut play = Play::with_ring();
    play.define_user_task(1);
    let position = play.written;
    let steps = play.define_ring(1);
    assert_eq!(play.refused, []);
    assert!(matches!(
        steps.last().and_then(|step| step.effect.clone()),
        Some(Effect::DefineRing { id: 1, .. })
    ));
    assert_eq!(play.guest_ring_word(position), play.profile.commands.define_ring.number);

    // The device's descriptor names the host's pages, in a row, and starts empty.
    let ours = play.shadow.memory().clone();
    let descriptor = play.child_descriptor(1);
    let number = |address: u64| (address >> PAGE_SHIFT) as u32;
    assert_eq!(ours.word(descriptor + 16), Some(number(play.shadow.child_list(1))));
    assert_eq!(
        ours.word(play.shadow.child_list(1) + 4),
        Some(number(play.shadow.child_ring(1) + PAGE_BYTES))
    );
    assert_eq!(ours.word(descriptor), Some(0));

    // A command that hands over a buffer, one this host doesn't pass, and one that does nothing.
    let run = play.profile.commands.run_buffers.number;
    let nothing = play.profile.commands.nothing.number;
    let commands = [run, 48, 0, 1, 1, 1, 1, 2, 0x4000, 0, 0x274, 0, 61, 20, 0, 0, 0, nothing, 12, 0];
    assert_eq!(play.wake(1, 0, &commands), Ok(()));
    assert_eq!(play.why(), Some(Why::UnknownCommand));
    assert_eq!(play.child_word(1, 0), run);
    assert_eq!(play.child_word(1, 48), play.profile.commands.refused);
    assert_eq!(play.child_word(1, 68), nothing);
    assert_eq!(ours.word(descriptor), Some(80), "the device is told how far the guest has written");
    // The guest's own pages are as the guest wrote them.
    assert_eq!(play.memory.word(CHILD_RING + 48), Some(61));
}

#[test]
fn a_child_ring_is_copied_across_its_pages_and_its_end() {
    let mut play = Play::with_ring();
    play.define_ring(3);
    let nothing = play.profile.commands.nothing.number;
    // One command of four pages less twelve bytes, so that the next lies across the ring's end.
    let long = 0x1_0000 - 12;
    let mut first = vec![nothing, long, 0];
    first.resize(long as usize / 4, 0x1111_1111);
    assert_eq!(play.wake(3, 0, &first), Ok(()));
    assert_eq!(play.wake(3, long, &[nothing, 20, 0, 0x2222_2222, 0x3333_3333]), Ok(()));
    assert_eq!(play.refused, []);
    assert_eq!(play.child_word(3, 0x4000), 0x1111_1111);
    assert_eq!(play.child_word(3, long + 8), 0);
    // The ring's first page again, after the end.
    assert_eq!(play.child_word(3, 0), 0x2222_2222);
    assert_eq!(play.child_word(3, 4), 0x3333_3333);
    assert_eq!(play.shadow.memory().word(play.child_descriptor(3)), Some(long + 20));
}

#[test]
fn a_child_ring_takes_no_command_that_changes_what_the_guard_keeps() {
    let mut play = Play::with_ring();
    play.define_ring(1);
    play.put(USER_ROOT, &[page(USER_TABLE), 1]);
    let commands = play.profile.commands.clone();
    assert_eq!(
        play.wake(1, 0, &[commands.define_task.number, 28, 0, 2 << 1, TASK_BYTES, 0, page(USER_ROOT)]),
        Ok(())
    );
    assert_eq!(play.why(), Some(Why::NotInAChildRing));
    assert_eq!(play.wake(1, 28, &[commands.state_page.number, 20, 0, 0, page(STATE_PAGE)]), Ok(()));
    assert_eq!(play.why(), Some(Why::NotInAChildRing));
    assert_eq!(play.child_word(1, 28), commands.refused);
    // And a task that was never defined is none in a child ring either.
    assert_eq!(play.wake(1, 48, &[commands.run_buffers.number, 24, 0, 2, 0, 0]), Ok(()));
    assert_eq!(play.why(), Some(Why::NoSuchTask));
}

#[test]
fn a_child_ring_s_position_is_read_once_and_only_goes_on() {
    let mut play = Play::with_ring();
    play.define_ring(1);
    let nothing = play.profile.commands.nothing.number;
    assert_eq!(play.wake(1, 0, &[nothing, 12, 0, nothing, 12, 0]), Ok(()));
    let descriptor = play.child_descriptor(1);
    assert_eq!(play.shadow.memory().word(descriptor), Some(24));
    // Back, and further on than the ring has room for.
    assert_eq!(play.wake(1, 0, &[nothing, 12, 0]), Err(Why::PositionWentBack));
    play.put(ROOT_PAGE + 0x400, &[24 + 0x1_0004]);
    assert_eq!(play.wake_written(1), Err(Why::PositionTooFar));
    assert_eq!(play.shadow.memory().word(descriptor), Some(24), "the device's position stands");
    // A length that is no command's: the device is told of what lies before it.
    assert_eq!(play.wake(1, 24, &[nothing, 12, 0, nothing, 7, 0]), Ok(()));
    assert_eq!(play.why(), Some(Why::BrokenCommand));
    assert_eq!(play.shadow.memory().word(descriptor), Some(36));
}

#[test]
fn a_child_ring_has_an_id_a_cell_and_pages_in_guest_memory_and_starts_empty() {
    let mut play = Play::with_ring();
    let define = play.profile.commands.define_ring.number;
    let wake = play.profile.registers.wake;
    let resume = play.profile.registers.resume_child;
    for id in [0, 64] {
        play.send(define, &[id]);
        assert_eq!(play.why(), Some(Why::RingId));
    }
    assert_eq!(play.write(wake, 1).map_err(|refusal| refusal.why), Err(Why::NoSuchRing));
    assert_eq!(play.write(resume, 1).map_err(|refusal| refusal.why), Err(Why::NoSuchRing));
    assert_eq!(play.write(resume, 0).map_err(|refusal| refusal.why), Err(Why::NoSuchRing));

    let descriptor = ROOT_PAGE + 0x400 + 20;
    play.put(CHILD_LIST, &[page(CHILD_RING); 4]);
    let tries = [
        // Commands that lie before the written position, the first ring's cell, a cell past the
        // last, a bit above the index, a list outside the guest's memory.
        ([12, 0, u32::MAX, 2, page(CHILD_LIST)], Why::ChildRingLayout),
        ([0, 0, u32::MAX, 0, page(CHILD_LIST)], Why::ChildRingLayout),
        ([0, 0, u32::MAX, 64, page(CHILD_LIST)], Why::ChildRingLayout),
        ([0, 0, u32::MAX, 2 | 1 << 16, page(CHILD_LIST)], Why::ChildRingLayout),
        ([0, 0, u32::MAX, 2, page(OUTSIDE)], Why::NotGuestMemory),
    ];
    for (words, why) in tries {
        play.put(descriptor, &words);
        play.send(define, &[2]);
        assert_eq!(play.why(), Some(why), "{words:x?}");
    }
    // A page of the ring outside the guest's memory.
    play.put(descriptor, &[0, 0, u32::MAX, 2, page(CHILD_LIST)]);
    play.put(CHILD_LIST + 8, &[page(OUTSIDE)]);
    play.send(define, &[2]);
    assert_eq!(play.why(), Some(Why::NotGuestMemory));

    play.define_ring(2);
    assert_eq!(play.refused, []);
    play.define_ring(2);
    assert_eq!(play.why(), Some(Why::RingExists));
    assert_eq!(play.write(resume, 2), Ok(Pass::AsItIs));
    // Deleted, it is woken no more, and can be defined again.
    let delete = play.profile.commands.delete_ring.number;
    play.send(delete, &[2]);
    assert_eq!(play.refused, []);
    assert_eq!(play.write(wake, 2).map_err(|refusal| refusal.why), Err(Why::NoSuchRing));
    play.send(delete, &[2]);
    assert_eq!(play.why(), Some(Why::NoSuchRing));
    play.define_ring(2);
    assert_eq!(play.refused, []);
}

#[test]
fn what_the_device_writes_of_a_child_ring_reaches_the_guest_s_descriptor() {
    let mut play = Play::with_ring();
    play.define_ring(5);
    let (ours, theirs) = (play.child_descriptor(5), ROOT_PAGE + 0x400 + 4 * 20);
    let mirror = play.guard.mirror();
    let mut mirror = mirror.lock().expect("the mirror");
    mirror.to_guest();
    assert_eq!(play.memory.word(theirs + 12), Some(5 | 1 << 16), "the bit that says it is defined");
    // The device has read 24 bytes, refused the command at 12, and stamped the ring's cell.
    let device = play.shadow.memory();
    device.set_word(ours + 4, 24);
    device.set_word(ours + 8, 12);
    device.set_word(play.shadow.ring() + 5 * 4, 0x77);
    mirror.to_guest();
    assert_eq!(play.memory.word(theirs + 4), Some(24));
    assert_eq!(play.memory.word(theirs + 8), Some(12));
    assert_eq!(play.memory.word(RING + 5 * 4), Some(0x77));
    assert_eq!(play.memory.word(theirs + 16), Some(page(CHILD_LIST)), "the guest's list stays the guest's");
}

#[test]
fn a_cursor_s_picture_is_read_for_the_host_as_the_command_names_it() {
    let mut play = Play::with_ring();
    play.define_task();
    // Rows of 128 bytes for 32 pixels: the second pixel of the third row.
    play.put(GLYPH + 2 * 128 + 4, &[0xFF11_2233]);
    play.glyph(0, 32 * 128);
    let glyph = play.guard.take_glyph().expect("a picture");
    assert_eq!((glyph.width, glyph.height, glyph.hot_x, glyph.hot_y), (32, 32, 0, 0));
    assert_eq!(glyph.pixels.len(), 32 * 32 * 4);
    assert_eq!(glyph.pixels[(2 * 32 + 1) * 4..][..4], 0xFF11_2233u32.to_le_bytes());
    assert_eq!(play.guard.take_glyph(), None, "taken once");
    // A picture longer than the length mapped is refused, at the row length the command gives.
    let position = play.written;
    play.glyph_rows(0, 31 * 128, 128, 32, 32);
    assert_eq!(play.why(), Some(Why::PictureNotMapped));
    assert_eq!(play.ring_word(position), play.profile.commands.refused);
    assert_eq!(play.guard.take_glyph(), None);
    play.glyph_rows(0, 32 * 128, 256, 32, 32);
    assert_eq!(play.why(), Some(Why::PictureNotMapped));
    play.glyph_rows(0, 32 * 128, u32::MAX, 32, 2);
    assert_eq!(play.why(), Some(Why::PictureNotMapped));
    // Rows with a gap between them that end within it are read, and so is no picture at all.
    play.glyph_rows(0, 31 * 128 + 64, 128, 16, 32);
    assert_eq!(play.why(), None);
    assert_eq!(play.guard.take_glyph().map(|glyph| (glyph.width, glyph.height)), Some((16, 32)));
    play.glyph_rows(0, 16, 0, 0, 0);
    assert_eq!(play.why(), None);
    assert_eq!(play.guard.take_glyph(), None);
}

#[test]
fn the_mapper_is_told_the_host_s_ring_and_table_and_the_guest_reads_its_own() {
    let mut play = Play::new();
    let mapper = play.profile.mapper.clone();
    assert_eq!(play.mapper_told(mapper.ring, MAPPER_RING, 0), Ok(play.shadow.mapper_ring()));
    assert_eq!(play.mapper_told(mapper.slots, mapper.slots_unit, 0), Ok(mapper.slots_unit));
    assert_eq!(play.mapper_told(mapper.table, MAPPER_TABLE, 0), Ok(play.shadow.mapper_table()));
    assert_eq!(play.guard.mapper_reads_as(mapper.ring), Some(MAPPER_RING));
    assert_eq!(play.guard.mapper_reads_as(mapper.table), Some(MAPPER_TABLE));
    assert_eq!(play.guard.mapper_reads_as(mapper.done), None);
    // The host's pages are the mapper's alone: neither the guest's memory nor the device's.
    let host = play.shadow.mapper_memory();
    assert!(!play.memory.holds(play.shadow.mapper_ring(), PAGE_BYTES));
    assert!(!play.shadow.memory().holds(play.shadow.mapper_ring(), PAGE_BYTES));
    assert!(host.holds(play.shadow.mapper_ring(), mapper.most_slots * mapper.slot_bytes));
    assert!(host.holds(play.shadow.page_list(mapper.most_id), PAGE_BYTES));
    // The host's table names the host's pages of descriptions, whatever the guest's names.
    let entry = host.word(play.shadow.mapper_table() + 4 * 255);
    assert_eq!(entry, Some(mapper_page(play.shadow.mapper_descriptions(255))));
}

#[test]
fn what_the_guest_writes_after_the_mapper_s_kick_is_not_what_the_mapper_reads() {
    let mut play = Play::new();
    let shape = play.profile.mapper.clone();
    play.start_mapper();
    play.describe(64, 64);
    assert_eq!(play.mapper_command(0, shape.map, 1), shape.map);
    assert_eq!(play.why(), None);
    let host = play.shadow.mapper_memory().clone();
    let description = play.shadow.mapper_descriptions(0) + 0x200;
    let list = play.shadow.page_list(1);

    // The mapper's copy names the host's page list, which names the guest's surface.
    assert_eq!(host.word(description), Some(mapper_page(list)));
    assert_eq!(host.word(description + 4), Some(u32::from_be_bytes(*b"BGRA")));
    assert_eq!(host.word(description + 0x14), Some(64 << 8 | 1));
    assert_eq!(host.word(list), Some(mapper_page(SURFACE)));
    assert_eq!(host.word(list + 4), Some(0), "64 by 64 is one page");

    // The other processor writes over the description and the list that were judged.
    play.put(DESCRIPTION, &[mapper_page(SURFACE_LIST) & !1]);
    play.put(DESCRIPTION + 0x14, &[16384 << 8 | 1]);
    play.put(SURFACE_LIST, &[mapper_page(OUTSIDE)]);
    play.put(MAPPER_TABLE, &[mapper_page(OUTSIDE)]);
    play.put(MAPPER_RING, &[7]);
    assert_eq!(host.word(description), Some(mapper_page(list)));
    assert_eq!(host.word(description + 0x14), Some(64 << 8 | 1));
    assert_eq!(host.word(list), Some(mapper_page(SURFACE)));
    assert_eq!(host.word(play.shadow.mapper_ring()), Some(shape.map));
    assert_eq!(host.word(play.shadow.mapper_table()), Some(mapper_page(play.shadow.mapper_descriptions(0))));

    // A larger surface after a smaller one: the list is the new one's, and nothing of the old.
    play.describe(1920, 1080);
    play.put(SURFACE_LIST + 4 * 507, &[mapper_page(SURFACE)]);
    assert_eq!(play.mapper_command(1, shape.map, 1), shape.map);
    assert_eq!(host.word(list + 4 * 506), Some(mapper_page(SURFACE + 506 * PAGE_BYTES)));
    assert_eq!(host.word(list + 4 * 507), Some(0), "507 pages, and the guest's next word is not copied");
    play.describe(64, 64);
    assert_eq!(play.mapper_command(2, shape.map, 1), shape.map);
    assert_eq!(host.word(list + 4), Some(0));
}

#[test]
fn the_mapper_s_outcome_reaches_the_guest_s_ring_before_the_guest_hears_it_is_done() {
    let mut play = Play::new();
    let shape = play.profile.mapper.clone();
    play.start_mapper();
    play.describe(64, 64);
    assert_eq!(play.mapper_command(0, shape.map, 1), shape.map);
    assert_eq!(play.mapper_command(1, 2, 1), 2);
    let host = play.shadow.mapper_memory().clone();
    let ring = play.shadow.mapper_ring();
    let mirror = play.guard.mirror();
    let mut mirror = mirror.lock().expect("the mirror");

    // Nothing done yet: the guest's outcome words stay as the guest wrote them.
    mirror.mapper_to_guest(None);
    assert_eq!(play.memory.word(MAPPER_RING + 8), Some(u32::MAX));
    // The mapper has done the first, and its interrupt comes.
    host.set_word(ring + 8, 0);
    mirror.mapper_to_guest(None);
    assert_eq!(play.memory.word(MAPPER_RING + 8), Some(0));
    // The second, and the guest reads the count of commands done.
    host.set_word(ring + 16 + 8, 0xE000_02C2);
    mirror.mapper_to_guest(Some(2));
    assert_eq!(play.memory.word(MAPPER_RING + 16 + 8), Some(0xE000_02C2));
    // Done with, both are forgotten: the guest may use the slots again for its own.
    play.put(MAPPER_RING + 8, &[0x1234]);
    host.set_word(ring + 8, 5);
    mirror.mapper_to_guest(None);
    assert_eq!(play.memory.word(MAPPER_RING + 8), Some(0x1234));
}
