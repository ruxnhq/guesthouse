use super::*;

const BUFFER: GemHandle = GemHandle(7);
const HERE: GemHandle = GemHandle(3);
const AT: u64 = 0x40_0000;
const SIZE: u64 = 0x1_0000;

/// A space with no card behind its file: every call to the kernel on it fails, so a test sees
/// what is refused before the kernel is asked.
#[test]
fn a_fence_no_run_here_writes_has_nothing_to_wait_on() {
    let space = space_without_a_card();
    assert!(space.sync_files_reaching(0x1000, 1).unwrap().is_none());
}

fn space_without_a_card() -> SpaceVm {
    SpaceVm {
        file: File::open("/dev/null").unwrap(),
        books: Books::default(),
        contexts: Contexts::default(),
        faults: fault::Watch::default(),
        fence_ibs: None,
        video_ib: None,
        video_runs: VideoRuns::default(),
        given_up: Vec::new(),
        own_on_card: None,
        on_card: VecDeque::new(),
        retired: Vec::new(),
        video_retired: Vec::new(),
        video_last_retired: None,
        video_wake_failed: false,
        shares: HashMap::new(),
        keeps_finished: false,
        finished: Vec::new(),
        clock_khz: 100_000,
        lists: BufferList::default(),
    }
}

#[test]
fn the_daemons_own_run_is_asked_about_and_let_go_at_its_end() {
    let mut space = space_without_a_card();
    assert_eq!(space.oldest_on_card(), None);
    space.own_on_card = Some((5, Syncobj::made_up(1)));
    // This file has no kernel behind it, which has no run for the sync object.
    assert_eq!(space.oldest_on_card(), None, "asked, not kept until the next copy");
    let _ = space.end_own(Syncobj::made_up(1), true);
    assert!(space.own_on_card.is_none());
    assert!(space.given_up.is_empty());
}

#[test]
fn a_run_given_up_on_stays_until_the_kernel_lets_it_go() {
    let mut space = space_without_a_card();
    space.own_on_card = Some((9, Syncobj::made_up(1)));
    let _ = space.end_own(Syncobj::made_up(1), false);
    assert!(space.own_on_card.is_none());
    assert_eq!(space.given_up.len(), 1, "kept, as the card may still be running it");
    // This file has no kernel behind it, which has no run for the sync object.
    assert_eq!(space.oldest_on_card(), None);
    assert!(space.given_up.is_empty());
}

#[test]
fn runs_are_numbered_in_the_order_they_go() {
    let first = next_run();
    let second = next_run();
    assert!(second > first);
    assert!(runs_sent() >= second);
}

/// A command buffer where a guest places its own, and one inside the daemon's video buffer.
const GUESTS: Ib = Ib { address: 0x4_2000_1000, bytes: 0x40 };
const COPY: Ib = Ib { address: video_ib::ADDRESS + 256, bytes: 0x40 };

#[test]
fn a_run_notes_the_clock_first_and_again_after_its_fence() {
    let fence_writer = Ib { address: crate::fence_ib::ADDRESS, bytes: 32 };
    let [start, end] = FenceIbs::stamp_ibs(3);
    let stamped = with_clock_notes(&[GUESTS, COPY, fence_writer], 3);
    assert_eq!(stamped, [start, GUESTS, COPY, fence_writer, end]);
    assert_ne!(start, end);
}

#[test]
fn the_cards_ticks_become_nanoseconds_at_its_own_rate() {
    assert_eq!(ticks_to_ns(100_000, 100_000), 1_000_000);
    assert_eq!(ticks_to_ns(27_000, 27_000), 1_000_000);
}

#[test]
fn the_video_ring_takes_only_command_buffers_in_the_daemons_video_buffer() {
    use crate::info::{HW_IP_COMPUTE, HW_IP_DMA, HW_IP_GFX};
    let refused = |ibs: &[Ib]| only_copies_for_the_video_ring(HW_IP_VCN_ENC, ibs).unwrap_err().to_string();
    assert_eq!(refused(&[GUESTS]), NOT_A_COPY);
    assert_eq!(refused(&[COPY, GUESTS]), NOT_A_COPY, "one guest buffer among copies");
    let end = video_ib::ADDRESS + (video_ib::SLOTS * video_ib::BYTES) as u64;
    let past_the_end = Ib { address: end - 4, bytes: 8 };
    assert_eq!(refused(&[past_the_end]), NOT_A_COPY);
    assert!(only_copies_for_the_video_ring(HW_IP_VCN_ENC, &[COPY, COPY]).is_ok());
    // The other engines run a guest's buffers where they lie, as before.
    for ip_type in [HW_IP_GFX, HW_IP_COMPUTE, HW_IP_DMA] {
        assert!(only_copies_for_the_video_ring(ip_type, &[GUESTS]).is_ok());
    }
}

#[test]
fn no_way_of_sending_work_gives_the_video_ring_a_guests_buffer() {
    let mut space = space_without_a_card();
    let waited = space.run(HW_IP_VCN_ENC, Some(1), &[GUESTS], Patience::RUN);
    assert_eq!(waited.unwrap_err().to_string(), NOT_A_COPY);
    let fence = GuestFence { queue: 1, address: 0x1000, value: 1 };
    let sent = space.send(HW_IP_VCN_ENC, &[GUESTS, COPY], 0, fence);
    assert_eq!(sent.unwrap_err().to_string(), NOT_A_COPY);
    let sent = space.send_video(0, &[COPY, GUESTS], 1, Some(fence), None, Instant::now());
    assert_eq!(sent.unwrap_err().to_string(), NOT_A_COPY);
    // Refused before the kernel was asked anything, so nothing is on the card.
    assert!(!space.video_on_card());
}

