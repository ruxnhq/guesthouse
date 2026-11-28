// SPDX-License-Identifier: GPL-2.0-or-later
//! The calls into Apple's framework, through the Objective-C file in `shim/`. All of this crate's
//! calls out of Rust are here, and with `memory.rs` all of its `unsafe`.
//!
//! Nothing here judges what a guest says. [`crate::Device`] is the only user, and asks the guard
//! before each call that carries a guest's word.

use crate::Error;
use crate::device::{Config, Event, Frame, Mode, Source};
use crate::memory::GuestMemory;
use std::ffi::{CStr, CString, c_char, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::OnceLock;

/// Who is told what the device does. Called from threads of the framework's own, and from inside
/// a register write on the thread that makes it.
pub(crate) type Sink = Box<dyn Fn(Event) + Send + Sync>;

const ERROR_ROOM: usize = 256;
const BUILD_ROOM: usize = 64;
const PATH_ROOM: usize = 1024;
const BYTES_PER_PIXEL: usize = 4;
/// What the shim's frame call answers.
const FRAME_DONE: i32 = 0;
const FRAME_REFUSED: i32 = 1;
const SOURCE_MAPPER: u32 = 1;

#[repr(C)]
struct RawRange {
    guest_address: u64,
    length: u64,
    host_address: *mut c_void,
}

#[repr(C)]
struct RawMode {
    width: u16,
    height: u16,
    hertz: u16,
}

#[repr(C)]
struct RawConfig {
    name: *const c_char,
    millimetres_wide: u32,
    millimetres_high: u32,
    serial: u32,
    mapper_register_bytes: u64,
}

#[repr(C)]
struct RawCallbacks {
    context: *mut c_void,
    interrupt: extern "C" fn(*mut c_void, u32, u32),
    mode_changed: extern "C" fn(*mut c_void, u32, u32, u32),
    new_frame: extern "C" fn(*mut c_void),
    cursor_shown: extern "C" fn(*mut c_void, bool),
    cursor_moved: extern "C" fn(*mut c_void),
}

unsafe extern "C" {
    fn pvg_host_build(build: *mut c_char, room: usize) -> bool;
    fn pvg_option_rom_path(path: *mut c_char, room: usize) -> bool;
    fn pvg_device_register_bytes() -> u64;
    fn pvg_host_new(
        ranges: *const RawRange,
        count: usize,
        mapper_ranges: *const RawRange,
        mapper_count: usize,
        config: *const RawConfig,
        modes: *const RawMode,
        mode_count: usize,
        callbacks: *const RawCallbacks,
        error: *mut c_char,
        error_room: usize,
    ) -> *mut c_void;
    fn pvg_host_free(host: *mut c_void);
    fn pvg_device_read(host: *mut c_void, offset: u64, value: *mut u32) -> bool;
    fn pvg_device_write(host: *mut c_void, offset: u64, value: u32) -> bool;
    fn pvg_mapper_read(host: *mut c_void, offset: u64, value: *mut u64) -> bool;
    fn pvg_mapper_write(host: *mut c_void, offset: u64, value: u64) -> bool;
    fn pvg_host_reset(host: *mut c_void) -> bool;
    fn pvg_host_set_modes(
        host: *mut c_void,
        modes: *const RawMode,
        count: usize,
        done: extern "C" fn(context: *mut c_void, taken: bool, microseconds: u64),
        context: *mut c_void,
    ) -> bool;
    fn pvg_host_gate_take(host: *mut c_void, milliseconds: u64) -> bool;
    fn pvg_host_gate_give(host: *mut c_void);
    fn pvg_host_frame_size(host: *mut c_void, width: *mut u32, height: *mut u32);
    fn pvg_host_presents(host: *mut c_void, by_guest: *mut u64, by_host: *mut u64);
    fn pvg_host_frame(host: *mut c_void, width: u32, height: u32, pixels: *mut u8, room: usize) -> i32;
    fn pvg_watch_reasons(seen: extern "C" fn(*mut c_void, *const c_char), context: *mut c_void);
    fn pvg_frame_source_new(host: *mut c_void) -> *mut c_void;
    fn pvg_frame_source_free(source: *mut c_void);
    fn pvg_frame_source_set_up(source: *mut c_void, layer: *mut c_void);
    fn pvg_frame_source_draw(source: *mut c_void, layer: *mut c_void) -> bool;
    fn pvg_frame_source_size(source: *mut c_void, width: *mut u32, height: *mut u32);
    fn pvg_frame_source_step() -> *const c_char;
    fn pvg_display_ping();
    fn pvg_display_look(answered_ago: *mut u64, inside: *mut *const c_char) -> bool;
    fn pvg_frame_source_draw_copy(
        source: *mut c_void,
        layer: *mut c_void,
        pixels: *mut u8,
        room: usize,
        width: *mut u32,
        height: *mut u32,
    ) -> bool;
    fn pvg_frame_source_cursor(source: *mut c_void, x: *mut i32, y: *mut i32);
    fn pvg_frame_source_metal(source: *mut c_void) -> *mut c_void;
    fn pvg_frame_source_texture_usage(source: *mut c_void) -> u64;
    fn pvg_frame_source_draw_texture(source: *mut c_void, texture: *mut c_void) -> bool;
}

fn tell(context: *mut c_void, event: Event) {
    // SAFETY: the context is the `Sink` that `Framework` keeps boxed for as long as the shim's
    // host lives, and a `Sink` may be called from any thread.
    let sink = unsafe { &*context.cast::<Sink>() };
    sink(event);
}

extern "C" fn interrupt(context: *mut c_void, source: u32, vector: u32) {
    let source = if source == SOURCE_MAPPER { Source::Mapper } else { Source::Device };
    tell(context, Event::Interrupt { source, vector });
}

extern "C" fn mode_changed(context: *mut c_void, width: u32, height: u32, format: u32) {
    tell(context, Event::ModeChanged { width, height, format });
}

extern "C" fn new_frame(context: *mut c_void) {
    tell(context, Event::NewFrame);
}

extern "C" fn cursor_shown(context: *mut c_void, shown: bool) {
    tell(context, Event::CursorShown(shown));
}

extern "C" fn cursor_moved(context: *mut c_void) {
    tell(context, Event::CursorMoved);
}

fn raw_modes(modes: &[Mode]) -> Vec<RawMode> {
    modes.iter().map(|mode| RawMode { width: mode.width, height: mode.height, hertz: mode.hertz }).collect()
}

/// The macOS build this process runs on, as `sw_vers -buildVersion` gives it.
pub fn host_build() -> Option<String> {
    let mut text = [0 as c_char; BUILD_ROOM];
    // SAFETY: the shim writes at most `BUILD_ROOM` bytes, and what the system gives ends in a zero.
    if !unsafe { pvg_host_build(text.as_mut_ptr(), BUILD_ROOM) } {
        return None;
    }
    text[BUILD_ROOM - 1] = 0;
    // SAFETY: the buffer ends in a zero, set above if nowhere before.
    Some(unsafe { CStr::from_ptr(text.as_ptr()) }.to_string_lossy().into_owned())
}

/// The file of the option ROM the framework has for the device's PCI function, if it has one. It
/// is an EFI driver for a guest's firmware. Whoever shows the guest the PCI function hands it out
/// as that function's ROM. The framework's header says the ROM is no longer used.
pub fn option_rom() -> Option<PathBuf> {
    let mut text = [0 as c_char; PATH_ROOM];
    // SAFETY: the shim writes at most `PATH_ROOM` bytes, ending in a zero, or nothing.
    if !unsafe { pvg_option_rom_path(text.as_mut_ptr(), PATH_ROOM) } {
        return None;
    }
    text[PATH_ROOM - 1] = 0;
    // SAFETY: the buffer ends in a zero, set above if nowhere before.
    let path = unsafe { CStr::from_ptr(text.as_ptr()) };
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(path.to_bytes())))
}

