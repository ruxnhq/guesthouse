//! Has the guest's screen take the size of the viewer's window while a screen is open.
//!
//! The picture's place in the window is looked at a few times a second, and a size goes to the guest
//! once it has stopped changing: every new size makes Windows lay out all its windows again, and the
//! display driver takes its monitor away for a moment to offer one it hasn't had. A thread asks the
//! guest, as that takes about a second.

use super::screen::Screen;
use gtk::glib;
use guesthouse_app_core::agent::{Sized, size_guest_screen, start_guest_window_size, stop_guest_window_size};
use guesthouse_app_core::context::Context;
use guesthouse_app_core::display::machine_modes;
use guesthouse_app_core::driver_modes::DriverModes;
use guesthouse_app_core::log::write_log;
use guesthouse_app_core::machine::Machine;
use guesthouse_daemon::slot::Slot;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread;
use std::time::Duration;

const BETWEEN_LOOKS: Duration = Duration::from_millis(250);
/// How many looks in a row find the same size before it's sent: half a second of a still window.
const STILL_LOOKS: u32 = 2;
const BETWEEN_STARTS: Duration = Duration::from_secs(5);
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
}

impl Watch {
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
        let size = size.filter(|size| self.still == STILL_LOOKS && self.sent != Some(*size))?;
        self.sent = Some(size);
        (frame != Some(size)).then_some(size)
    }
}

/// A guest that restarted has lost its helper, so one that doesn't answer is started again.
fn run(context: &Context, machine: &Machine, slot: Option<Slot>, modes: &DriverModes, wake: &Receiver<Size>) {
    let mut started = false;
    let mut wanted: Option<Size> = None;
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
        match outcome {
            Sized::Set => {}
            // Windows on its basic display, or a size it won't take. The next size may do better.
            Sized::Not(why) => {
                let _ = write_log(context, &format!("window size: {width}x{height} not set: {why}"));
            }
            Sized::NoHelper => {
                started = false;
                match wake.recv_timeout(BETWEEN_STARTS) {
                    Ok(newer) => wanted = Some(newer),
                    Err(RecvTimeoutError::Timeout) => wanted = Some((width, height)),
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        }
    }
    if started {
        stop_guest_window_size(context, machine, slot);
    }
}

pub struct WindowSize {
    looks: Option<glib::SourceId>,
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
        let looks = glib::timeout_add_local(BETWEEN_LOOKS, move || {
            let picture = screen.has_frames().then(|| screen.picture_pixels()).flatten();
            let frame =
                screen.frame_size().map(|(width, height)| (width.unsigned_abs(), height.unsigned_abs()));
            if let Some(size) = watch.look(picture, frame) {
                let _ = for_guest.send(size);
            }
            glib::ControlFlow::Continue
        });
        Some(Self { looks: Some(looks), _sizes: sizes })
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
}
