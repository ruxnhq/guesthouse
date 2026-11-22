//! Has the guest's screen take the size of the viewer's window while a screen is open.
//!
//! The picture's place in the window is looked at a few times a second, and a size goes to the guest
//! once it has stopped changing: every new size makes Windows lay out all its windows again, and the
//! display driver takes its monitor away for a moment to offer one it hasn't had. A thread asks the
//! guest, as that takes about a second.
//!
//! A start, a restart in the guest and a change to the display's settings end with the guest at the
//! size in the machine's settings. Nothing is sent while such a task runs, and the window's size is
//! sent again once it's over.
//!
//! While an app in the guest is fullscreen, the guest's helper leaves the screen alone and the size
//! waits: a game whose resolution changes under it can close.

use super::screen::Screen;
use gtk::glib;
use guesthouse_app_core::agent::{Sized, size_guest_screen, start_guest_window_size, stop_guest_window_size};
use guesthouse_app_core::context::Context;
use guesthouse_app_core::display::machine_modes;
use guesthouse_app_core::driver_modes::DriverModes;
use guesthouse_app_core::log::write_log;
use guesthouse_app_core::machine::Machine;
use guesthouse_app_core::tasks::Status;
use guesthouse_daemon::slot::Slot;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread;
use std::time::Duration;

const BETWEEN_LOOKS: Duration = Duration::from_millis(250);
/// How many looks in a row find the same size before it's sent: half a second of a still window.
const STILL_LOOKS: u32 = 2;
const BETWEEN_STARTS: Duration = Duration::from_secs(5);
/// How often a size a fullscreen app holds back is asked for again. The helper only looks, and
/// doesn't touch the screen while the app is there.
const BETWEEN_HELD_ASKS: Duration = Duration::from_secs(3);
/// Windows' desktop is hard to use below this, and its own settings offer nothing smaller.
const SMALLEST: Size = (800, 600);
const LARGEST: Size = (7680, 4320);

type Size = (u32, u32);

/// The size the guest is asked for: within what Windows takes, and even both ways, as video
/// formats that halve the colour planes need.
fn fit((width, height): Size) -> Size {
    let even = |length: u32, least: u32, most: u32| length.clamp(least, most) & !1;
    (even(width, SMALLEST.0, LARGEST.0), even(height, SMALLEST.1, LARGEST.1))
}

/// What the looks have seen so far.
#[derive(Default)]
struct Watch {
    seen: Option<Size>,
    still: u32,
    sent: Option<Size>,
    /// A task is setting the guest's screen.
    held: bool,
}

impl Watch {
    /// Holds the sizes back while a task sets the guest's screen. Once it's done, the window's size is
    /// sent again when the window is still, as the task will have put the guest back at its own.
    fn hold(&mut self, held: bool) {
        if self.held && !held {
            self.sent = None;
            self.still = 0;
        }
        self.held = held;
    }

    /// The size to send now, given what this look found: the picture's size, and the size of the
    /// guest's frames. A size is sent once, so a guest that then picks its own, as a game does, keeps it.
    fn look(&mut self, picture: Option<Size>, frame: Option<Size>) -> Option<Size> {
        let size = picture.map(fit);
        if size != self.seen {
            self.seen = size;
            self.still = 0;
            return None;
        }
        self.still = self.still.saturating_add(1);
        if self.held {
            return None;
        }
        let size = size.filter(|size| self.still == STILL_LOOKS && self.sent != Some(*size))?;
        self.sent = Some(size);
        (frame != Some(size)).then_some(size)
    }
}

