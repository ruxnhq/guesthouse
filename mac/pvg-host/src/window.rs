// SPDX-License-Identifier: GPL-2.0-or-later
//! A window on the Mac for the guest's picture, through the Objective-C file in `shim/`.
//!
//! A process has one. AppKit wants the process's first thread, so [`open`] and [`Window::run`]
//! are called there, and the guest is served from another thread, which holds a [`Handle`].
//!
//! With input, the window's keys, pointer, buttons and wheel come to the callbacks here on the
//! first thread, and go to the guest from it through [`crate::input`].

use crate::input::{self, Input, Qmp};
use guesthouse_pvg_sys::Glyph;
use std::ffi::{CString, c_char, c_void};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::Mutex;

/// How the window takes input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Taking {
    /// None: the window shows the guest and takes neither the keyboard nor the pointer.
    Nothing,
    /// Keys and pointer, with the window in front and the keyboard's.
    Focus,
    /// The same, from made-up events of the control socket only: the window never takes the
    /// keyboard, for runs with no person.
    Test,
}

impl Taking {
    fn number(self) -> i32 {
        match self {
            Taking::Nothing => 0,
            Taking::Focus => 1,
            Taking::Test => 2,
        }
    }
}

/// A made-up event for the window's input, as `pvg_window_test_event` takes it. `Edge` is a
/// person's drag of the window's edge as AppKit hands it over: the press at x, y, then the
/// window's content at `width` by `height` pixels, with the release kept from the window. `Drag`
/// is a left press at x, y, a drag by `right`, `down` points and the release, handed to AppKit
/// itself, which takes it for a drag of the window's size or hands it on. `Screen` moves the
/// window to the screen of that index in `NSScreen.screens`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TestEvent {
    Key { code: u16, down: bool },
    Flags { code: u16, flags: u64 },
    Move { x: f64, y: f64 },
    Button { button: u32, down: bool, x: f64, y: f64 },
    Scroll { right: i32, up: i32, x: f64, y: f64 },
    Resign,
    Close,
    Edge { width: u32, height: u32, x: f64, y: f64 },
    Drag { x: f64, y: f64, right: i32, down: i32 },
    Screen { index: u32 },
}

#[repr(C)]
struct RawCallbacks {
    resized: extern "C" fn(u32, u32),
    key: extern "C" fn(u16, bool),
    pointer: extern "C" fn(f64, f64, f64, f64, u32, u32),
    button: extern "C" fn(u32, bool),
    wheel: extern "C" fn(i32, i32),
    released: extern "C" fn(),
    closing: extern "C" fn() -> bool,
}

unsafe extern "C" {
    fn pvg_window_open(
        title: *const c_char,
        width: u32,
        height: u32,
        callbacks: *const RawCallbacks,
        taking: i32,
    ) -> bool;
    fn pvg_window_iso_keyboard() -> bool;
    fn pvg_window_screen_scale() -> f64;
    fn pvg_window_test_event(kind: i32, a: i64, b: i64, x: f64, y: f64);
    fn pvg_window_full_screen();
    fn pvg_window_close();
    fn pvg_window_layer() -> *mut c_void;
    fn pvg_window_run();
    fn pvg_window_stop();
    fn pvg_window_resize(width: u32, height: u32);
    fn pvg_window_picture_size(width: u32, height: u32);
    fn pvg_window_glyph(pixels: *const u8, width: u32, height: u32, hot_x: u32, hot_y: u32);
    fn pvg_window_cursor(x: i32, y: i32, shown: bool);
    fn pvg_window_state(width: *mut u32, height: *mut u32, visible: *mut bool, cursor: *mut bool);
    fn pvg_window_ping();
    fn pvg_window_look(
        answered_ago: *mut u64,
        inside: *mut *const c_char,
        left: *mut *const c_char,
        mode: *mut c_char,
        room: usize,
    );
}

/// The size the window's content was last given, until the serving thread takes it, and the
/// writing end of the pipe that wakes that thread.
struct Resized {
    size: Option<(u32, u32)>,
    wake: Option<OwnedFd>,
}

