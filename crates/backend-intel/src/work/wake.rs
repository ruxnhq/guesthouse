//! Being woken when work the host must finish has ended, instead of looking for it.
//!
//! A context's submission has the host write its queue's fence and its marks once its run has
//! ended, and so does a run whose fence the card can't write. The guest waits on those (a present
//! token, a signal from the context), so the host must notice the end when it happens. For each
//! such run the kernel is asked to add one to an eventfd when the run's point on its queue's
//! timeline is signalled (`DRM_IOCTL_SYNCOBJ_EVENTFD`). The daemon sleeps on the eventfd beside the
//! guest's channels, and once it is readable the runs are looked at.
//!
//! A kernel before Linux v6.6 doesn't have the call. Then those runs are looked at each time the
//! core polls, which it does about every millisecond while there are any.

use super::CardWork;
use guesthouse_backend::HostServices;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::time::Instant;

/// The eventfd, made on first use, and whether the kernel takes it.
#[derive(Debug, Default)]
pub(super) struct Wake {
    eventfd: Option<OwnedFd>,
    unsupported: bool,
    /// A run the host must finish is already finishable, with nothing to wake for.
    look_now: bool,
}

impl Wake {
    /// The descriptor the daemon sleeps on, once one is made.
    pub fn descriptor(&self) -> Option<RawFd> {
        self.eventfd.as_ref().map(AsRawFd::as_raw_fd)
    }

    /// Takes what the eventfd holds, so it isn't readable again until the next signal.
    fn drain(&self) {
        let Some(eventfd) = &self.eventfd else { return };
        let mut count = [0u8; 8];
        // SAFETY: eight bytes into a live array, from a descriptor this value owns. The eventfd is
        // non-blocking, so with nothing in it the read fails with EAGAIN, which is ignored.
        unsafe { libc::read(eventfd.as_raw_fd(), count.as_mut_ptr().cast(), count.len()) };
    }
}

fn new_eventfd() -> io::Result<OwnedFd> {
    // SAFETY: eventfd returns a fresh descriptor or -1.
    let raw = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the descriptor is fresh and owned by nothing else.
    Ok(unsafe { OwnedFd::from_raw_fd(raw) })
}

impl CardWork {
    /// Asks to be woken when a timeline reaches a point. Without the kernel's help the runs are
    /// polled instead: see [`CardWork::needs_polling`].
    pub(super) fn wake_at(&mut self, timeline: u32, point: u64) {
        if self.wake.unsupported {
            return;
        }
        let made = match &self.wake.eventfd {
            Some(eventfd) => Ok(eventfd.as_raw_fd()),
            None => new_eventfd().map(|eventfd| self.wake.eventfd.insert(eventfd).as_raw_fd()),
        };
        let armed = made.and_then(|fd| self.xe.notify_at(timeline, point, fd));
        if let Err(error) = armed {
            self.wake.unsupported = true;
            log::info!(
                "intel work: the kernel won't wake the daemon when a run ends ({error}), so runs the \
                 host finishes are looked for about every millisecond instead"
            );
        }
    }

    /// Has the runs looked at on the next poll, as one is already finishable.
    pub(super) fn look_soon(&mut self) {
        self.wake.look_now = true;
    }

    /// The eventfd the daemon sleeps on.
    pub fn wake_descriptor(&self) -> Option<RawFd> {
        self.wake.descriptor()
    }

    /// Whether the core must poll: a run is already finishable, or runs the host finishes are on
    /// the card and nothing will wake the daemon for them.
    pub fn needs_polling(&self) -> bool {
        self.wake.look_now || (self.wake.unsupported && self.host_finishes_runs())
    }

    /// Takes the wake, and finishes every run that has ended.
    pub fn woke(&mut self, host: &mut dyn HostServices, now: Instant) {
        self.wake.drain();
        self.finish_ended_runs(host, now);
    }

    /// Finishes the runs that ended, on each queue with a run the host finishes.
    pub(super) fn finish_ended_runs(&mut self, host: &mut dyn HostServices, now: Instant) {
        self.wake.look_now = false;
        self.write_left_marks(host);
        let ids: Vec<u32> = self
            .queues
            .iter()
            .filter(|(_, queue)| queue.in_flight.iter().any(super::queues::Run::host_finishes))
            .map(|(&id, _)| id)
            .collect();
        for id in ids {
            self.look_at_runs(host, id, now);
        }
    }

