//! Files between this PC and a Windows guest, by dragging: dropped on the screen they arrive where
//! the guest's pointer is, and dragged out of the screen they come along.
//!
//! A helper in the guest does the guest's half, see [`guesthouse_app_core::guest_files`]. It runs
//! while a screen is open, like the clipboard's.

mod drag_out;
mod dragged_files;
mod drop_in;

use super::screen::Screen;
use crate::window::Window;
use drag_out::DragOut;
use drop_in::DropIn;
use gtk::gio;
use guesthouse_app_core::agent::{files_helper, start_guest_files, stop_guest_files};
use guesthouse_app_core::context::Context;
use guesthouse_app_core::guest_files::Helper;
use guesthouse_app_core::log::write_log;
use guesthouse_app_core::machine::Machine;
use guesthouse_daemon::slot::Slot;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Linux's key code for Escape, which ends a drag of the guest's own in Windows.
const ESCAPE: u32 = 1;
/// PowerShell takes a few seconds to build the helper before it listens.
const START_PATIENCE: Duration = Duration::from_secs(15);
const BETWEEN_LOOKS: Duration = Duration::from_millis(300);

/// The way to the guest's helper, and what starts it.
struct Link {
    helper: Helper,
    context: Arc<Context>,
    machine: Machine,
    slot: Option<Slot>,
    /// One start at a time: a second would stop the first one's helper.
    starting: Mutex<()>,
    /// The helper answered when it was last asked. Nothing that holds the guest's pointer asks
    /// one that didn't.
    ready: AtomicBool,
}

impl Link {
    /// What became of a drop or a drag, for a report. A log that can't be written is no reason
    /// to stop.
    fn log(&self, text: &str) {
        let _ = write_log(&self.context, &format!("files: {text}"));
    }

    /// Starts the helper if it isn't answering, as a guest that restarted has lost it. Says
    /// whether it answers now. This waits, so it is for a thread.
    fn ensure(&self) -> bool {
        let _one = self.starting.lock();
        let ready = self.helper.answers() || self.start();
        self.ready.store(ready, Ordering::Relaxed);
        ready
    }

    fn start(&self) -> bool {
        if !start_guest_files(&self.context, &self.machine, self.slot).unwrap_or(false) {
            return false;
        }
        let end = Instant::now() + START_PATIENCE;
        while Instant::now() < end {
            if self.helper.answers() {
                return true;
            }
            thread::sleep(BETWEEN_LOOKS);
        }
        false
    }

    fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Relaxed)
    }

    /// The helper stopped answering, as it does when the guest restarts. It's started again for
    /// the next drag.
    fn start_again(self: &Arc<Self>) {
        self.ready.store(false, Ordering::Relaxed);
        let link = self.clone();
        thread::spawn(move || link.ensure());
    }
}

/// Runs `work` on a thread. `None` if it panicked.
async fn off_thread<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    gio::spawn_blocking(work).await.ok()
}

pub struct SharedFiles {
    link: Arc<Link>,
    _drop_in: Rc<DropIn>,
    _drag_out: Rc<DragOut>,
}

impl SharedFiles {
    /// `None` for a machine with no token, which has no agent to start the helper.
    pub fn new(
        window: &Window,
        screen: &Screen,
        context: Arc<Context>,
        machine: Machine,
        slot: Option<Slot>,
    ) -> Option<Self> {
        let helper = files_helper(&context, &machine, slot)?;
        let link = Arc::new(Link {
            helper,
            context,
            machine,
            slot,
            starting: Mutex::default(),
            ready: AtomicBool::new(false),
        });
        // Started now, so the first drop doesn't wait for it.
        let warming = link.clone();
        thread::spawn(move || warming.ensure());
        Some(Self {
            _drop_in: DropIn::new(window, screen, link.clone()),
            _drag_out: DragOut::new(window, screen, link.clone()),
            link,
        })
    }
}

impl Drop for SharedFiles {
    fn drop(&mut self) {
        let link = self.link.clone();
        thread::spawn(move || stop_guest_files(&link.context, &link.machine, link.slot));
    }
}