static RESIZED: Mutex<Resized> = Mutex::new(Resized { size: None, wake: None });

extern "C" fn resized(width: u32, height: u32) {
    let mut state = RESIZED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    state.size = Some((width, height));
    if let Some(pipe) = &state.wake {
        // SAFETY: one byte from a live buffer into a pipe this process owns. A full pipe means
        // the serving thread has not looked yet, and will find the newest size when it does.
        unsafe { libc::write(pipe.as_raw_fd(), [0u8].as_ptr().cast(), 1) };
    }
}

/// The guest's keys and buttons, and the socket they go through. Only the first thread sends; the
/// thread that finds the socket puts it here.
static INPUT: Mutex<Option<Input>> = Mutex::new(None);

fn with_input(work: impl FnOnce(&mut Input)) {
    let mut input = INPUT.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(input) = input.as_mut() {
        work(input);
    }
}

extern "C" fn key(code: u16, down: bool) {
    with_input(|input| input.key(code, down));
}

extern "C" fn pointer(x: f64, y: f64, width: f64, height: f64, picture_width: u32, picture_height: u32) {
    if let Some((x, y)) = input::absolute(x, y, width, height, (picture_width, picture_height)) {
        with_input(|input| input.point(x, y));
    }
}

extern "C" fn button(button: u32, down: bool) {
    with_input(|input| input.button(button, down));
}

extern "C" fn wheel(right: i32, up: i32) {
    with_input(|input| input.wheel(right, up));
}

extern "C" fn released() {
    with_input(Input::release_all);
}

/// The window's close button: the guest's power button, through QEMU's monitor. Returns whether
/// the guest was asked.
extern "C" fn closing() -> bool {
    let mut asked = false;
    let mut there = false;
    with_input(|input| {
        there = true;
        asked = input.power_down();
    });
    match (there, asked) {
        (_, true) => eprintln!("pvg-host: the window was closed, and the guest is asked to power down"),
        (true, false) => {
            eprintln!("pvg-host: the window was closed, and QEMU's monitor is not there to tell")
        }
        (false, _) => eprintln!("pvg-host: the window was closed, and with no QEMU monitor it only hides"),
    }
    asked
}

/// Hands QEMU's monitor socket to the window's input, once it is there.
pub fn input_connected(qmp: Qmp) {
    with_input(|input| input.connected(qmp));
}

/// How input is doing, as a line for the control socket's `status`.
pub fn input_status() -> Option<String> {
    let input = INPUT.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    input.as_ref().map(|input| {
        let (connected, answered, errors) = input.state();
        let sent = input.sent();
        format!(
            "input connected {connected} answered {answered} errors {errors} keys {} moves {} buttons {} steps {}",
            sent.keys, sent.moves, sent.buttons, sent.steps
        )
    })
}

/// The backing scale of the screen the window opens on, 1 or 2, for a guest's desktop that is to
/// show its things at the size the Mac shows them.
pub fn screen_scale() -> f64 {
    // SAFETY: the call only asks AppKit about the screens.
    unsafe { pvg_window_screen_scale() }
}

/// How the window's first thread is doing, for [`crate::watch`], and the work that tells it next
/// time: from any thread, once the window is open.
pub fn look_main() -> crate::watch::Main {
    let (mut answered_ago, mut inside, mut left) = (0u64, std::ptr::null(), std::ptr::null());
    let mut mode = [0 as c_char; 128];
    // SAFETY: the shim writes the four values, the names are static strings of its own, and the
    // mode ends in a zero within the room given.
    unsafe {
        pvg_window_look(&raw mut answered_ago, &raw mut inside, &raw mut left, mode.as_mut_ptr(), mode.len());
        pvg_window_ping();
    }
    let text = |pointer: *const c_char| {
        // SAFETY: as above.
        unsafe { std::ffi::CStr::from_ptr(pointer) }.to_string_lossy().into_owned()
    };
    crate::watch::Main {
        answered_ago: std::time::Duration::from_millis(answered_ago),
        inside: match text(inside) {
            name if name.is_empty() => "none of ours".to_owned(),
            name => name,
        },
        left: text(left),
        mode: text(mode.as_ptr()),
    }
}

