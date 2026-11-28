//! The kernel's list of the buffers a space's submissions may touch, made once and named by its
//! handle until the buffers change. Making a list only looks the handles up and takes a reference on
//! each buffer; the kernel still locks and validates every buffer in it at each submission. What a
//! submission saves is the lookup and the building of the list from the handles it would carry.

use crate::memory::GemHandle;
use crate::submit;
use std::fs::File;
use std::io;

/// What the cache asks of the kernel, so its rules can be tried without a card.
pub trait ListCalls {
    /// Makes a list of `buffers`, or replaces the one `existing` names, and returns its handle.
    fn make(&self, buffers: &[GemHandle], existing: Option<u64>) -> io::Result<u64>;
    /// Lets the kernel free `list`, and with it its hold on every buffer in it.
    fn destroy(&self, list: u64);
}

impl ListCalls for File {
    fn make(&self, buffers: &[GemHandle], existing: Option<u64>) -> io::Result<u64> {
        submit::make_bo_list(self, buffers, existing)
    }

    fn destroy(&self, list: u64) {
        let _ = submit::destroy_bo_list(self, list);
    }
}

/// The list a space has made, and whether it is still the one to name.
#[derive(Debug)]
pub struct BufferList {
    /// The kernel's handle for the list. Looking the handles up and building the list costs on every
    /// submission that carries them, so it is made only when the buffers change.
    list: Option<u64>,
    /// Whether a mapping has changed since the list was made.
    stale: bool,
    /// The buffers the list was made from. A submission that names the list gets these, not the
    /// array it carries, so a list made from another set is a list that names the wrong buffers.
    holds: Vec<GemHandle>,
    /// Set once the kernel has refused a list here for good, so every submission after that carries
    /// its own rather than asking again.
    given_up: bool,
}

impl Default for BufferList {
    fn default() -> Self {
        Self { list: None, stale: true, holds: Vec::new(), given_up: false }
    }
}

impl BufferList {
    /// Notes that a mapping was added or taken away, so the list is made again before it is named.
    pub fn mark_stale(&mut self) {
        self.stale = true;
    }

    /// Lets the kernel free the list. A list holds a reference on every buffer in it, so a buffer's
    /// memory is only freed once the list is gone, whatever happens to its handle.
    pub fn destroy(&mut self, calls: &impl ListCalls) {
        if let Some(list) = self.list.take() {
            calls.destroy(list);
        }
        self.holds.clear();
    }

    /// Frees the list if it holds `handle`, which is about to be closed.
    pub fn close(&mut self, calls: &impl ListCalls, handle: GemHandle) {
        if self.holds.contains(&handle) {
            self.destroy(calls);
        }
    }

    /// The list to name for a submission that may touch `buffers`, made if there isn't one that
    /// holds exactly them. `None` when the submission should carry the buffers itself.
    pub fn get(&mut self, calls: &impl ListCalls, buffers: &[GemHandle]) -> Option<u64> {
        if self.given_up {
            return None;
        }
        if self.list.is_some() && !self.stale && self.holds == buffers {
            return self.list;
        }
        if buffers.is_empty() {
            self.destroy(calls);
            return None;
        }
        let made = match calls.make(buffers, self.list) {
            Ok(made) => made,
            Err(error) if may_pass_later(&error) => {
                // Out of memory or interrupted: this submission names its buffers itself, and the
                // next one asks for the list again. A refused update leaves the old list as it was.
                log::debug!("the kernel didn't take this space's buffer list this time ({error})");
                return None;
            }
            Err(error) => {
                // The kernel refuses a list it cannot take, and a submission that names its buffers
                // outright still runs, so the space gets that from now on rather than losing its
                // card. This cost the screen once, when a refusal went up instead of sideways.
                // A refused update leaves the old list under its handle, still holding its buffers.
                self.destroy(calls);
                self.given_up = true;
                log::warn!(
                    "the kernel won't take this space's buffer list ({error}); submissions name the buffers themselves"
                );
                return None;
            }
        };
        self.list = Some(made);
        self.stale = false;
        self.holds.clear();
        self.holds.extend_from_slice(buffers);
        Some(made)
    }
}