type Listener = Box<dyn Fn(&str) + Send + Sync>;

static REASONS: OnceLock<Listener> = OnceLock::new();

extern "C" fn reason_seen(_context: *mut c_void, reason: *const c_char) {
    if let Some(listener) = REASONS.get() {
        // SAFETY: the shim hands over a string that ends in a zero and lives through this call.
        listener(&unsafe { CStr::from_ptr(reason) }.to_string_lossy());
    }
}

/// Has `listener` hear the reason of every exception thrown in this process from now on.
///
/// The framework refuses a guest's command by throwing, and logs it with the reason blanked; this
/// is how a host program can say why. It is one hook for the whole process, so only the first call
/// counts, and a program that has such a hook of its own should not call it.
pub fn watch_reasons(listener: impl Fn(&str) + Send + Sync + 'static) {
    if REASONS.set(Box::new(listener)).is_ok() {
        // SAFETY: the function given lives as long as the program, and reads only `REASONS`.
        unsafe { pvg_watch_reasons(reason_seen, std::ptr::null_mut()) };
    }
}

/// Where a [`FrameSource`]'s drawing is now, in the framework or in Core Animation, if it is in
/// one of their calls: for a line when the thread that draws stands still. Any thread may ask.
pub fn draw_step() -> Option<&'static str> {
    // SAFETY: the shim hands back a static string of its own, or none.
    let step = unsafe { pvg_frame_source_step() };
    // SAFETY: as above, a static string that ends in a zero.
    (!step.is_null()).then(|| unsafe { CStr::from_ptr(step) }.to_str().unwrap_or("?"))
}

