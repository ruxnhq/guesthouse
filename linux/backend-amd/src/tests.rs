use super::*;
use guesthouse_backend::{
    CardLabel, DriverVersion, GuestSpan, MemoryError, SharedSpan, StartDecision, decide_start,
};

/// Guest memory that holds only fences, by guest physical address.
#[derive(Default)]
struct Fences(HashMap<u64, u64>);

impl HostServices for Fences {
    fn take_gpu_addresses(&mut self, _size: u64, _align: u64) -> Result<u64, BackendError> {
        unreachable!()
    }
    fn read_at_gpu_address(&self, _space: SpaceId, _address: u64, _out: &mut [u8]) -> Option<usize> {
        unreachable!()
    }
    fn shared_span(&self, _space: SpaceId, _address: u64) -> Option<SharedSpan> {
        unreachable!()
    }
    fn guest_span(&self, _space: SpaceId, _address: u64) -> Option<GuestSpan> {
        unreachable!()
    }
    fn read_guest_span(&self, _space: SpaceId, _address: u64, _out: &mut [u8]) -> Option<usize> {
        unreachable!()
    }
    fn write_guest_span(&mut self, _space: SpaceId, _address: u64, _value: u32) -> bool {
        unreachable!()
    }
    fn write_u32(&mut self, _address: u64, _value: u32) -> Result<(), MemoryError> {
        unreachable!()
    }
    fn write_u64(&mut self, address: u64, value: u64) -> Result<(), MemoryError> {
        self.0.insert(address, value);
        Ok(())
    }
    fn read_guest(&self, _address: u64, _out: &mut [u8]) -> usize {
        unreachable!()
    }
    fn fence_value(&self, fence: u64) -> Result<u64, MemoryError> {
        Ok(self.0.get(&fence).copied().unwrap_or(0))
    }
}

#[test]
fn a_wait_for_a_copy_is_never_cut_short_by_rounding() {
    assert_eq!(whole_millis(Duration::from_micros(300)), 1);
    assert_eq!(whole_millis(Duration::from_millis(50)), 50);
    assert_eq!(whole_millis(Duration::ZERO), 0);
}

#[test]
fn a_run_that_ended_before_writing_its_fence_gets_it_written_by_the_host() {
    const WRITTEN: u64 = 0x1000;
    const LOST: u64 = 0x2000;
    let mut host = Fences::default();
    host.0.insert(WRITTEN, 7);
    host.0.insert(LOST, 3);
    let retired = vec![
        GuestFence { queue: 1, address: WRITTEN, value: 7 },
        GuestFence { queue: 2, address: LOST, value: 4 },
    ];
    // Only the lost run is named, as that is the one whose command buffers are kept.
    let lost = stand_in_for_lost_runs(&mut host, retired);
    assert_eq!(lost, vec![GuestFence { queue: 2, address: LOST, value: 4 }]);
    assert_eq!(host.0[&WRITTEN], 7);
    assert_eq!(host.0[&LOST], 4);
}

#[test]
fn the_host_writes_the_fence_of_each_retired_video_run_and_leaves_one_already_past() {
    use video_runs::Retired;
    const FIRST: u64 = 0x1000;
    const PAST: u64 = 0x2000;
    let mut host = Fences::default();
    host.0.insert(FIRST, 2);
    host.0.insert(PAST, 9);
    let took = Duration::from_micros(1900);
    let retired = [
        Retired { queue: 1, fence: Some(GuestFence { queue: 1, address: FIRST, value: 3 }), took },
        // The core gave up on this one and wrote a later value.
        Retired { queue: 2, fence: Some(GuestFence { queue: 2, address: PAST, value: 8 }), took },
        // The daemon's own closing, which has no fence.
        Retired { queue: 1, fence: None, took },
    ];
    write_retired_video_fences(&mut host, &retired);
    assert_eq!(host.0[&FIRST], 3);
    assert_eq!(host.0[&PAST], 9);
    assert_eq!(host.0.len(), 2, "nothing else was written");
}

