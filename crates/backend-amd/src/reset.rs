//! Getting going again after amdgpu resets the card.
//!
//! A context that had work running when the card hung keeps the error that work ended with, and
//! amdgpu fails every later submission on it with that error, ETIME for work that timed out. A context made before a reset that lost video
//! memory is older than the VM's new generation, and amdgpu fails every submission on it with
//! ECANCELED. Neither ever takes work again, but a context made after the reset does. So a space
//! that finds one spoiled lets all its contexts go, as a reset reaches them all, and makes others
//! the next time it needs them.
//!
//! A guest's queue that was there before the reset isn't moved onto a fresh context: its work and
//! what it kept in video memory may be gone, and running on as if nothing happened would hide that.
//! It fails instead, as the guest's own driver would see after a reset, and the guest makes new
//! queues, which run.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt;
use std::io;

/// Video memory was lost since the context was made (`AMDGPU_CTX_QUERY2_FLAGS_VRAMLOST`).
pub const FLAG_VRAM_LOST: u64 = 1 << 1;
/// Work on the context hung the card.
pub const FLAG_GUILTY: u64 = 1 << 2;

/// Whether amdgpu refused work on a context with `error` because a reset spoiled the context for
/// good. `flags` are what amdgpu says of the context, when it could be asked.
pub fn spoiled_by_reset(error: &io::Error, flags: Option<u64>) -> bool {
    let refused_for_good = matches!(error.raw_os_error(), Some(libc::ETIME | libc::ECANCELED));
    refused_for_good || flags.is_some_and(|flags| flags & (FLAG_VRAM_LOST | FLAG_GUILTY) != 0)
}

/// What a space asks of amdgpu about its contexts, apart so the tests can stand in for it.
pub trait ContextCalls {
    type Context;
    fn make(&self) -> io::Result<Self::Context>;
    /// Whether a reset spoiled `context`, after amdgpu refused work on it with `error`.
    fn spoiled(&self, context: &Self::Context, error: &io::Error) -> bool;
    fn free(&self, context: Self::Context);
}

/// A space's contexts, one for each engine family, made on first use, and the guest queues that
/// have brought work here.
#[derive(Debug)]
pub struct Contexts<C> {
    made: HashMap<u32, C>,
    /// How many times a reset was found here.
    resets: u64,
    /// The count when each queue first brought work.
    queues: HashMap<u32, u64>,
}

impl<C> Default for Contexts<C> {
    fn default() -> Self {
        Self { made: HashMap::new(), resets: 0, queues: HashMap::new() }
    }
}

impl<C> Contexts<C> {
    /// Submits by `submit` on the context for `ip_type`, for `queue`, or for the daemon's own work
    /// with `None`. A reset found here lets every context go, as it reaches them all, and the
    /// daemon's own work is tried once more on a fresh one. A guest's queue from before the reset
    /// fails, then and after.
    pub fn submit<K: ContextCalls<Context = C>>(
        &mut self,
        calls: &K,
        ip_type: u32,
        queue: Option<u32>,
        mut submit: impl FnMut(&C) -> io::Result<()>,
    ) -> io::Result<()> {
        if let Some(queue) = queue
            && *self.queues.entry(queue).or_insert(self.resets) != self.resets
        {
            return Err(io::Error::other(
                "the queue was there before amdgpu reset the card, so its work may be lost",
            ));
        }
        let mut tries = if queue.is_none() { 2 } else { 1 };
        loop {
            let context =
                match self.made.entry(ip_type) {
                    Entry::Occupied(slot) => slot.into_mut(),
                    Entry::Vacant(slot) => slot.insert(calls.make().map_err(|error| {
                        io::Error::new(error.kind(), format!("making a context: {error}"))
                    })?),
                };
            let error = match submit(context) {
                Ok(()) => return Ok(()),
                Err(error) => error,
            };
            if error.kind() == io::ErrorKind::WouldBlock || !calls.spoiled(context, &error) {
                return Err(error);
            }
            for (_, spoiled) in self.made.drain() {
                calls.free(spoiled);
            }
            self.resets += 1;
            tries -= 1;
            if tries == 0 {
                return Err(io::Error::new(error.kind(), AfterReset(error)));
            }
        }
    }

    /// Forgets a queue that has gone.
    pub fn forget_queue(&mut self, queue: u32) {
        self.queues.remove(&queue);
    }
}

/// The kernel's refusal of the run that showed the reset. It keeps the kernel's own error, so the
/// error number can still be asked for.
#[derive(Debug)]
struct AfterReset(io::Error);

impl fmt::Display for AfterReset {
    fn fmt(&self, text: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(text, "{}, after amdgpu reset the card", self.0)
    }
}

impl std::error::Error for AfterReset {}