/// How the queue the framework calls the display's handlers on is doing, from any thread: how
/// long ago it last ran a little work it was handed, and which of the display's handlers it is
/// in, if one; `None` before there is a display. Each call hands it that work again.
pub fn display_look() -> Option<(std::time::Duration, Option<&'static str>)> {
    let (mut answered_ago, mut inside) = (0u64, std::ptr::null());
    // SAFETY: the shim writes both values, and the name is a static string of its own or none.
    if !unsafe { pvg_display_look(&raw mut answered_ago, &raw mut inside) } {
        return None;
    }
    // SAFETY: as above.
    unsafe { pvg_display_ping() };
    // SAFETY: a static string that ends in a zero, or none.
    let inside = (!inside.is_null()).then(|| unsafe { CStr::from_ptr(inside) }.to_str().unwrap_or("?"));
    Some((std::time::Duration::from_millis(answered_ago), inside))
}

/// What hears, on the host's queue for mode lists, that a mode list was taken, or not, and how long the
/// framework took for it.
pub type ModesDone = Box<dyn FnOnce(bool, std::time::Duration) + Send>;

extern "C" fn modes_done(context: *mut c_void, taken: bool, microseconds: u64) {
    // SAFETY: the context is the box `set_modes` made, handed back once.
    let done = unsafe { Box::from_raw(context.cast::<ModesDone>()) };
    done(taken, std::time::Duration::from_micros(microseconds));
}

/// The display's picture for a thread other than the device's: it draws the guest's current
/// frame into a Metal layer, such as a window's, or into a texture of the caller's. It keeps the
/// framework's objects alive, with the guest's memory they read, for as long as it lives.
pub struct FrameSource {
    source: *mut c_void,
    _memory: (GuestMemory, GuestMemory),
}

// SAFETY: the framework's display hands out its frame on whatever thread asks, as its own users
// ask from queues of their own, and the shim's object behind this is only read.
unsafe impl Send for FrameSource {}
// SAFETY: as above.
unsafe impl Sync for FrameSource {}

impl FrameSource {
    /// Makes a `CAMetalLayer` one the display can draw into: the device's own Metal device, four
    /// bytes a pixel, and textures the display may write as it needs to.
    ///
    /// # Safety
    ///
    /// `layer` must be a live `CAMetalLayer`.
    pub unsafe fn set_up(&self, layer: *mut c_void) {
        // SAFETY: the source is live, and the caller vouches for the layer.
        unsafe { pvg_frame_source_set_up(self.source, layer) };
    }

