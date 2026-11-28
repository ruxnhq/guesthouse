//! What a context's submission has written once its work is done: the monitored fence its header
//! names, which the guest waits on, then its tag, then the core's fence for the context's queue.
//!
//! The guest's batch stores the tag itself, at its end. The monitored fence is left to the kernel
//! driver. Here the card writes it when it can, as a run's one fence of its own, and the host
//! writes the rest once the run has ended. A submission that runs nothing has them written at once,
//! or after the runs before it on its queue, so they always come in order.
//!
//! Both come from the guest, and only a compact header's are read. A monitored fence is written
//! only while the process holds a sync object with it, and never lowered. A tag is written only
//! inside a mapping of the process's own space, and never lowered either, so the host can't step
//! back one a later batch has stored.

use super::CardWork;
use crate::protocol::command_header::{Header, Length};
use crate::xe::plan::Backing;
use guesthouse_backend::{HostServices, SpaceId, Work};
use std::fmt;

/// A monitored fence a context's submission brings to a value: where the guest's driver put it, and
/// where the core keeps it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Monitored {
    pub gpu_address: u64,
    pub guest_address: u64,
    pub value: u64,
}

/// See the module documentation. Each is `None` when there is nothing to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct Marks {
    pub monitored: Option<Monitored>,
    /// The tag's GPU address and value.
    pub tag: Option<(u64, u32)>,
    /// The core's fence for the context's queue, by guest physical address, and its value.
    pub queue_fence: Option<(u64, u64)>,
}

impl Marks {
    /// The marks a context's submission names in its header. The queue's fence is left out, as the
    /// core writes it for work finished unrun.
    pub fn of(host: &dyn HostServices, work: &Work<'_>, header: &Header) -> Self {
        let compact = header.compact.as_ref().filter(|_| header.verdict == Length::Compact);
        let named = compact.and_then(|compact| compact.monitor_fence);
        let monitored = named.and_then(|(gpu_address, value)| {
            let found = host.monitored_fence(work.space, gpu_address);
            if found.is_none() {
                log::warn!(
                    "intel work: queue {:#x} names a monitored fence at {gpu_address:#x} that its process \
                     holds no sync object for, so it is not written",
                    work.queue
                );
            }
            found.map(|guest_address| Monitored { gpu_address, guest_address, value })
        });
        Self { monitored, tag: compact.and_then(|compact| compact.tag), queue_fence: None }
    }
}

/// What became of a mark the host was to raise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Raised {
    Now,
    /// It was there already, at this value.
    Already(u64),
    /// Its process no longer holds it, or nothing maps it.
    Gone,
    Failed,
}

impl fmt::Display for Raised {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Now => f.write_str("written"),
            Self::Already(value) => write!(f, "already-{value:#x}"),
            Self::Gone => f.write_str("gone"),
            Self::Failed => f.write_str("unwritable"),
        }
    }
}

/// Brings a fence up to a value, and never down.
fn raise(host: &mut dyn HostServices, fence: u64, value: u64) -> Raised {
    match host.fence_value(fence) {
        Ok(reached) if reached >= value => Raised::Already(reached),
        Ok(_) if host.write_u64(fence, value).is_ok() => Raised::Now,
        _ => Raised::Failed,
    }
}

/// Brings a monitored fence up to its value, if the space's process still holds it. A fence page
/// can pass to another sync object once its own has gone.
fn raise_monitored(host: &mut dyn HostServices, space: SpaceId, monitored: &Monitored) -> Raised {
    if host.monitored_fence(space, monitored.gpu_address) != Some(monitored.guest_address) {
        return Raised::Gone;
    }
    raise(host, monitored.guest_address, monitored.value)
}

/// A mark for the log: its address, its value and what became of it, or `-`.
struct Said(Option<(u64, u64, Raised)>);

impl fmt::Display for Said {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Some((address, value, what)) => write!(f, "{address:#x}:{value:#x}:{what}"),
            None => f.write_str("-"),
        }
    }
}