/// Whether the kernel's refusal of a buffer list is one that can pass, such as no memory for it
/// just now or a call cut short, as opposed to a kernel that will not make lists here at all.
fn may_pass_later(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(libc::ENOMEM | libc::EINTR | libc::EAGAIN | libc::EBUSY))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    const A: GemHandle = GemHandle(1);
    const B: GemHandle = GemHandle(2);
    const C: GemHandle = GemHandle(3);

    /// A kernel that numbers the lists it makes and remembers every call, and refuses on request.
    #[derive(Default)]
    struct Kernel {
        made: RefCell<Vec<(Vec<GemHandle>, Option<u64>)>>,
        destroyed: RefCell<Vec<u64>>,
        handed_out: Cell<u64>,
        refuse_with: Cell<Option<i32>>,
    }

    impl ListCalls for Kernel {
        fn make(&self, buffers: &[GemHandle], existing: Option<u64>) -> io::Result<u64> {
            self.made.borrow_mut().push((buffers.to_vec(), existing));
            if let Some(code) = self.refuse_with.get() {
                return Err(io::Error::from_raw_os_error(code));
            }
            Ok(existing.unwrap_or_else(|| {
                self.handed_out.set(self.handed_out.get() + 1);
                self.handed_out.get()
            }))
        }

        fn destroy(&self, list: u64) {
            self.destroyed.borrow_mut().push(list);
        }
    }

    #[test]
    fn a_list_is_made_once_and_named_again_while_the_buffers_stay_the_same() {
        let (kernel, mut lists) = (Kernel::default(), BufferList::default());
        let first = lists.get(&kernel, &[A, B]);
        assert_eq!(first, Some(1));
        assert_eq!(lists.get(&kernel, &[A, B]), first);
        assert_eq!(lists.get(&kernel, &[A, B]), first);
        assert_eq!(kernel.made.borrow().len(), 1, "the kernel was asked once");
    }

    #[test]
    fn a_mapping_that_changed_has_the_list_updated_in_place_before_it_is_named_again() {
        let (kernel, mut lists) = (Kernel::default(), BufferList::default());
        let first = lists.get(&kernel, &[A, B]);
        lists.mark_stale();
        assert_eq!(lists.get(&kernel, &[A, B]), first, "an update keeps the handle");
        assert_eq!(kernel.made.borrow()[1], (vec![A, B], first), "and is asked as an update");
        assert_eq!(lists.get(&kernel, &[A, B]), first);
        assert_eq!(kernel.made.borrow().len(), 2, "the list is fresh again after it");
    }

    #[test]
    fn another_set_of_buffers_has_the_list_made_again_even_if_nothing_was_marked() {
        let (kernel, mut lists) = (Kernel::default(), BufferList::default());
        lists.get(&kernel, &[A, B]);
        lists.get(&kernel, &[A, B, C]);
        assert_eq!(kernel.made.borrow()[1].0, vec![A, B, C]);
        lists.get(&kernel, &[A, B]);
        assert_eq!(kernel.made.borrow().len(), 3, "going back to the first set is another set");
    }

    #[test]
    fn no_buffers_frees_the_list_and_names_nothing() {
        let (kernel, mut lists) = (Kernel::default(), BufferList::default());
        lists.get(&kernel, &[A]);
        assert_eq!(lists.get(&kernel, &[]), None);
        assert_eq!(*kernel.destroyed.borrow(), [1]);
        assert_eq!(lists.get(&kernel, &[A]), Some(2), "a list made after it is a new one");
    }

    #[test]
    fn closing_a_buffer_the_list_holds_frees_the_list_and_closing_another_does_not() {
        let (kernel, mut lists) = (Kernel::default(), BufferList::default());
        lists.get(&kernel, &[A, B]);
        lists.close(&kernel, C);
        assert!(kernel.destroyed.borrow().is_empty());
        lists.close(&kernel, B);
        assert_eq!(*kernel.destroyed.borrow(), [1]);
        lists.close(&kernel, B);
        assert_eq!(kernel.destroyed.borrow().len(), 1, "a list that is gone is not freed twice");
    }

    #[test]
    fn a_refusal_for_good_frees_the_old_list_and_every_submission_after_carries_its_own() {
        let (kernel, mut lists) = (Kernel::default(), BufferList::default());
        lists.get(&kernel, &[A]);
        lists.mark_stale();
        kernel.refuse_with.set(Some(libc::EINVAL));
        assert_eq!(lists.get(&kernel, &[A, B]), None);
        assert_eq!(*kernel.destroyed.borrow(), [1], "the old list was still there to free");
        kernel.refuse_with.set(None);
        assert_eq!(lists.get(&kernel, &[A, B]), None, "it does not ask again");
        assert_eq!(kernel.made.borrow().len(), 2);
    }

    #[test]
    fn a_refusal_that_can_pass_is_asked_again_by_the_next_submission() {
        let (kernel, mut lists) = (Kernel::default(), BufferList::default());
        kernel.refuse_with.set(Some(libc::ENOMEM));
        assert_eq!(lists.get(&kernel, &[A]), None);
        assert_eq!(lists.get(&kernel, &[A]), None);
        assert_eq!(kernel.made.borrow().len(), 2, "asked on each");
        kernel.refuse_with.set(None);
        assert_eq!(lists.get(&kernel, &[A]), Some(1));
        assert!(kernel.destroyed.borrow().is_empty());
    }

    #[test]
    fn a_refused_update_that_can_pass_keeps_the_old_list_to_update_again() {
        let (kernel, mut lists) = (Kernel::default(), BufferList::default());
        let first = lists.get(&kernel, &[A]);
        lists.mark_stale();
        kernel.refuse_with.set(Some(libc::EINTR));
        assert_eq!(lists.get(&kernel, &[A, B]), None);
        kernel.refuse_with.set(None);
        assert_eq!(lists.get(&kernel, &[A, B]), first);
        assert_eq!(kernel.made.borrow()[2], (vec![A, B], first), "the same list is updated");
        assert!(kernel.destroyed.borrow().is_empty());
    }

    #[test]
    fn a_list_refused_for_want_of_memory_or_by_a_signal_is_tried_again_and_any_other_is_not() {
        for code in [libc::ENOMEM, libc::EINTR, libc::EAGAIN, libc::EBUSY] {
            assert!(may_pass_later(&io::Error::from_raw_os_error(code)), "errno {code}");
        }
        for code in [libc::ENOTTY, libc::EINVAL, libc::EPERM, libc::ENOENT, libc::EFAULT] {
            assert!(!may_pass_later(&io::Error::from_raw_os_error(code)), "errno {code}");
        }
        assert!(!may_pass_later(&io::Error::other("the kernel gave no handle for the buffer list")));
    }
}