    /// Draws the guest's current picture into the layer's next drawable and shows it. `false` if
    /// the guest has shown none yet, or the layer has no drawable to give.
    ///
    /// # Safety
    ///
    /// `layer` must be a live `CAMetalLayer` that [`FrameSource::set_up`] was called on.
    pub unsafe fn draw(&self, layer: *mut c_void) -> bool {
        // SAFETY: the source is live, and the caller vouches for the layer.
        unsafe { pvg_frame_source_draw(self.source, layer) }
    }

    /// As [`FrameSource::draw`], and hands back a copy of what went into the layer: its width,
    /// its height and four bytes a pixel, blue first, rows from the top. `None` if nothing was
    /// drawn.
    ///
    /// # Safety
    ///
    /// As for [`FrameSource::draw`].
    pub unsafe fn draw_copy(&self, layer: *mut c_void) -> Option<(u32, u32, Vec<u8>)> {
        let (mut width, mut height) = (0u32, 0u32);
        // SAFETY: the source is live, and both numbers are the call's to write.
        unsafe { pvg_frame_source_size(self.source, &raw mut width, &raw mut height) };
        // A frame that grew since is not copied.
        let room = width as usize * height as usize * 4;
        let mut pixels = vec![0u8; room];
        // SAFETY: the source is live, the caller vouches for the layer, the buffer has the room
        // given, and both numbers are the call's to write.
        let drawn = unsafe {
            pvg_frame_source_draw_copy(
                self.source,
                layer,
                pixels.as_mut_ptr(),
                room,
                &raw mut width,
                &raw mut height,
            )
        };
        if !drawn {
            return None;
        }
        pixels.truncate(width as usize * height as usize * 4);
        Some((width, height, pixels))
    }

    /// Where the guest's cursor is, in the picture's pixels.
    pub fn cursor(&self) -> (i32, i32) {
        let (mut x, mut y) = (0, 0);
        // SAFETY: the source is live, and both numbers are the call's to write.
        unsafe { pvg_frame_source_cursor(self.source, &raw mut x, &raw mut y) };
        (x, y)
    }

    /// The `MTLDevice` the display draws with, which a texture for [`FrameSource::draw_into`]
    /// has to be made on. It lives as long as this source.
    pub fn metal_device(&self) -> *mut c_void {
        // SAFETY: the source is live; the call only reads it.
        unsafe { pvg_frame_source_metal(self.source) }
    }

    /// The `MTLTextureUsage` a texture for [`FrameSource::draw_into`] has to allow.
    pub fn texture_usage(&self) -> u64 {
        // SAFETY: the source is live; the call only reads it.
        unsafe { pvg_frame_source_texture_usage(self.source) }
    }

    /// Draws the guest's current picture into a texture and waits for the GPU to finish it.
    /// `false` if the guest has shown none, the texture is not of the picture's size, or the
    /// display refused. The pixels go from the display to the texture on the GPU only. Nothing
    /// the guest says reaches this call: it asks the framework for its own last frame.
    ///
    /// # Safety
    ///
    /// `texture` must be a live `MTLTexture` of [`FrameSource::metal_device`], four bytes a pixel
    /// in BGRA order, allowing at least [`FrameSource::texture_usage`].
    pub unsafe fn draw_into(&self, texture: *mut c_void) -> bool {
        // SAFETY: the source is live, and the caller vouches for the texture.
        unsafe { pvg_frame_source_draw_texture(self.source, texture) }
    }
}

impl Drop for FrameSource {
    fn drop(&mut self) {
        // SAFETY: the source is live and is not used again.
        unsafe { pvg_frame_source_free(self.source) };
    }
}

/// The framework's device, display and mapper for one guest.
pub(crate) struct Framework {
    host: NonNull<c_void>,
    /// The framework's blocks call into this for as long as the host lives.
    _sink: Box<Sink>,
    /// The framework reads and writes these for as long as the host lives: the device's memory
    /// and the mapper's.
    _memory: (GuestMemory, GuestMemory),
}