    /// Whether any run on the card is one the host finishes.
    fn host_finishes_runs(&self) -> bool {
        self.queues.values().any(|queue| queue.in_flight.iter().any(super::queues::Run::host_finishes))
    }
}

#[cfg(test)]
mod tests {
    use crate::stage::Stage;
    use crate::work::testing::{Rig, WorkBuilder};
    use crate::xe::batch;
    use crate::xe::fake::Call;
    use guesthouse_backend::WorkStart;
    use std::time::{Duration, Instant};

    /// A rig whose batch ends at once, whose process holds the monitored fence, with runs held.
    fn ready() -> Rig {
        let mut rig = Rig::new(Stage::Everything);
        rig.write_batch(&[batch::BATCH_END]);
        let (gpu_address, guest_address) = rig.monitored_addresses();
        rig.host.hold_monitored(rig.space, gpu_address, guest_address);
        rig.xe.hold_runs();
        rig
    }

    fn context_work(rig: &Rig, value: u64) -> WorkBuilder {
        let monitored = (rig.monitored_addresses().0, value * 10);
        rig.submission(1, value).sent_to_context((rig.data_address() + 0x80, value as u32 * 100), monitored)
    }

    fn ring(rig: &mut Rig, work: &WorkBuilder) {
        let WorkStart::Started { doorbell, .. } = rig.start(work) else { panic!() };
        rig.work.ring_doorbell(doorbell).unwrap();
    }

    fn readable(rig: &Rig) -> bool {
        let fd = rig.work.wake_descriptor().expect("an eventfd");
        let mut place = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        // SAFETY: one pollfd on the stack, and no waiting.
        unsafe { libc::poll(&mut place, 1, 0) == 1 }
    }

    fn notified(rig: &Rig) -> Vec<u64> {
        let calls = rig.xe.calls_named("notify_at").into_iter();
        calls
            .map(|call| if let Call::NotifyAt { point, .. } = call { point } else { unreachable!() })
            .collect()
    }

    #[test]
    fn a_context_run_wakes_the_daemon_when_it_ends_and_its_marks_are_written_then() {
        let mut rig = ready();
        let now = Instant::now();
        rig.work.poll(&mut rig.host, now);
        let work = context_work(&rig, 1);
        ring(&mut rig, &work);
        assert_eq!(notified(&rig), [1]);
        assert!(!readable(&rig), "the run hasn't ended");
        assert!(!rig.work.needs_polling(), "the eventfd will say");
        rig.xe.release_runs();
        assert!(readable(&rig));
        // Well inside the fifty milliseconds between looks for runs xe ended early.
        rig.work.woke(&mut rig.host, now + Duration::from_millis(1));
        assert_eq!(rig.fence(), 1, "the queue's fence, written at once");
        assert!(!readable(&rig), "and the wake taken");
    }

    #[test]
    fn work_behind_a_run_wakes_for_that_run() {
        let mut rig = ready();
        let work = context_work(&rig, 1);
        ring(&mut rig, &work);
        let mut unrun = context_work(&rig, 2);
        unrun.commands = None;
        ring(&mut rig, &unrun);
        assert_eq!(notified(&rig), [1, 1], "both end with the one run");
        rig.xe.release_runs();
        rig.work.woke(&mut rig.host, Instant::now());
        assert_eq!(rig.fence(), 2);
    }

    #[test]
    fn a_run_whose_card_writes_its_fence_asks_for_no_wake() {
        let mut rig = ready();
        let work = rig.submission(1, 1);
        ring(&mut rig, &work);
        assert!(notified(&rig).is_empty());
        assert_eq!(rig.work.wake_descriptor(), None, "nothing made");
    }

    #[test]
    fn without_the_kernels_help_runs_the_host_finishes_are_polled_without_the_fifty_milliseconds() {
        let mut rig = ready();
        rig.xe.fail("notify_at", libc::ENOTTY);
        let now = Instant::now();
        rig.work.poll(&mut rig.host, now);
        let work = context_work(&rig, 1);
        ring(&mut rig, &work);
        assert!(rig.work.needs_polling());
        rig.xe.release_runs();
        rig.work.poll(&mut rig.host, now + Duration::from_millis(1));
        assert_eq!(rig.fence(), 1);
        assert!(!rig.work.needs_polling(), "nothing left to finish");
        let work = context_work(&rig, 2);
        ring(&mut rig, &work);
        assert_eq!(notified(&rig).len(), 1, "the kernel isn't asked again");
    }
}