#[test]
fn the_video_wake_is_emptied_by_one_drain_and_wakes_again_for_the_next_post() {
    let readable = |event: &OwnedFd| {
        let mut poll = libc::pollfd { fd: event.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // SAFETY: one pollfd, valid for the call, and no wait.
        unsafe { libc::poll(&mut poll, 1, 0) == 1 }
    };
    let post = |event: &OwnedFd| {
        let one = 1u64.to_ne_bytes();
        // SAFETY: eight bytes, as an eventfd's write wants.
        assert_eq!(unsafe { libc::write(event.as_raw_fd(), one.as_ptr().cast(), 8) }, 8);
    };
    let event = make_wake().unwrap();
    assert!(!readable(&event));
    drain_wake(&event);
    // Several runs that retire before the daemon looks are one wake.
    post(&event);
    post(&event);
    assert!(readable(&event));
    drain_wake(&event);
    assert!(!readable(&event), "the daemon sleeps again");
    post(&event);
    assert!(readable(&event));
}

#[test]
fn a_fence_already_past_a_lost_run_is_left_alone() {
    const FENCE: u64 = 0x1000;
    let mut host = Fences::default();
    host.0.insert(FENCE, 9);
    let lost = stand_in_for_lost_runs(&mut host, vec![GuestFence { queue: 1, address: FENCE, value: 8 }]);
    assert!(lost.is_empty());
    assert_eq!(host.0[&FENCE], 9);
}

fn setup_for(version: &str) -> Setup {
    Setup {
        guest_driver_version: DriverVersion::parse(version).unwrap(),
        guest_branch: version.into(),
        driver_folder: Default::default(),
        card: None,
        card_address: None,
    }
}

#[test]
fn these_guest_drivers_may_start_and_another_is_refused() {
    for release in protocol::guest_driver::GUEST_RELEASES {
        let survey = AmdBackend::survey(&setup_for(release)).unwrap();
        assert_eq!(decide_start(&survey), StartDecision::Run);
        assert_eq!(survey.card, CardLabel::Experimental);
    }
    let survey = AmdBackend::survey(&setup_for("1.0.0.0")).unwrap();
    assert!(matches!(decide_start(&survey), StartDecision::Refuse(_)));
}

/// Guest pages that hold command buffers, by GPU address: each dword is its own address.
fn read_from_guest_pages(readable: std::ops::Range<u64>) -> impl FnMut(&Ib, &mut [u32]) -> Option<()> {
    move |ib, room| {
        let end = ib.address + u64::from(ib.bytes);
        (readable.start <= ib.address && end <= readable.end).then(|| {
            for (index, word) in room.iter_mut().enumerate() {
                *word = (ib.address + 4 * index as u64) as u32;
            }
        })
    }
}

#[test]
fn video_commands_are_copied_out_of_guest_pages_and_run_from_the_daemons_buffer() {
    const START: u32 = 256;
    let guests = [Ib { address: 0x4_2000_1000, bytes: 0xC }, Ib { address: 0x4_2000_3000, bytes: 0x8 }];
    let mut copied = video_ib::Copied::default();
    copy_video_commands(&guests, START, &mut copied, read_from_guest_pages(0..u64::MAX)).unwrap();
    // No run is at an address the guest gave, and each holds what the guest's buffer held.
    let runs =
        [Ib { address: video_ib::ADDRESS, bytes: 0xC }, Ib { address: video_ib::ADDRESS + 256, bytes: 0x8 }];
    assert_eq!(copied.runs(), runs);
    let held: Vec<&[u32]> = copied.buffers().collect();
    assert_eq!(held, [&[0x2000_1000, 0x2000_1004, 0x2000_1008][..], &[0x2000_3000, 0x2000_3004][..]]);

    // What an earlier submission left is gone before the next is copied.
    copy_video_commands(&guests[1..], START, &mut copied, read_from_guest_pages(0..u64::MAX)).unwrap();
    assert_eq!(copied.runs(), [Ib { address: video_ib::ADDRESS, bytes: 0x8 }]);
}

#[test]
fn video_commands_the_daemon_cannot_copy_are_not_run() {
    const START: u32 = 256;
    let mut copied = video_ib::Copied::default();
    let failed = |guests: &[Ib], copied: &mut video_ib::Copied| {
        let read = read_from_guest_pages(0x1000..0x2000);
        copy_video_commands(guests, START, copied, read).unwrap_err().to_string()
    };
    // In video memory, say, where the CPU doesn't read them.
    assert_eq!(
        failed(&[Ib { address: 0x1000, bytes: 8 }, Ib { address: 0x8000, bytes: 8 }], &mut copied),
        "its commands at 0x8000 for 0x8 bytes are not in guest pages the daemon can read"
    );
    assert_eq!(
        failed(&[Ib { address: 0x1000, bytes: 6 }], &mut copied),
        "its commands at 0x1000 for 0x6 bytes are not a whole number of dwords"
    );
    let long = video_ib::BYTES as u32 + 4;
    assert_eq!(
        failed(&[Ib { address: 0x1000, bytes: long }], &mut copied),
        "its commands at 0x1000 for 0x10004 bytes are more than the daemon's buffer holds"
    );
}

#[test]
fn address_spaces_stay_inside_the_cards_range() {
    let ranges = [GpuRange { start: 0x400_0000, end: 0x1_0000_0000 }];
    assert!(ranges_fit(&ranges, 0x1_0000, 0x8000_0000_0000));
    assert!(!ranges_fit(&ranges, 0x800_0000, 0x8000_0000_0000));
    assert!(!ranges_fit(&ranges, 0x1_0000, 0x8000_0000));
    assert!(ranges_fit(&[], 0, 0));
}
