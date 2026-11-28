// SPDX-License-Identifier: GPL-2.0-or-later
//! A reader of the program's frames, as the Guesthouse app and rmhost are: rmframes' own reading
//! side, built into the crate's archive, driven from Rust for the tests beside it.
//!
//! A reader either gives every frame back as soon as it comes, as an encoder that keeps up does,
//! or holds what it gets until the test lets go, as a slow one does.
//!
//! `colour` looks at a slot's first pixel to see the guest's colour reached it. That one read
//! locks the surface, which the program itself never does: it is the test's check, not a step
//! frames take.
#![allow(dead_code)]

// The reading side is in the library's archive; a test that names nothing of the library would
// not be linked with it.
use guesthouse_pvg_host as _;
use std::collections::HashSet;
use std::ffi::{CString, c_char, c_void};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

const LOCK_READ_ONLY: u32 = 1;
/// The colour bits of a pixel: the display may set the fourth byte as it likes.
pub const COLOUR_BITS: u32 = 0x00FF_FFFF;
const ERROR_ROOM: usize = 256;

/// `rmf_frame_msg` of `rmframes.h`: a Mach header of six words, then the frame's numbers.
#[repr(C)]
struct FrameMessage {
    header: [u32; 6],
    generation: u32,
    slot: u32,
    width: u32,
    height: u32,
    sequence: u64,
    host_time: u64,
}

/// `rmf_sub_callbacks` of `rmframes_sub.h`.
#[repr(C)]
struct Callbacks {
    context: *mut c_void,
    on_slots: extern "C" fn(*mut c_void, u32, u32, u32, u32, *const *mut c_void),
    on_frame: extern "C" fn(*mut c_void, *const FrameMessage),
    on_gone: extern "C" fn(*mut c_void),
    on_refused: extern "C" fn(*mut c_void, *const c_char),
}

unsafe extern "C" {
    fn rmf_sub_open(
        service: *const c_char,
        queue: *mut c_void,
        callbacks: Callbacks,
        error: *mut c_char,
        room: usize,
    ) -> *mut c_void;
    fn rmf_sub_release(sub: *mut c_void, generation: u32, slot: u32);
    fn rmf_sub_close(sub: *mut c_void);
    fn dispatch_queue_create(label: *const c_char, attributes: *const c_void) -> *mut c_void;
    fn dispatch_suspend(object: *mut c_void);
    fn dispatch_resume(object: *mut c_void);
    fn CFRetain(object: *mut c_void) -> *mut c_void;
    fn IOSurfaceLock(surface: *mut c_void, options: u32, seed: *mut u32) -> i32;
    fn IOSurfaceUnlock(surface: *mut c_void, options: u32, seed: *mut u32) -> i32;
    fn IOSurfaceGetBaseAddress(surface: *mut c_void) -> *const u32;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub generation: u32,
    pub slot: u32,
    pub sequence: u64,
}

/// What the reader has heard, as rmframes' queue tells it.
#[derive(Default)]
pub struct Heard {
    /// The reader's own handle, for giving frames back from the queue.
    sub: usize,
    /// Give every frame back as soon as it comes.
    pub prompt: bool,
    pub generation: u32,
    pub size: (u32, u32),
    /// The slots' surfaces, retained, and never let go: a test is short.
    surfaces: Vec<usize>,
    pub frames: Vec<Frame>,
    /// Slots of the current generation the reader holds.
    pub held: HashSet<u32>,
    /// A slot was handed out again while this reader held it.
    pub handed_twice: bool,
    pub gone: bool,
    pub refused: Option<String>,
}

impl Heard {
    /// How many slots the reader was given.
    pub fn surfaces_len(&self) -> usize {
        self.surfaces.len()
    }
}

type Shared = (Mutex<Heard>, Condvar);

fn shared(context: *mut c_void) -> &'static Shared {
    // SAFETY: the context is a `Shared` leaked in `Reader::open`, alive for the whole test.
    unsafe { &*context.cast::<Shared>() }
}

fn lock(shared: &Shared) -> std::sync::MutexGuard<'_, Heard> {
    shared.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

extern "C" fn on_slots(
    context: *mut c_void,
    generation: u32,
    width: u32,
    height: u32,
    count: u32,
    surfaces: *const *mut c_void,
) {
    let shared = shared(context);
    let mut heard = lock(shared);
    heard.generation = generation;
    heard.size = (width, height);
    heard.held.clear();
    // SAFETY: rmframes hands `count` live surfaces, valid through the call; each kept is retained.
    heard.surfaces =
        (0..count as usize).map(|index| unsafe { CFRetain(*surfaces.add(index)) } as usize).collect();
    shared.1.notify_all();
}

extern "C" fn on_frame(context: *mut c_void, message: *const FrameMessage) {
    // SAFETY: rmframes hands a whole frame message, valid through the call.
    let message = unsafe { &*message };
    let shared = shared(context);
    let mut heard = lock(shared);
    let frame = Frame { generation: message.generation, slot: message.slot, sequence: message.sequence };
    heard.frames.push(frame);
    if heard.prompt {
        // SAFETY: the reader is open: it is closed only after its queue has stopped calling.
        unsafe { rmf_sub_release(heard.sub as *mut c_void, frame.generation, frame.slot) };
    } else if message.generation == heard.generation && !heard.held.insert(message.slot) {
        heard.handed_twice = true;
    }
    shared.1.notify_all();
}