#[test]
fn a_space_that_faulted_before_sends_nothing_and_says_the_fault_was_earlier() {
    use crate::info::HW_IP_DMA;
    let earlier = Fault { address: 0x1000, status: 0xf4_3a51 };
    let mut space = space_without_a_card();
    space.faults = fault::Watch::having_found(earlier);
    // The file has no card behind it, so a run that got as far as the kernel would fail.
    let ran = space.run(HW_IP_DMA, Some(1), &[GUESTS], Patience::RUN).unwrap();
    assert_eq!(ran, Ran::Stopped(earlier));
    // Video runs aren't waited for: one is refused as not sent, with the earlier fault.
    let sent = space.send_video(0, &[COPY], 1, None, None, Instant::now());
    assert_eq!(
        sent.unwrap_err().to_string(),
        "work here reached 0x1000 earlier, where nothing is mapped (fault status 0xf43a51), so nothing \
         more is sent"
    );
    assert!(!space.video_on_card());
}

#[test]
fn video_commands_are_sent_only_from_a_buffer_that_was_made() {
    let mut copied = Copied::default();
    copied.place(8, 256).unwrap().copy_from_slice(&[0x8, 0x1]);
    let mut space = space_without_a_card();
    let failed = space.place_video(0x1000, &copied).unwrap_err().to_string();
    assert!(failed.starts_with("making the buffer for video commands: "), "{failed}");
    let sent = space.send_video(0, &copied.runs_in(0), 1, None, None, Instant::now());
    assert_eq!(sent.unwrap_err().to_string(), "no buffer for video commands was made here");
    assert!(!space.video_on_card());
    assert!(space.take_retired_video(Instant::now()).is_empty());
}

#[test]
fn a_buffer_mapped_twice_is_let_go_after_its_second_mapping() {
    let mut books = Books::default();
    books.add(BUFFER, HERE, AT, 0, SIZE);
    books.add(BUFFER, HERE, AT + SIZE, SIZE, SIZE);
    assert_eq!(books.handle_of(BUFFER), Some(HERE));
    let first = books.remove(AT).unwrap();
    assert_eq!(first, Unmapping { handle: HERE, address: AT, length: SIZE, last_use: false });
    assert!(books.remove(AT + SIZE).unwrap().last_use);
    assert_eq!(books.handle_of(BUFFER), None);
}

#[test]
fn an_address_with_nothing_mapped_has_nothing_to_take_back() {
    let mut books = Books::default();
    assert_eq!(books.remove(AT), None);
}

#[test]
fn a_buffers_mappings_are_found_by_the_buffer() {
    let mut books = Books::default();
    books.add(BUFFER, HERE, AT, 0, SIZE);
    books.add(GemHandle(8), GemHandle(4), AT + SIZE, 0, SIZE);
    assert_eq!(books.addresses_of(BUFFER), vec![AT]);
}

#[test]
fn a_buffer_mapped_twice_is_listed_once() {
    let mut books = Books::default();
    books.add(BUFFER, HERE, AT, 0, SIZE);
    books.add(BUFFER, HERE, AT + SIZE, SIZE, SIZE);
    books.add(GemHandle(8), GemHandle(4), AT + 2 * SIZE, 0, SIZE);
    let mut handles = books.handles();
    handles.sort_by_key(|handle| handle.0);
    assert_eq!(handles, vec![HERE, GemHandle(4)]);
}

#[test]
fn an_address_is_found_in_its_buffer_by_the_mapping_s_offset() {
    let mut books = Books::default();
    books.add(BUFFER, HERE, AT, 0x3000, SIZE);
    assert_eq!(books.locate(AT), Some((BUFFER, 0x3000)));
    assert_eq!(books.locate(AT + 0x10), Some((BUFFER, 0x3010)));
    assert_eq!(books.locate(AT + SIZE), None);
    assert_eq!(books.locate(AT - 1), None);
}

#[test]
fn a_range_is_covered_across_mappings_that_follow_one_another() {
    let mut books = Books::default();
    books.add(BUFFER, HERE, AT, 0, SIZE);
    books.add(GemHandle(8), GemHandle(4), AT + SIZE, 0, SIZE);
    books.add(GemHandle(9), GemHandle(5), AT + 3 * SIZE, 0, SIZE);
    assert!(books.covers(AT, SIZE));
    assert!(books.covers(AT + 0x100, 2 * SIZE - 0x100));
    assert!(books.covers(AT, 0));
    assert!(!books.covers(AT, 2 * SIZE + 1));
    assert!(!books.covers(AT + SIZE, 3 * SIZE));
    assert!(!books.covers(AT - 1, 2));
    assert!(!books.covers(AT + 2 * SIZE, 1));
    assert!(!books.covers(u64::MAX - 1, 4));
}

#[test]
fn a_range_is_held_only_inside_one_mapping() {
    let mut books = Books::default();
    books.add(BUFFER, HERE, AT, 0, SIZE);
    assert!(books.holds(AT, SIZE));
    assert!(books.holds(AT + 0x100, 0x100));
    assert!(!books.holds(AT + 0x100, SIZE));
    assert!(!books.holds(AT - 0x100, 0x200));
    assert!(!books.holds(AT + SIZE, 4));
    assert!(!books.holds(u64::MAX - 1, 4));
}