// SAFETY: the framework's objects take calls from any thread, as its own users make them from
// pool threads; and `Device` holds this by `&mut`, so two calls never run at once from here.
unsafe impl Send for Framework {}

impl Framework {
    /// How many bytes of registers the device has.
    pub fn register_bytes() -> u64 {
        // SAFETY: the call takes nothing and makes one object that it lets go again.
        unsafe { pvg_device_register_bytes() }
    }

    /// `memory` is what the device is shown as the guest's memory, and `mapper_memory` what the
    /// surface mapper is.
    pub fn new(
        memory: &GuestMemory,
        mapper_memory: &GuestMemory,
        config: &Config,
        mapper_register_bytes: u64,
        sink: Sink,
    ) -> Result<Self, Error> {
        let name = CString::new(config.name.as_str())
            .map_err(|_| Error::Framework("a display's name can't hold a zero byte".to_owned()))?;
        let raw = |memory: &GuestMemory| -> Vec<RawRange> {
            memory
                .ranges()
                .iter()
                .map(|range| RawRange {
                    guest_address: range.guest_address,
                    length: range.length,
                    host_address: range.host_address.cast(),
                })
                .collect()
        };
        let (ranges, mapper_ranges) = (raw(memory), raw(mapper_memory));
        let modes = raw_modes(&config.modes);
        let raw_config = RawConfig {
            name: name.as_ptr(),
            millimetres_wide: config.millimetres.0,
            millimetres_high: config.millimetres.1,
            serial: config.serial,
            mapper_register_bytes,
        };
        let sink = Box::new(sink);
        let callbacks = RawCallbacks {
            context: (&raw const *sink).cast_mut().cast(),
            interrupt,
            mode_changed,
            new_frame,
            cursor_shown,
            cursor_moved,
        };
        let mut error = [0 as c_char; ERROR_ROOM];
        // SAFETY: every pointer is to a value that lives through the call, with the count that goes
        // with it. The ranges are mapped for as long as `memory` lives and the callbacks' context
        // for as long as `sink` does, and both are kept in the value returned, beside the host.
        let host = unsafe {
            pvg_host_new(
                ranges.as_ptr(),
                ranges.len(),
                mapper_ranges.as_ptr(),
                mapper_ranges.len(),
                &raw const raw_config,
                modes.as_ptr(),
                modes.len(),
                &raw const callbacks,
                error.as_mut_ptr(),
                ERROR_ROOM,
            )
        };
        match NonNull::new(host) {
            Some(host) => Ok(Self { host, _sink: sink, _memory: (memory.clone(), mapper_memory.clone()) }),
            None => {
                error[ERROR_ROOM - 1] = 0;
                // SAFETY: the buffer ends in a zero, set above if nowhere before.
                let why = unsafe { CStr::from_ptr(error.as_ptr()) }.to_string_lossy().into_owned();
                Err(Error::Framework(why))
            }
        }
    }

    /// A register of the device. A read the framework threw on reads as zero.
    pub fn read(&mut self, offset: u64) -> u32 {
        let mut value = 0;
        // SAFETY: the host is live, and `value` is the call's to write.
        unsafe { pvg_device_read(self.host.as_ptr(), offset, &raw mut value) };
        value
    }

    pub fn write(&mut self, offset: u64, value: u32) {
        // SAFETY: the host is live. What the value makes the device do is the guard's to judge.
        unsafe { pvg_device_write(self.host.as_ptr(), offset, value) };
    }

    pub fn mapper_read(&mut self, offset: u64) -> u64 {
        let mut value = 0;
        // SAFETY: the host is live, and `value` is the call's to write.
        unsafe { pvg_mapper_read(self.host.as_ptr(), offset, &raw mut value) };
        value
    }

    pub fn mapper_write(&mut self, offset: u64, value: u64) {
        // SAFETY: the host is live. What the value makes the mapper do is the guard's to judge.
        unsafe { pvg_mapper_write(self.host.as_ptr(), offset, value) };
    }