/// The kernel's error number behind a failed submission, also when [`Contexts::submit`] has said
/// what the failure meant.
pub fn os_error(error: &io::Error) -> Option<i32> {
    let behind = || error.get_ref()?.downcast_ref::<AfterReset>()?.0.raw_os_error();
    error.raw_os_error().or_else(behind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    /// Stands in for amdgpu: contexts are numbered, and those made before `reset_after` are
    /// spoiled.
    #[derive(Default)]
    struct Kernel {
        made: Cell<u32>,
        reset_after: Cell<u32>,
        freed: RefCell<Vec<u32>>,
    }

    impl Kernel {
        fn reset(&self) {
            self.reset_after.set(self.made.get());
        }

        fn cs(&self, context: &u32) -> io::Result<()> {
            if *context < self.reset_after.get() {
                Err(io::Error::from_raw_os_error(libc::ECANCELED))
            } else {
                Ok(())
            }
        }
    }

    impl ContextCalls for Kernel {
        type Context = u32;
        fn make(&self) -> io::Result<u32> {
            self.made.set(self.made.get() + 1);
            Ok(self.made.get() - 1)
        }
        fn spoiled(&self, _context: &u32, error: &io::Error) -> bool {
            spoiled_by_reset(error, None)
        }
        fn free(&self, context: u32) {
            self.freed.borrow_mut().push(context);
        }
    }

    const GFX: u32 = 0;
    const DMA: u32 = 2;
    const OWN: Option<u32> = None;

    #[test]
    fn the_daemons_own_work_runs_on_a_fresh_context_after_a_reset() {
        let kernel = Kernel::default();
        let mut contexts = Contexts::default();
        contexts.submit(&kernel, DMA, OWN, |context| kernel.cs(context)).unwrap();
        kernel.reset();
        let mut used = Vec::new();
        let mut cs = |&context: &u32| {
            used.push(context);
            kernel.cs(&context)
        };
        contexts.submit(&kernel, DMA, OWN, &mut cs).unwrap();
        // And it stays on the fresh one.
        contexts.submit(&kernel, DMA, OWN, &mut cs).unwrap();
        assert_eq!(used, vec![0, 1, 1]);
        assert_eq!(*kernel.freed.borrow(), vec![0]);
    }

    #[test]
    fn a_queue_from_before_a_reset_fails_and_a_new_one_runs_on_a_fresh_context() {
        let kernel = Kernel::default();
        let mut contexts = Contexts::default();
        let (old, other_old, new) = (Some(1), Some(2), Some(3));
        contexts.submit(&kernel, GFX, old, |context| kernel.cs(context)).unwrap();
        contexts.submit(&kernel, DMA, other_old, |context| kernel.cs(context)).unwrap();
        kernel.reset();
        let failed = contexts.submit(&kernel, GFX, old, |context| kernel.cs(context)).unwrap_err();
        assert!(failed.to_string().contains("reset"));
        // Every context went, not just the one found spoiled.
        assert_eq!(*kernel.freed.borrow(), vec![0, 1]);
        // Neither queue from before is put on a fresh context, so the guest hears of the reset.
        assert!(contexts.submit(&kernel, GFX, old, |context| kernel.cs(context)).is_err());
        assert!(contexts.submit(&kernel, DMA, other_old, |context| kernel.cs(context)).is_err());
        contexts.submit(&kernel, GFX, new, |context| kernel.cs(context)).unwrap();
        contexts.submit(&kernel, DMA, new, |context| kernel.cs(context)).unwrap();
        // A queue id the guest uses again after the old queue went is a new queue.
        contexts.forget_queue(1);
        contexts.submit(&kernel, GFX, old, |context| kernel.cs(context)).unwrap();
    }

    #[test]
    fn work_refused_for_another_reason_keeps_its_context() {
        let kernel = Kernel::default();
        let mut contexts = Contexts::default();
        let busy = |_: &u32| Err(io::Error::from_raw_os_error(libc::EAGAIN));
        assert!(contexts.submit(&kernel, GFX, Some(1), busy).is_err());
        let bad = |_: &u32| Err(io::Error::from_raw_os_error(libc::EINVAL));
        assert!(contexts.submit(&kernel, GFX, OWN, bad).is_err());
        contexts.submit(&kernel, GFX, Some(1), |context| kernel.cs(context)).unwrap();
        assert_eq!(kernel.made.get(), 1);
        assert!(kernel.freed.borrow().is_empty());
    }

    #[test]
    fn a_reset_is_known_by_the_error_or_by_what_amdgpu_says_of_the_context() {
        let error = |code| io::Error::from_raw_os_error(code);
        assert!(spoiled_by_reset(&error(libc::ETIME), None));
        assert!(spoiled_by_reset(&error(libc::ECANCELED), None));
        assert!(spoiled_by_reset(&error(libc::EIO), Some(FLAG_VRAM_LOST)));
        assert!(spoiled_by_reset(&error(libc::EIO), Some(FLAG_GUILTY)));
        // A context that only saw a reset, with nothing lost, still takes work.
        let reset_seen = 1;
        assert!(!spoiled_by_reset(&error(libc::EINVAL), Some(reset_seen)));
        assert!(!spoiled_by_reset(&error(libc::EINVAL), None));
    }

    #[test]
    fn the_kernels_error_number_is_still_there_after_a_reset_is_named() {
        let plain = io::Error::from_raw_os_error(libc::ETIME);
        assert_eq!(os_error(&plain), Some(libc::ETIME));
        let named = io::Error::new(plain.kind(), AfterReset(plain));
        assert_eq!(named.raw_os_error(), None);
        assert_eq!(os_error(&named), Some(libc::ETIME));
        assert!(named.to_string().ends_with(", after amdgpu reset the card"));
        assert_eq!(os_error(&io::Error::other("the queue was there before")), None);
    }
}