/// A guest that restarted has lost its helper, so one that doesn't answer is started again. A size
/// a fullscreen app held back is asked for again until the app lets go, or a newer size comes.
fn run(context: &Context, machine: &Machine, slot: Option<Slot>, modes: &DriverModes, wake: &Receiver<Size>) {
    let mut started = false;
    let mut wanted: Option<Size> = None;
    // What the log last said, so a size that waits, or a helper that stays away, is one line.
    let mut said: Option<Said> = None;
    while let Some(size) = wanted.or_else(|| wake.recv().ok()) {
        // Only the newest matters.
        let (width, height) = wake.try_iter().last().unwrap_or(size);
        if !started {
            started = start_guest_window_size(context, machine, slot).unwrap_or(false);
        }
        let mode = (width, height, machine.refresh_hz);
        let outcome = match started {
            true => size_guest_screen(context, machine, mode, modes, slot).unwrap_or(Sized::NoHelper),
            false => Sized::NoHelper,
        };
        wanted = None;
        let say = |what: Said, said: &mut Option<Said>, text: String| {
            if *said != Some(what) {
                let _ = write_log(context, &text);
            }
            *said = Some(what);
        };
        let wait = match outcome {
            // Every time, as the same size set again is what a size sent twice looks like.
            Sized::Set => {
                let _ = write_log(context, &format!("window size: {width}x{height} set"));
                said = Some(Said::Set);
                continue;
            }
            // Windows on its basic display, or a size it won't take. The next size may do better.
            Sized::Not(why) => {
                say(
                    Said::Not((width, height)),
                    &mut said,
                    format!("window size: {width}x{height} not set: {why}"),
                );
                continue;
            }
            Sized::Held => {
                let text = format!("window size: {width}x{height} waits, as an app in Windows is fullscreen");
                say(Said::Held((width, height)), &mut said, text);
                BETWEEN_HELD_ASKS
            }
            Sized::NoHelper => {
                let text =
                    "window size: the helper in Windows didn't answer, so it's started again".to_owned();
                say(Said::NoHelper, &mut said, text);
                started = false;
                BETWEEN_STARTS
            }
        };
        match wake.recv_timeout(wait) {
            Ok(newer) => wanted = Some(newer),
            Err(RecvTimeoutError::Timeout) => wanted = Some((width, height)),
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    if started {
        stop_guest_window_size(context, machine, slot);
    }
}

/// The kinds of line the log has had, each for its size.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Said {
    Set,
    Not(Size),
    Held(Size),
    NoHelper,
}

pub struct WindowSize {
    looks: Option<glib::SourceId>,
    /// Whether a task is setting the guest's screen, which the app's look at its tasks says.
    held: Rc<Cell<bool>>,
    /// Dropping this ends the thread, which is why it's kept.
    _sizes: Sender<Size>,
}

impl WindowSize {
    /// `None` in a build with no display driver, which has no modes to add a size to.
    pub fn new(screen: Screen, context: Arc<Context>, machine: Machine, slot: Option<Slot>) -> Option<Self> {
        let modes = machine_modes(&context, &machine)?;
        let (sizes, wake) = channel();
        thread::spawn(move || run(&context, &machine, slot, &modes, &wake));
        let for_guest = sizes.clone();
        let mut watch = Watch::default();
        // Held until the app has looked at the machine's tasks, as a start may be setting the screen.
        let held = Rc::new(Cell::new(true));
        let holding = held.clone();
        let looks = glib::timeout_add_local(BETWEEN_LOOKS, move || {
            watch.hold(holding.get());
            let picture = screen.has_frames().then(|| screen.picture_pixels()).flatten();
            let frame =
                screen.frame_size().map(|(width, height)| (width.unsigned_abs(), height.unsigned_abs()));
            if let Some(size) = watch.look(picture, frame) {
                let _ = for_guest.send(size);
            }
            glib::ControlFlow::Continue
        });
        Some(Self { looks: Some(looks), held, _sizes: sizes })
    }

    /// What the app found its tasks doing.
    pub fn follow_tasks(&self, status: &Status) {
        self.held.set(status.sets_screen);
    }
}

impl Drop for WindowSize {
    fn drop(&mut self) {
        if let Some(looks) = self.looks.take() {
            looks.remove();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_size_is_made_even_and_kept_within_what_windows_takes() {
        assert_eq!(fit((1283, 733)), (1282, 732));
        assert_eq!(fit((300, 200)), SMALLEST);
        assert_eq!(fit((9000, 5001)), (7680, 4320));
    }

    #[test]
    fn a_size_is_sent_once_the_window_is_still() {
        let mut watch = Watch::default();
        let frame = Some((1920, 1080));
        assert_eq!(watch.look(Some((1500, 844)), frame), None, "just seen");
        assert_eq!(watch.look(Some((1600, 900)), frame), None, "still changing");
        assert_eq!(watch.look(Some((1600, 900)), frame), None, "one look isn't still");
        assert_eq!(watch.look(Some((1600, 900)), frame), Some((1600, 900)));
        assert_eq!(watch.look(Some((1600, 900)), frame), None, "sent once");
        assert_eq!(watch.look(Some((1600, 900)), Some((1024, 768))), None, "a guest's own size is left");
    }

    #[test]
    fn a_size_the_guest_already_has_isn_t_sent() {
        let mut watch = Watch::default();
        for _ in 0..4 {
            assert_eq!(watch.look(Some((1920, 1080)), Some((1920, 1080))), None);
        }
        assert_eq!(watch.look(None, None), None, "no picture yet");
    }

    /// Looks at a still window until one sends, or gives up.
    fn send(watch: &mut Watch, picture: Size, frame: Size) -> Option<Size> {
        (0..=STILL_LOOKS).find_map(|_| watch.look(Some(picture), Some(frame)))
    }

    #[test]
    fn nothing_is_sent_while_a_task_sets_the_screen() {
        let mut watch = Watch::default();
        watch.hold(true);
        assert_eq!(send(&mut watch, (3518, 1794), (2560, 1440)), None);
        watch.hold(false);
        assert_eq!(send(&mut watch, (3518, 1794), (2560, 1440)), Some((3518, 1794)), "sent once it's over");
        assert_eq!(send(&mut watch, (3518, 1794), (2560, 1440)), None, "and only once");
    }

    #[test]
    fn the_window_s_size_goes_again_after_a_task_has_set_the_screen() {
        let mut watch = Watch::default();
        assert_eq!(send(&mut watch, (3518, 1794), (2560, 1440)), Some((3518, 1794)));
        watch.hold(true);
        assert_eq!(send(&mut watch, (3518, 1794), (2560, 1440)), None, "the task puts its size back");
        watch.hold(false);
        assert_eq!(send(&mut watch, (3518, 1794), (2560, 1440)), Some((3518, 1794)));
    }

    #[test]
    fn a_task_that_left_the_window_s_size_has_nothing_sent_after_it() {
        let mut watch = Watch::default();
        watch.hold(true);
        watch.hold(false);
        assert_eq!(send(&mut watch, (3518, 1794), (3518, 1794)), None);
    }

    #[test]
    fn a_guest_s_own_size_is_left_by_a_task_that_doesn_t_set_the_screen() {
        let mut watch = Watch::default();
        assert_eq!(send(&mut watch, (3518, 1794), (2560, 1440)), Some((3518, 1794)));
        // A game picks its own, and a task that doesn't touch the screen runs and ends.
        watch.hold(false);
        assert_eq!(send(&mut watch, (3518, 1794), (1920, 1080)), None);
        watch.hold(true);
        watch.hold(false);
        assert_eq!(send(&mut watch, (3518, 1794), (3518, 1794)), None);
        assert_eq!(send(&mut watch, (3518, 1794), (1920, 1080)), None, "a game's size after that is left");
    }
}