extern "C" fn on_gone(context: *mut c_void) {
    let shared = shared(context);
    lock(shared).gone = true;
    shared.1.notify_all();
}

extern "C" fn on_refused(context: *mut c_void, reason: *const c_char) {
    // SAFETY: rmframes hands a string that ends in a zero, valid through the call.
    let reason = unsafe { std::ffi::CStr::from_ptr(reason) }.to_string_lossy().into_owned();
    let shared = shared(context);
    lock(shared).refused = Some(reason);
    shared.1.notify_all();
}

/// A reader of the program's frames.
pub struct Reader {
    sub: *mut c_void,
    shared: &'static Shared,
}

impl Reader {
    /// Looks the publisher up and says hello. `prompt` gives every frame back as it comes.
    pub fn open(name: &str, prompt: bool) -> Self {
        let shared: &'static Shared = Box::leak(Box::default());
        lock(shared).prompt = prompt;
        let name = CString::new(name).expect("a name");
        let label = CString::new("pvg-host-test.reader").expect("a label");
        let mut error = [0 as c_char; ERROR_ROOM];
        let callbacks = Callbacks {
            context: std::ptr::from_ref(shared).cast_mut().cast(),
            on_slots,
            on_frame,
            on_gone,
            on_refused,
        };
        // SAFETY: the strings end in zeroes, the error buffer is as long as it says, and the
        // callbacks' context lives for the rest of the test. The queue waits until the reader's
        // handle is known, which a prompt reader's callbacks need.
        let sub = unsafe {
            let queue = dispatch_queue_create(label.as_ptr(), std::ptr::null());
            dispatch_suspend(queue);
            let sub = rmf_sub_open(name.as_ptr(), queue, callbacks, error.as_mut_ptr(), error.len());
            lock(shared).sub = sub as usize;
            dispatch_resume(queue);
            sub
        };
        // SAFETY: on failure the call wrote a string that ends in a zero.
        let why = unsafe { std::ffi::CStr::from_ptr(error.as_ptr()) };
        assert!(!sub.is_null(), "no reader: {}", why.to_string_lossy());
        Self { sub, shared }
    }

    /// Waits until what the reader heard passes `test`, and returns what `test` makes of it.
    pub fn wait<T>(&self, what: &str, patience: Duration, mut test: impl FnMut(&Heard) -> Option<T>) -> T {
        let since = Instant::now();
        let mut heard = lock(self.shared);
        loop {
            if let Some(found) = test(&heard) {
                return found;
            }
            let left = patience.checked_sub(since.elapsed()).unwrap_or_else(|| panic!("{what} never came"));
            heard =
                self.shared.1.wait_timeout(heard, left).unwrap_or_else(|poisoned| poisoned.into_inner()).0;
        }
    }

    /// How many frames the reader has had.
    pub fn count(&self) -> usize {
        self.heard(|heard| heard.frames.len())
    }

    /// The frame after the first `seen` the reader had.
    pub fn frame_after(&self, seen: usize, patience: Duration) -> Frame {
        self.wait("a frame", patience, |heard| heard.frames.get(seen).copied())
    }

    pub fn heard<T>(&self, look: impl FnOnce(&Heard) -> T) -> T {
        look(&lock(self.shared))
    }

    /// Gives back a frame the reader held.
    pub fn release(&self, frame: Frame) {
        lock(self.shared).held.remove(&frame.slot);
        // SAFETY: the reader is open.
        unsafe { rmf_sub_release(self.sub, frame.generation, frame.slot) };
    }

    /// Holds every frame that comes from now on, as a reader that has stopped keeping up.
    pub fn hold(&self) {
        let mut heard = lock(self.shared);
        heard.prompt = false;
        heard.held.clear();
    }

    /// The sequence numbers of the frames after the first `seen`.
    pub fn sequences_after(&self, seen: usize) -> Vec<u64> {
        self.heard(|heard| heard.frames[seen..].iter().map(|frame| frame.sequence).collect())
    }

    /// Gives back everything the reader holds, and gives every frame back from now on.
    pub fn let_go(&self) {
        let (generation, held) = {
            let mut heard = lock(self.shared);
            heard.prompt = true;
            (heard.generation, std::mem::take(&mut heard.held))
        };
        for slot in held {
            // SAFETY: the reader is open.
            unsafe { rmf_sub_release(self.sub, generation, slot) };
        }
    }

    /// The colour of the first pixel of a slot's surface.
    pub fn colour(&self, frame: Frame) -> u32 {
        let surface = self.heard(|heard| heard.surfaces[frame.slot as usize]) as *mut c_void;
        // SAFETY: the surface is retained by the reader; the lock is read-only and undone at
        // once, and a slot's surface holds at least one pixel.
        let pixel = unsafe {
            IOSurfaceLock(surface, LOCK_READ_ONLY, std::ptr::null_mut());
            let pixel = IOSurfaceGetBaseAddress(surface).read_volatile();
            IOSurfaceUnlock(surface, LOCK_READ_ONLY, std::ptr::null_mut());
            pixel
        };
        pixel & COLOUR_BITS
    }

    pub fn close(self) {
        // SAFETY: the reader is open, and is not used again.
        unsafe { rmf_sub_close(self.sub) };
    }
}