    pub fn reset(&mut self) -> Result<(), Error> {
        // SAFETY: the host is live.
        if unsafe { pvg_host_reset(self.host.as_ptr()) } {
            Ok(())
        } else {
            Err(Error::Framework("the framework threw on a reset".to_owned()))
        }
    }

    /// Gives the display a new mode list from a queue of the host's own, and returns at once:
    /// `done` hears there whether the framework took the list, and how long it took.
    pub fn set_modes(&mut self, modes: &[Mode], done: ModesDone) -> Result<(), Error> {
        let modes = raw_modes(modes);
        let context = Box::into_raw(Box::new(done));
        // SAFETY: the host is live, the pointer is to `modes.len()` modes, and the context is a
        // box that `modes_done` takes back, once, when the shim calls it.
        let queued = unsafe {
            pvg_host_set_modes(self.host.as_ptr(), modes.as_ptr(), modes.len(), modes_done, context.cast())
        };
        if queued {
            Ok(())
        } else {
            // SAFETY: the shim did not take the context, so it is still this function's box.
            drop(unsafe { Box::from_raw(context) });
            Err(Error::Framework("the framework refused the mode list".to_owned()))
        }
    }

    /// Takes the gate between the mode list's setter and the first ring, waiting at most
    /// `patience` for it; whether it was had. One that was had is given back with
    /// [`Framework::give_gate`].
    pub fn take_gate(&mut self, patience: std::time::Duration) -> bool {
        // SAFETY: the host is live.
        unsafe { pvg_host_gate_take(self.host.as_ptr(), patience.as_millis() as u64) }
    }

    pub fn give_gate(&mut self) {
        // SAFETY: the host is live, and the gate was had.
        unsafe { pvg_host_gate_give(self.host.as_ptr()) }
    }

    /// How many pictures the guest has presented, and how many the host has taken.
    pub fn presents(&mut self) -> (u64, u64) {
        let (mut by_guest, mut by_host) = (0, 0);
        // SAFETY: the host is live, and both numbers are the call's to write.
        unsafe { pvg_host_presents(self.host.as_ptr(), &raw mut by_guest, &raw mut by_host) };
        (by_guest, by_host)
    }

    /// The display's current picture, or `None` before the guest has shown one.
    pub fn frame(&mut self) -> Result<Option<Frame>, Error> {
        let (mut width, mut height) = (0, 0);
        // SAFETY: the host is live, and both numbers are the call's to write.
        unsafe { pvg_host_frame_size(self.host.as_ptr(), &raw mut width, &raw mut height) };
        if width == 0 || height == 0 {
            return Ok(None);
        }
        let mut pixels = vec![0u8; width as usize * height as usize * BYTES_PER_PIXEL];
        // SAFETY: the host is live, and the shim writes at most `pixels.len()` bytes.
        let answer =
            unsafe { pvg_host_frame(self.host.as_ptr(), width, height, pixels.as_mut_ptr(), pixels.len()) };
        match answer {
            FRAME_DONE => Ok(Some(Frame { width, height, pixels })),
            FRAME_REFUSED => Ok(None),
            _ => Err(Error::Framework("the display's picture could not be read".to_owned())),
        }
    }

    pub fn frame_source(&mut self) -> FrameSource {
        // SAFETY: the host is live, and the call gives back a hold on it of its own.
        FrameSource {
            source: unsafe { pvg_frame_source_new(self.host.as_ptr()) },
            _memory: self._memory.clone(),
        }
    }

    /// Lets the framework's objects go.
    pub fn free(self) {
        // SAFETY: the host is live and is not used again: `self` ends here.
        unsafe { pvg_host_free(self.host.as_ptr()) };
    }

    /// Keeps the framework's objects for as long as the process lives, with the memory and the
    /// sink they use. For a device that letting go would end the process.
    pub fn leak(self) {
        std::mem::forget(self);
    }
}