/// Writes a context's monitored fence and a tag in guest pages, with no card to run anything.
pub fn write_marks_without_card(host: &mut dyn HostServices, work: &Work<'_>, header: &Header) {
    let marks = Marks::of(host, work, header);
    let monitored = marks.monitored.map(|monitored| {
        (monitored.gpu_address, monitored.value, raise_monitored(host, work.space, &monitored))
    });
    let tag = marks.tag.map(|(address, value)| {
        let mut bytes = [0; 4];
        let read = host.read_guest_span(work.space, address, &mut bytes) == Some(4);
        let raised = match u32::from_le_bytes(bytes) {
            _ if !read => Raised::Gone,
            held if held >= value => Raised::Already(u64::from(held)),
            _ if host.write_guest_span(work.space, address, value) => Raised::Now,
            _ => Raised::Failed,
        };
        (address, u64::from(value), raised)
    });
    log::debug!(
        "intel work: queue {:#x} marks at once, with no card: monitored_fence={} tag={} queue_fence=-",
        work.queue,
        Said(monitored),
        Said(tag)
    );
}

impl CardWork {
    /// Writes a context's marks, in order. `when` says why now, for the log.
    pub(super) fn write_marks(
        &mut self,
        host: &mut dyn HostServices,
        space: SpaceId,
        queue: u32,
        marks: &Marks,
        when: &str,
    ) {
        let monitored = marks.monitored.map(|monitored| {
            (monitored.gpu_address, monitored.value, raise_monitored(host, space, &monitored))
        });
        let tag = marks
            .tag
            .map(|(address, value)| (address, u64::from(value), self.raise_tag(host, space, address, value)));
        let queue_fence = marks.queue_fence.map(|(fence, value)| (fence, value, raise(host, fence, value)));
        log::debug!(
            "intel work: queue {queue:#x} marks {when}: monitored_fence={} tag={} queue_fence={}",
            Said(monitored),
            Said(tag),
            Said(queue_fence)
        );
    }

    /// Brings a tag up to a value where the process's space maps its address: a buffer through the
    /// CPU's view of it, guest pages through the core.
    fn raise_tag(&mut self, host: &mut dyn HostServices, space: SpaceId, address: u64, value: u32) -> Raised {
        let Some(held) = self.spaces.get(&space) else { return Raised::Gone };
        let Some((memory, offset)) = held.holding(address, 4) else { return Raised::Gone };
        let in_buffer =
            matches!(self.blocks.get(&memory).map(|block| block.backing), Some(Backing::Buffer { .. }));
        let mut bytes = [0; 4];
        let read = if in_buffer {
            self.cpu_view(memory).is_ok_and(|view| view.read(offset, &mut bytes))
        } else {
            host.read_guest_span(space, address, &mut bytes) == Some(4)
        };
        let held = u32::from_le_bytes(bytes);
        if !read {
            return Raised::Failed;
        }
        if held >= value {
            return Raised::Already(u64::from(held));
        }
        let written = if in_buffer {
            self.cpu_view(memory).is_ok_and(|view| view.write(offset, &value.to_le_bytes()))
        } else {
            host.write_guest_span(space, address, value)
        };
        if written { Raised::Now } else { Raised::Failed }
    }