/// The window, on the process's first thread.
pub struct Window(());

/// What the thread that serves the guest does with the window. All of it may be called from any
/// thread.
pub struct Handle {
    layer: *mut c_void,
    woken: OwnedFd,
}

// SAFETY: the layer is only handed on to `FrameSource`, whose calls on it may come from any
// thread, and every other call here hands its work to the first thread.
unsafe impl Send for Handle {}
// SAFETY: as above.
unsafe impl Sync for Handle {}

/// Opens the window with a content of this size, in points, taking input or not. With `monitor`,
/// QEMU's monitor socket is to come through [`input_connected`], for the input and the close
/// button, or for the close button alone with [`Taking::Nothing`]. Call on the process's first
/// thread.
pub fn open(
    title: &str,
    width: u32,
    height: u32,
    taking: Taking,
    monitor: bool,
) -> Result<(Window, Handle), String> {
    let title = CString::new(title).map_err(|_| "a window's title can't hold a zero byte".to_owned())?;
    let mut ends = [0 as RawFd; 2];
    // SAFETY: the array has room for the two descriptors the call writes.
    if unsafe { libc::pipe(ends.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    // SAFETY: both descriptors were just made and nothing else holds them.
    let (woken, wake) = unsafe { (OwnedFd::from_raw_fd(ends[0]), OwnedFd::from_raw_fd(ends[1])) };
    for end in [&woken, &wake] {
        // SAFETY: setting flags on descriptors this process owns.
        unsafe {
            libc::fcntl(end.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK);
            libc::fcntl(end.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
    RESIZED.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).wake = Some(wake);
    if taking != Taking::Nothing || monitor {
        // Without input the shim hands on no keys or pointer, so the socket only powers down.
        // SAFETY: the call only asks the system what kind of keyboard it has.
        let iso = unsafe { pvg_window_iso_keyboard() };
        *INPUT.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Input::new(iso));
    }
    let callbacks = RawCallbacks { resized, key, pointer, button, wheel, released, closing };
    // SAFETY: the title ends in a zero and lives through the call, as the callbacks do; the shim
    // copies what it keeps.
    if !unsafe { pvg_window_open(title.as_ptr(), width, height, &raw const callbacks, taking.number()) } {
        return Err("no window could be opened".to_owned());
    }
    // SAFETY: the window is open, so its layer is there.
    let layer = unsafe { pvg_window_layer() };
    Ok((Window(()), Handle { layer, woken }))
}

impl Window {
    /// Serves the window until [`Handle::stop`].
    pub fn run(&self) {
        // SAFETY: the window is open, and this is the thread that opened it.
        unsafe { pvg_window_run() };
    }
}

impl Handle {
    /// The `CAMetalLayer` the guest's frames are drawn into.
    pub fn layer(&self) -> *mut c_void {
        self.layer
    }

    /// Readable when the window's content has a new size that nobody has taken yet.
    pub fn resized_fd(&self) -> RawFd {
        self.woken.as_raw_fd()
    }

    /// The size the window's content was last given, in pixels, if it changed since the last
    /// call.
    pub fn take_size(&self) -> Option<(u32, u32)> {
        let mut bytes = [0u8; 64];
        // SAFETY: the pipe is this process's, and the buffer has room for what is asked.
        while unsafe { libc::read(self.woken.as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len()) } > 0 {}
        RESIZED.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).size.take()
    }

    /// The window's content in pixels, whether any of the window can be seen, and whether the
    /// cursor's layer shows, as the first thread last saw them. From any thread: nothing waits.
    pub fn state(&self) -> (u32, u32, bool, bool) {
        let (mut width, mut height, mut visible, mut cursor) = (0, 0, false, false);
        // SAFETY: the four values are the call's to write, and it returns when it has.
        unsafe { pvg_window_state(&raw mut width, &raw mut height, &raw mut visible, &raw mut cursor) };
        (width, height, visible, cursor)
    }

    pub fn stop(&self) {
        // SAFETY: the call only queues work for the first thread.
        unsafe { pvg_window_stop() };
    }

    /// Gives the window's content this size in pixels, as a user's drag would.
    pub fn resize(&self, width: u32, height: u32) {
        // SAFETY: as above.
        unsafe { pvg_window_resize(width, height) };
    }

    /// Does what the window's close button does, with input or without.
    pub fn close(&self) {
        // SAFETY: the call only queues work for the first thread.
        unsafe { pvg_window_close() };
    }

    /// Takes the window to full screen or back, as its green button does.
    pub fn full_screen(&self) {
        // SAFETY: the call only queues work for the first thread.
        unsafe { pvg_window_full_screen() };
    }

    /// Hands the window's input a made-up event, as AppKit would hand it a real one. The window
    /// drops it unless it was opened with [`Taking::Test`].
    pub fn test_event(&self, event: TestEvent) {
        let (kind, a, b, x, y) = match event {
            TestEvent::Key { code, down } => (0, i64::from(code), i64::from(down), 0.0, 0.0),
            TestEvent::Flags { code, flags } => (1, i64::from(code), flags as i64, 0.0, 0.0),
            TestEvent::Move { x, y } => (2, 0, 0, x, y),
            TestEvent::Button { button, down, x, y } => (3, i64::from(button), i64::from(down), x, y),
            TestEvent::Scroll { right, up, x, y } => (4, i64::from(right), i64::from(up), x, y),
            TestEvent::Resign => (5, 0, 0, 0.0, 0.0),
            TestEvent::Close => (6, 0, 0, 0.0, 0.0),
            TestEvent::Edge { width, height, x, y } => (7, i64::from(width), i64::from(height), x, y),
            TestEvent::Drag { x, y, right, down } => (8, i64::from(right), i64::from(down), x, y),
            TestEvent::Screen { index } => (9, i64::from(index), 0, 0.0, 0.0),
        };
        // SAFETY: the call only queues work for the first thread.
        unsafe { pvg_window_test_event(kind, a, b, x, y) };
    }

    pub fn picture_size(&self, width: u32, height: u32) {
        // SAFETY: as above.
        unsafe { pvg_window_picture_size(width, height) };
    }

    pub fn glyph(&self, glyph: &Glyph) {
        if glyph.pixels.len() as u64 != u64::from(glyph.width) * u64::from(glyph.height) * 4 {
            return;
        }
        // SAFETY: the pixels are as many as the size says, and the shim copies them before it
        // returns.
        unsafe {
            pvg_window_glyph(glyph.pixels.as_ptr(), glyph.width, glyph.height, glyph.hot_x, glyph.hot_y)
        };
    }

    pub fn cursor(&self, x: i32, y: i32, shown: bool) {
        // SAFETY: the call only queues work for the first thread.
        unsafe { pvg_window_cursor(x, y, shown) };
    }
}

impl TestEvent {
    /// Reads an event as the control socket's `input` takes it:
    /// `key CODE down|up`, `flags CODE FLAGS`, `move X Y`, `button N down|up X Y`,
    /// `scroll RIGHT UP X Y`, `resign`, `close`, `edge WIDTH HEIGHT X Y`, `drag X Y RIGHT DOWN`,
    /// `screen N`. Key codes are the Mac's, decimal or 0x hex, points the window content's from
    /// its top left.
    pub fn read(text: &str) -> Result<Self, String> {
        let words: Vec<&str> = text.split_whitespace().collect();
        let number = |word: &str| match word.strip_prefix("0x") {
            Some(hex) => i64::from_str_radix(hex, 16).ok(),
            None => word.parse::<i64>().ok(),
        };
        let point = |word: &str| word.parse::<f64>().ok().filter(|value| value.is_finite());
        let down = |word: &str| match word {
            "down" => Some(true),
            "up" => Some(false),
            _ => None,
        };
        let event = match words.as_slice() {
            ["key", code, way] => number(code)
                .and_then(|code| u16::try_from(code).ok())
                .zip(down(way))
                .map(|(code, down)| TestEvent::Key { code, down }),
            ["flags", code, flags] => number(code)
                .and_then(|code| u16::try_from(code).ok())
                .zip(number(flags).and_then(|flags| u64::try_from(flags).ok()))
                .map(|(code, flags)| TestEvent::Flags { code, flags }),
            ["move", x, y] => point(x).zip(point(y)).map(|(x, y)| TestEvent::Move { x, y }),
            ["button", button, way, x, y] => number(button)
                .and_then(|button| u32::try_from(button).ok())
                .zip(down(way))
                .zip(point(x).zip(point(y)))
                .map(|((button, down), (x, y))| TestEvent::Button { button, down, x, y }),
            ["scroll", right, up, x, y] => number(right)
                .and_then(|right| i32::try_from(right).ok())
                .zip(number(up).and_then(|up| i32::try_from(up).ok()))
                .zip(point(x).zip(point(y)))
                .map(|((right, up), (x, y))| TestEvent::Scroll { right, up, x, y }),
            ["resign"] => Some(TestEvent::Resign),
            ["close"] => Some(TestEvent::Close),
            ["edge", width, height, x, y] => number(width)
                .and_then(|width| u32::try_from(width).ok())
                .zip(number(height).and_then(|height| u32::try_from(height).ok()))
                .zip(point(x).zip(point(y)))
                .map(|((width, height), (x, y))| TestEvent::Edge { width, height, x, y }),
            ["drag", x, y, right, down] => point(x)
                .zip(point(y))
                .zip(number(right).and_then(|right| i32::try_from(right).ok()))
                .zip(number(down).and_then(|down| i32::try_from(down).ok()))
                .map(|(((x, y), right), down)| TestEvent::Drag { x, y, right, down }),
            ["screen", index] => number(index)
                .and_then(|index| u32::try_from(index).ok())
                .map(|index| TestEvent::Screen { index }),
            _ => None,
        };
        event.ok_or_else(|| {
            "an input event is one of: key CODE down|up, flags CODE FLAGS, move X Y, button N down|up X Y, scroll RIGHT UP X Y, resign, close, edge WIDTH HEIGHT X Y, drag X Y RIGHT DOWN, screen N"
                .to_owned()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_events_are_read_as_written() {
        assert_eq!(TestEvent::read("key 0x24 down"), Ok(TestEvent::Key { code: 0x24, down: true }));
        assert_eq!(TestEvent::read("key 0 up"), Ok(TestEvent::Key { code: 0, down: false }));
        assert_eq!(TestEvent::read("flags 56 0x20102"), Ok(TestEvent::Flags { code: 56, flags: 0x20102 }));
        assert_eq!(TestEvent::read("move 10.5 20"), Ok(TestEvent::Move { x: 10.5, y: 20.0 }));
        assert_eq!(
            TestEvent::read("button 1 up 3 4"),
            Ok(TestEvent::Button { button: 1, down: false, x: 3.0, y: 4.0 })
        );
        assert_eq!(
            TestEvent::read("scroll 0 -3 100 100"),
            Ok(TestEvent::Scroll { right: 0, up: -3, x: 100.0, y: 100.0 })
        );
        assert_eq!(TestEvent::read("resign"), Ok(TestEvent::Resign));
        assert_eq!(TestEvent::read("close"), Ok(TestEvent::Close));
        assert_eq!(
            TestEvent::read("edge 1200 800 959 270"),
            Ok(TestEvent::Edge { width: 1200, height: 800, x: 959.0, y: 270.0 })
        );
        assert_eq!(
            TestEvent::read("drag 959 270 40 -10"),
            Ok(TestEvent::Drag { x: 959.0, y: 270.0, right: 40, down: -10 })
        );
        assert_eq!(TestEvent::read("screen 1"), Ok(TestEvent::Screen { index: 1 }));
        for broken in [
            "",
            "key 70000 down",
            "key 1 sideways",
            "move 1",
            "move nan 2",
            "button 1 down 2",
            "jump",
            "drag 1 2 3",
            "screen -1",
        ] {
            assert!(TestEvent::read(broken).is_err(), "{broken}");
        }
    }
}