    /// Writes the marks of runs that will never end, which nothing else will.
    pub(super) fn write_left_marks(&mut self, host: &mut dyn HostServices) {
        for (space, queue, marks, when) in std::mem::take(&mut self.left_marks) {
            self.write_marks(host, space, queue, &marks, when);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::stage::Stage;
    use crate::work::testing::{Rig, WorkBuilder};
    use crate::xe::abi::Sync;
    use crate::xe::batch;
    use crate::xe::fake::Call;
    use guesthouse_backend::{Doorbell, WorkStart};

    /// Where a tag can be: in guest pages, which the core writes, or in video memory.
    const TAG_IN_DATA: u64 = 0x80;
    const TAG_IN_BATCH: u64 = 0x800;

    /// A rig whose batch ends at once, whose process holds the monitored fence.
    fn ready() -> Rig {
        let mut rig = Rig::new(Stage::Everything);
        rig.write_batch(&[batch::BATCH_END]);
        let (gpu_address, guest_address) = rig.monitored_addresses();
        rig.host.hold_monitored(rig.space, gpu_address, guest_address);
        rig
    }

    /// A context's submission on queue 1 that brings its queue's fence to `value`, the monitored
    /// fence to ten times that and the tag at `tag` to a hundred times.
    fn context_work(rig: &Rig, value: u64, tag: u64) -> WorkBuilder {
        let monitored = (rig.monitored_addresses().0, value * 10);
        rig.submission(1, value).sent_to_context((tag, value as u32 * 100), monitored)
    }

    fn unrun(mut work: WorkBuilder) -> WorkBuilder {
        work.commands = None;
        work
    }

    fn started(rig: &mut Rig, work: &WorkBuilder) -> u64 {
        let start = rig.start(work);
        let WorkStart::Started { doorbell, .. } = start else { panic!("{start:?}") };
        doorbell
    }

    fn host_wrote(rig: &Rig, fence: u64) -> Vec<u64> {
        rig.host.written.iter().filter(|(at, _)| *at == fence).map(|&(_, value)| value).collect()
    }

    #[test]
    fn a_context_submission_that_runs_nothing_has_its_marks_written_at_once() {
        let mut rig = ready();
        let work = unrun(context_work(&rig, 3, rig.data_address() + TAG_IN_DATA));
        assert_eq!(rig.start(&work), WorkStart::Finished);
        assert_eq!(rig.monitored(), 30);
        assert_eq!(rig.data_dword(TAG_IN_DATA as usize), 300);
        assert_eq!(rig.fence(), 0, "the queue's fence is the core's to write");
        assert!(rig.xe.calls_named("exec").is_empty());
    }

    #[test]
    fn only_a_monitored_fence_the_process_holds_is_written_and_never_lowered() {
        let mut rig = Rig::new(Stage::Everything);
        let work = unrun(context_work(&rig, 3, rig.data_address() + TAG_IN_DATA));
        assert_eq!(rig.start(&work), WorkStart::Finished);
        assert_eq!(rig.monitored(), 0, "not held");
        assert_eq!(rig.data_dword(TAG_IN_DATA as usize), 300, "the tag is still written");
        let (gpu_address, guest_address) = rig.monitored_addresses();
        rig.host.hold_monitored(rig.space, gpu_address, guest_address);
        rig.set_monitored(50);
        rig.start(&work);
        assert_eq!(rig.monitored(), 50);
        assert!(rig.host.written.is_empty());
    }

    #[test]
    fn a_tag_outside_every_mapping_is_left_alone() {
        let mut rig = ready();
        let work = unrun(context_work(&rig, 3, 0x7_7000_0000));
        assert_eq!(rig.start(&work), WorkStart::Finished);
        assert_eq!(rig.monitored(), 30, "the fence is still written");
    }

    #[test]
    fn a_context_run_has_the_card_write_its_monitored_fence_and_the_host_the_rest_once_it_ends() {
        let mut rig = ready();
        rig.xe.hold_runs();
        let work = context_work(&rig, 1, rig.batch_address() + TAG_IN_BATCH);
        let doorbell = started(&mut rig, &work);
        assert!(rig.work.sends(doorbell));
        assert_eq!(rig.work.ring_doorbell(doorbell).unwrap(), Doorbell::Rung);
        let Some(Call::Exec { syncs, .. }) = rig.xe.calls_named("exec").pop() else { panic!() };
        let user_fences: Vec<&Sync> =
            syncs.iter().filter(|sync| matches!(sync, Sync::UserFence { .. })).collect();
        assert_eq!(user_fences, [&Sync::UserFence { address: rig.monitored_addresses().0, value: 10 }]);
        rig.poll();
        assert_eq!(
            (rig.monitored(), rig.fence(), rig.batch_dword(TAG_IN_BATCH)),
            (0, 0, 0),
            "not before it ends"
        );
        rig.xe.release_runs();
        assert_eq!((rig.monitored(), rig.fence()), (10, 0), "the card wrote the monitored fence");
        rig.poll();
        assert_eq!((rig.fence(), rig.batch_dword(TAG_IN_BATCH)), (1, 100));
        assert!(host_wrote(&rig, rig.monitored_addresses().1).is_empty());
        assert!(!rig.work.space_failed(rig.space));
    }

    #[test]
    fn work_that_runs_nothing_behind_a_run_waits_for_it_so_the_marks_come_in_order() {
        let mut rig = ready();
        rig.xe.hold_runs();
        let tag = rig.data_address() + TAG_IN_DATA;
        let work = context_work(&rig, 1, tag);
        let first = started(&mut rig, &work);
        rig.work.ring_doorbell(first).unwrap();
        let work = unrun(context_work(&rig, 2, tag));
        let second = started(&mut rig, &work);
        assert!(!rig.work.sends(second));
        assert_eq!(rig.work.ring_doorbell(second).unwrap(), Doorbell::Rung);
        assert_eq!(rig.xe.calls_named("exec").len(), 1, "it sends nothing");
        rig.poll();
        assert_eq!((rig.monitored(), rig.data_dword(tag as usize - 0x400_0000), rig.fence()), (0, 0, 0));
        rig.xe.release_runs();
        rig.poll();
        assert_eq!((rig.monitored(), rig.data_dword(TAG_IN_DATA as usize), rig.fence()), (20, 200, 2));
        assert_eq!(host_wrote(&rig, rig.fence_guest_address()), [1, 2], "the queue's fence, in order");
        assert_eq!(host_wrote(&rig, rig.monitored_addresses().1), [20], "the first was the card's");
    }

    #[test]
    fn work_that_may_not_run_is_dropped_with_its_marks_written_at_once_even_behind_a_run() {
        let mut rig = ready();
        rig.xe.hold_runs();
        let tag = rig.data_address() + TAG_IN_DATA;
        let work = context_work(&rig, 1, tag);
        let first = started(&mut rig, &work);
        rig.work.ring_doorbell(first).unwrap();
        let mut dropped = context_work(&rig, 2, tag);
        dropped.queue_failed = true;
        assert_eq!(rig.start(&dropped), WorkStart::Finished);
        assert_eq!((rig.monitored(), rig.data_dword(TAG_IN_DATA as usize)), (20, 200));
    }

    #[test]
    fn a_monitored_fence_the_card_cant_reach_is_written_by_the_host_after_the_run() {
        let mut rig = Rig::new(Stage::Everything);
        rig.write_batch(&[batch::BATCH_END]);
        const UNMAPPED: u64 = 0x9_0000_0000;
        rig.host.hold_monitored(rig.space, UNMAPPED, rig.monitored_addresses().1);
        let work =
            rig.submission(1, 1).sent_to_context((rig.data_address() + TAG_IN_DATA, 100), (UNMAPPED, 10));
        let doorbell = started(&mut rig, &work);
        rig.work.ring_doorbell(doorbell).unwrap();
        let Some(Call::Exec { syncs, .. }) = rig.xe.calls_named("exec").pop() else { panic!() };
        assert!(!syncs.iter().any(|sync| matches!(sync, Sync::UserFence { .. })));
        rig.poll();
        assert_eq!((rig.monitored(), rig.data_dword(TAG_IN_DATA as usize), rig.fence()), (10, 100, 1));
        assert!(!rig.work.space_failed(rig.space));
    }

    #[test]
    fn the_card_is_never_asked_to_lower_a_monitored_fence() {
        let mut rig = ready();
        rig.set_monitored(50);
        let work = context_work(&rig, 1, rig.data_address() + TAG_IN_DATA);
        let doorbell = started(&mut rig, &work);
        rig.work.ring_doorbell(doorbell).unwrap();
        let Some(Call::Exec { syncs, .. }) = rig.xe.calls_named("exec").pop() else { panic!() };
        assert!(!syncs.iter().any(|sync| matches!(sync, Sync::UserFence { .. })));
        rig.poll();
        assert_eq!((rig.monitored(), rig.fence()), (50, 1));
        // Nor past a value a run on its way will write.
        rig.set_monitored(0);
        rig.xe.hold_runs();
        let work = context_work(&rig, 5, rig.data_address() + TAG_IN_DATA);
        let higher = started(&mut rig, &work);
        rig.work.ring_doorbell(higher).unwrap();
        let mut lower = context_work(&rig, 6, rig.data_address() + TAG_IN_DATA);
        crate::protocol::command_header::tests::put_marks(
            &mut lower.private,
            (rig.data_address() + TAG_IN_DATA, 600),
            (rig.monitored_addresses().0, 20),
        );
        let doorbell = started(&mut rig, &lower);
        rig.work.ring_doorbell(doorbell).unwrap();
        let Some(Call::Exec { syncs, .. }) = rig.xe.calls_named("exec").pop() else { panic!() };
        assert!(!syncs.iter().any(|sync| matches!(sync, Sync::UserFence { .. })));
        rig.xe.release_runs();
        rig.poll();
        assert_eq!(rig.monitored(), 50);
    }

    #[test]
    fn the_marks_of_a_run_xe_refused_or_whose_queue_went_are_still_written() {
        let mut rig = ready();
        let tag = rig.data_address() + TAG_IN_DATA;
        let work = context_work(&rig, 1, tag);
        let doorbell = started(&mut rig, &work);
        rig.xe.fail("exec", libc::EINVAL);
        assert!(rig.work.ring_doorbell(doorbell).is_err());
        rig.xe.heal();
        rig.poll();
        assert_eq!((rig.monitored(), rig.data_dword(TAG_IN_DATA as usize)), (10, 100));
        rig.xe.hold_runs();
        let work = context_work(&rig, 2, tag);
        let doorbell = started(&mut rig, &work);
        rig.work.ring_doorbell(doorbell).unwrap();
        let work = unrun(context_work(&rig, 3, tag));
        let unsent = started(&mut rig, &work);
        rig.work.close_queue(1);
        assert!(rig.work.ring_doorbell(unsent).is_err(), "it went with its queue");
        rig.poll();
        assert_eq!((rig.monitored(), rig.data_dword(TAG_IN_DATA as usize)), (30, 300));
        assert_eq!(host_wrote(&rig, rig.monitored_addresses().1), [10, 20, 30], "in order");
    }

    #[test]
    fn a_fence_its_process_let_go_of_before_the_marks_came_is_left_alone() {
        let mut rig = ready();
        let work = context_work(&rig, 1, rig.data_address() + TAG_IN_DATA);
        let doorbell = started(&mut rig, &work);
        rig.xe.fail("exec", libc::EINVAL);
        assert!(rig.work.ring_doorbell(doorbell).is_err());
        rig.host.release_monitored();
        rig.poll();
        assert_eq!((rig.monitored(), rig.data_dword(TAG_IN_DATA as usize)), (0, 100));
        assert_eq!(rig.fence(), 0, "the queue's fence is the core's once the run is refused");
    }

    #[test]
    fn a_tag_the_guests_batch_already_took_further_is_not_stepped_back() {
        let mut rig = ready();
        rig.write_data(TAG_IN_DATA as usize, &500u32.to_le_bytes());
        let work = context_work(&rig, 1, rig.data_address() + TAG_IN_DATA);
        let doorbell = started(&mut rig, &work);
        rig.work.ring_doorbell(doorbell).unwrap();
        rig.poll();
        assert_eq!((rig.data_dword(TAG_IN_DATA as usize), rig.fence()), (500, 1));
    }

    #[test]
    fn only_a_compact_headers_marks_are_read() {
        let mut rig = ready();
        let mut work = unrun(context_work(&rig, 3, rig.data_address() + TAG_IN_DATA));
        work.private.resize(crate::protocol::command_header::FULL_SIZE, 0);
        rig.start(&work);
        assert_eq!((rig.monitored(), rig.data_dword(TAG_IN_DATA as usize)), (0, 0));
    }

    #[test]
    fn with_no_card_a_contexts_fence_and_a_tag_in_guest_pages_are_still_written() {
        let mut rig = ready();
        let work = unrun(context_work(&rig, 3, rig.data_address() + TAG_IN_DATA));
        let built = work.work();
        super::write_marks_without_card(
            &mut rig.host,
            &built,
            &crate::protocol::command_header::Header::read(built.private),
        );
        assert_eq!((rig.monitored(), rig.data_dword(TAG_IN_DATA as usize)), (30, 300));
    }

    #[test]
    fn a_context_batch_that_parks_is_refused_as_its_marks_would_never_come() {
        let mut rig = ready();
        let mut dwords = batch::store_qword(rig.data_address() + 0x40, 0xBEEF).to_vec();
        dwords.extend([batch::SEMAPHORE_WAIT_HEADER | 1 << 15 | 1 << 12, 1, 0x4000_1000, 0, 0]);
        rig.write_batch(&dwords);
        let work = context_work(&rig, 1, rig.data_address() + TAG_IN_DATA);
        assert_eq!(rig.start(&work), WorkStart::QueueFailed);
        assert_eq!(rig.monitored(), 10, "its marks are written all the same");
        assert!(rig.xe.calls_named("exec").is_empty());
    }
}
