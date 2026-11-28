// SPDX-License-Identifier: GPL-2.0-or-later
//! The process's work: one guest's device, served to QEMU.
//!
//! One thread reads QEMU's requests and answers each before the next is read, as a register read
//! or write is one step for the guest's processor, which waits. Interrupts go to QEMU from
//! whatever thread the framework raises them on, often from inside a write that is being served,
//! so the socket's writing end has a lock.

use crate::export::{self, Shown};
use crate::guest_ram::{self, Shared};
use crate::picture::{self, ModeText};
use crate::watch::{self, DRAWING, SERVING, Step};
use crate::window;
use crate::wire::{self, Access, Answer, MESSAGE_BYTES, Request};
use guesthouse_pvg_sys::{Config, Device, Error, Event, FrameSource, Glyph, Mode, Source};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What the process is started with.
pub struct Options {
    /// Where QEMU's forwarding device connects.
    pub socket: PathBuf,
    /// Where a person or a script asks for a picture or sets the modes, if anywhere.
    pub control: Option<PathBuf>,
    pub name: String,
    pub modes: Vec<ModeText>,
    /// QEMU's monitor socket for the window's input and its close button, if the window has one.
    pub monitor: Option<PathBuf>,
    /// The bootstrap name to publish the guest's frames under, for other processes, if any.
    pub frames: Option<String>,
}

/// The first of each kind of line is worth reading. A guest can make any number.
const LINES_SHOWN: u64 = 32;
const MAPPER_VECTOR: u32 = 1;
const DEVICE_VECTOR: u32 = 0;
const MAPPER_WORD: u32 = 8;
const DEVICE_WORD: u32 = 4;
/// The guest's device stands still while a line is read, so whoever asks has to be quick.
const CONTROL_PATIENCE: Duration = Duration::from_millis(250);
/// A display the size of QEMU's own default, 20 inches.
const MILLIMETRES: (u32, u32) = (400, 300);

/// What the device has told, counted.
#[derive(Default)]
struct Told {
    interrupts: AtomicU64,
    frames: AtomicU64,
    /// How many of the guest's pictures were drawn into the window.
    drawn: AtomicU64,
    glyphs: AtomicU64,
    refused: AtomicU64,
    /// The picture's width above its height.
    size: AtomicU64,
}

fn note(count: &AtomicU64, line: impl FnOnce() -> String) {
    let seen = count.fetch_add(1, Ordering::Relaxed);
    if seen < LINES_SHOWN {
        eprintln!("pvg-host: {}", line());
    } else if seen == LINES_SHOWN {
        eprintln!("pvg-host: more of the kind, not shown");
    }
}

/// What the thread that draws into the window is told.
enum View {
    Frame,
    Size(u32, u32),
    CursorShown(bool),
    CursorMoved,
    CursorGlyph(Glyph),
    /// The guest's picture drawn into the window again, and what went into the window's layer
    /// written to a file.
    Shown(PathBuf),
    /// The process is ending: the thread lets go of the display's picture and ends.
    Stop,
}

/// The thread that draws into the window. Dropped before the device, it ends the thread first,
/// so that the display's picture, which keeps the framework's display alive, is let go while
/// the device's sink is still there: a display let go after it would raise an interrupt into a
/// sink that has gone.
struct Drawer {
    view: Sender<View>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// The frame export's thread, which also holds the display's picture: ended and joined before
/// the device drops, as the window's drawing thread is.
struct Exporter {
    shown: Sender<Shown>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Exporter {
    fn drop(&mut self) {
        let _ = self.shown.send(Shown::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Drawer {
    fn drop(&mut self) {
        let _ = self.view.send(View::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A window's smallest picture on a side, what a guest's desktop can still use (the window lets
/// itself be made no smaller), and its largest: the largest mode the device has been given so
/// far (5120x2880, by QEMU's window on 2026-11-27, with nothing from the Mac's kernel). A larger
/// window shows that picture scaled.
const WINDOW_SMALLEST: u32 = 320;
const WINDOW_LARGEST: (u32, u32) = (5120, 2880);
const WINDOW_HERTZ: u16 = 60;
/// A window's mode is a whole number of these pixels wide. The display shows a picture whose
/// width is no multiple of 4 sheared (seen on macOS 27.2.1 by our Linux guest driver's tests,
/// on QEMU's device too); 8 keeps clear of that with room. The height is the window's own.
const WIDTH_STEP: u32 = 8;
/// How long a window's size has to stay the same before the guest is given it. A drag, a
/// screen change or full screen brings several sizes in a row, and each mode list the guest
/// takes may have it make its desktop's output again; only the last size counts.
const SETTLE: Duration = Duration::from_millis(300);

/// Draws the guest's pictures into the window, and keeps the cursor's layer where the guest's
/// cursor is, until the device has gone. A guest may show pictures faster than a screen takes
/// them: of those that have piled up only the last is drawn.
fn draw(source: &FrameSource, window: &window::Handle, events: &Receiver<View>, told: &Told) {
    let (mut shown, mut next) = (false, None);
    loop {
        DRAWING.set(Step::Waiting);
        let Some(event) = next.take().or_else(|| events.recv().ok()) else {
            return;
        };
        match event {
            View::Frame => {
                loop {
                    match events.try_recv() {
                        Ok(View::Frame) => {}
                        Ok(other) => {
                            next = Some(other);
                            break;
                        }
                        Err(_) => break,
                    }
                }
                DRAWING.set(Step::Draw);
                // SAFETY: the layer is the open window's, set up for this source in `run`.
                if unsafe { source.draw(window.layer()) } {
                    told.drawn.fetch_add(1, Ordering::Relaxed);
                }
            }
            View::Shown(path) => {
                DRAWING.set(Step::Shown);
                // SAFETY: the layer is the open window's, set up for this source in `run`.
                let copy = unsafe { source.draw_copy(window.layer()) };
                let written = match copy {
                    Some((width, height, pixels)) => save_pixels(&path, width, height, &pixels).map(|()| {
                        format!("the window's layer, {width} by {height}, is in {}", path.display())
                    }),
                    None => Err("the window's layer could not be copied: no picture yet".to_owned()),
                };
                eprintln!("pvg-host: {}", written.unwrap_or_else(|why| why));
            }
            View::Stop => return,
            View::Size(width, height) => {
                DRAWING.set(Step::Tell);
                window.picture_size(width, height);
            }
            View::CursorGlyph(glyph) => {
                DRAWING.set(Step::Tell);
                window.glyph(&glyph);
            }
            View::CursorShown(_) | View::CursorMoved => {
                DRAWING.set(Step::Tell);
                if let View::CursorShown(now) = event {
                    shown = now;
                }
                let (x, y) = source.cursor();
                window.cursor(x, y, shown);
            }
        }
    }
}

/// The mode list for a window whose content is this many pixels: that size, as the one a guest
/// should choose, and behind it those of the list the process was started with that are no
/// larger. A guest's driver may put the largest mode first whatever the order here, so no mode
/// is offered that the window would have to scale down. The width is rounded down to
/// [`WIDTH_STEP`]; the window shows the picture one to one with a bar of a few pixels beside it.
fn modes_for_window(width: u32, height: u32, list: &[ModeText]) -> Vec<Mode> {
    let first = Mode {
        width: (width.clamp(WINDOW_SMALLEST, WINDOW_LARGEST.0) / WIDTH_STEP * WIDTH_STEP) as u16,
        height: height.clamp(WINDOW_SMALLEST, WINDOW_LARGEST.1) as u16,
        hertz: WINDOW_HERTZ,
    };
    let rest = modes(list).into_iter().filter(|mode| {
        mode.width <= first.width
            && mode.height <= first.height
            && (mode.width, mode.height) != (first.width, first.height)
    });
    std::iter::once(first).chain(rest).collect()
}

fn modes(list: &[ModeText]) -> Vec<Mode> {
    list.iter().map(|&(width, height, hertz)| Mode { width, height, hertz }).collect()
}

/// Modes as the control socket writes them: `1920x1080@60,1280x800@60`.
fn modes_text(list: &[Mode]) -> String {
    let each: Vec<_> =
        list.iter().map(|mode| format!("{}x{}@{}", mode.width, mode.height, mode.hertz)).collect();
    each.join(",")
}

/// The mode list the display was last handed, by the window or the control socket.
type Offered = Arc<Mutex<Option<Vec<Mode>>>>;

/// Calls what waits for a line, if nothing has yet.
fn say<F: FnOnce(String)>(then: &Mutex<Option<F>>, line: String) {
    let once = then.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
    if let Some(then) = once {
        then(line);
    }
}

/// Hands the display a mode list, unless it is the one it has, and returns at once: the
/// framework's setter runs on a queue of the host's own, never on the thread that serves the
/// guest. `then` hears, from that queue or from here, `ok modes LIST` once the framework has
/// taken the list, `ok modes LIST, offered already` for the list it has, or `error WHY`.
fn offer(
    device: &mut Device,
    list: Vec<Mode>,
    offered: &Offered,
    then: impl FnOnce(String) + Send + 'static,
) {
    // The answer the Mac app expects first, `ok N modes`, and what was offered after it.
    let text = format!("{} modes: {}", list.len(), modes_text(&list));
    let mut last = offered.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if last.as_ref() == Some(&list) {
        drop(last);
        return then(format!("ok {text}, offered already"));
    }
    // `then` is the framework's to call, unless the list never reaches it.
    let then = Arc::new(Mutex::new(Some(then)));
    watch::modes_queued();
    let (again, taken, heard) = (Arc::clone(offered), list.clone(), Arc::clone(&then));
    let done = Box::new(move |was_taken: bool, took: Duration| {
        watch::modes_done();
        if was_taken {
            say(&heard, format!("ok {text} ({} ms)", took.as_millis()));
        } else {
            let mut last = again.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if last.as_ref() == Some(&taken) {
                *last = None;
            }
            drop(last);
            say(&heard, "error the framework threw on the mode list".to_owned());
        }
    });
    match device.set_modes(&list, done) {
        Ok(()) => *last = Some(list),
        Err(error) => {
            watch::modes_done();
            drop(last);
            say(&then, format!("error {error}"));
        }
    }
}

fn send(to_qemu: &Mutex<UnixStream>, answer: Answer) -> io::Result<()> {
    let mut stream = to_qemu.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    stream.write_all(&answer.write())
}

/// Binds a socket in the place of any file there, one only this process's user may connect to,
/// whatever the folder's mode: QEMU's and the control socket both. A socket
/// takes its mode from the umask at the bind, so the mask is narrowed for the bind and the mode
/// set after: the socket is never open to others. The mask is the process's; a file another
/// thread makes meanwhile is made private too, never more open.
fn bind_private(path: &Path) -> io::Result<UnixListener> {
    let _ = fs::remove_file(path);
    // SAFETY: umask only swaps the process's mask, and the old one is put back.
    let before = unsafe { libc::umask(0o077) };
    let bound = UnixListener::bind(path);
    unsafe { libc::umask(before) };
    let listener = bound?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

/// Waits for QEMU, takes the guest's memory from it, and makes the device over it.
fn greet(
    options: &Options,
    told: &Arc<Told>,
    view: Option<Sender<View>>,
    export: Option<Sender<Shown>>,
) -> io::Result<(UnixStream, Arc<Mutex<UnixStream>>, Device)> {
    let listener = bind_private(&options.socket)?;
    eprintln!("pvg-host: waiting for QEMU on {}", options.socket.display());
    let (mut from_qemu, _) = listener.accept()?;
    // One guest, one QEMU: nobody else has business here.
    drop(listener);
    let _ = fs::remove_file(&options.socket);

    let (message, _) = guest_ram::receive(&mut from_qemu).map_err(|error| match error.kind() {
        // QEMU's device says no before its greeting if the machine's RAM is not shared memory.
        io::ErrorKind::UnexpectedEof => {
            io::Error::other("QEMU went before it had said what the guest's memory is")
        }
        _ => error,
    })?;
    let Some(Request::Hello { protocol: wire::PROTOCOL, ranges }) = Request::read(&message) else {
        return Err(io::Error::other("QEMU's first message is no greeting this process knows"));
    };
    if ranges == 0 || ranges > wire::MOST_RANGES {
        return Err(io::Error::other(format!("QEMU names {ranges} ranges of guest memory")));
    }
    let mut shared = Vec::new();
    for _ in 0..ranges {
        let (message, file) = guest_ram::receive(&mut from_qemu)?;
        let (Some(Request::Range { guest_address, length, file_offset }), Some(file)) =
            (Request::read(&message), file)
        else {
            return Err(io::Error::other("a range of guest memory came without its numbers or its file"));
        };
        eprintln!("pvg-host: guest memory at {guest_address:#x}, {length:#x} bytes");
        shared.push(Shared { guest_address, length, file_offset, file });
    }
    let memory = guest_ram::map(shared)?;

    let to_qemu = Arc::new(Mutex::new(from_qemu.try_clone()?));
    let interrupts = Arc::clone(&to_qemu);
    let heard = Arc::clone(told);
    // The window's thread hears of pictures and of the cursor. If it has gone, nobody does.
    let view = view.map(Mutex::new);
    let show = move |event: View| {
        if let Some(view) = &view {
            let _ = view.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).send(event);
        }
    };
    // The frame export's thread hears of pictures and their size, the same way.
    let export = export.map(Mutex::new);
    let publish = move |event: Shown| {
        if let Some(export) = &export {
            let _ = export.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).send(event);
        }
    };
    let config = Config {
        name: options.name.clone(),
        millimetres: MILLIMETRES,
        serial: 1,
        modes: modes(&options.modes),
    };
    let device = Device::new(memory, &config, move |event| match event {
        Event::Interrupt { source, vector: _ } => {
            heard.interrupts.fetch_add(1, Ordering::Relaxed);
            // The guest's function has one vector for each of the two devices.
            let vector = if source == Source::Mapper { MAPPER_VECTOR } else { DEVICE_VECTOR };
            // If QEMU has gone, the reading end finds out and ends the process.
            let _ = send(&interrupts, Answer::Interrupt(vector));
        }
        Event::ModeChanged { width, height, format } => {
            heard.size.store(u64::from(width) << 32 | u64::from(height), Ordering::Relaxed);
            eprintln!("pvg-host: the guest's picture is {width} by {height}, format {format:#x}");
            show(View::Size(width, height));
            publish(Shown::Size(width, height));
        }
        Event::NewFrame => {
            heard.frames.fetch_add(1, Ordering::Relaxed);
            show(View::Frame);
            publish(Shown::Frame);
        }
        Event::CursorShown(shown) => show(View::CursorShown(shown)),
        Event::CursorMoved => show(View::CursorMoved),
        Event::CursorGlyph(glyph) => {
            note(&heard.glyphs, || format!("the cursor's picture is {} by {}", glyph.width, glyph.height));
            show(View::CursorGlyph(glyph));
        }
        Event::Refused(refusal) => note(&heard.refused, || format!("not passed on: {refusal}")),
    })
    .map_err(io::Error::other)?;
    send(
        &to_qemu,
        Answer::Ready { device_bytes: device.register_bytes(), mapper_bytes: device.mapper_register_bytes() },
    )?;
    Ok((from_qemu, to_qemu, device))
}

/// Does one register read for the guest. A read of a width the registers don't have is zero.
fn read(device: &mut Device, access: Access) -> u64 {
    match (access.bar, access.size) {
        (wire::BAR_DEVICE, DEVICE_WORD) => u64::from(device.read(access.offset)),
        (wire::BAR_MAPPER, MAPPER_WORD) => device.mapper_read(access.offset),
        // Half of one of the mapper's registers, the lower half first.
        (wire::BAR_MAPPER, DEVICE_WORD) => {
            let half = access.offset % u64::from(MAPPER_WORD);
            device.mapper_read(access.offset - half) >> (half * 8) & u64::from(u32::MAX)
        }
        _ => 0,
    }
}

fn write(device: &mut Device, access: Access, value: u64, odd: &AtomicU64) {
    match (access.bar, access.size) {
        (wire::BAR_DEVICE, DEVICE_WORD) => device.write(access.offset, value as u32),
        (wire::BAR_MAPPER, MAPPER_WORD) => device.mapper_write(access.offset, value),
        // Half a register written is not something a driver does, and what the mapper would make
        // of it is not known.
        _ => note(odd, || format!("not passed on: a write of {} bytes to BAR {}", access.size, access.bar)),
    }
}

/// Writes a picture to a PPM file, whole or not at all.
fn save_pixels(path: &Path, width: u32, height: u32, pixels: &[u8]) -> Result<(), String> {
    let partial = path.with_extension("partial");
    let written = File::create(&partial).and_then(|file| {
        let mut file = BufWriter::new(file);
        picture::write_ppm(&mut file, width, height, pixels)?;
        file.flush()?;
        fs::rename(&partial, path)
    });
    written.map_err(|error| format!("{}: {error}", path.display()))
}

/// Writes the display's picture to a file, whole or not at all.
fn save_picture(device: &mut Device, path: &Path) -> Result<String, String> {
    let frame = device.frame().map_err(|error| error.to_string())?;
    let Some(frame) = frame else {
        return Ok("none: the guest has shown no picture yet".to_owned());
    };
    save_pixels(path, frame.width, frame.height, &frame.pixels)?;
    Ok(format!("ok {} {}", frame.width, frame.height))
}

/// What the control socket reports on besides the device: the window and the frame export, if
/// the process has them.
#[derive(Clone, Copy)]
struct Extras<'a> {
    window: Option<&'a window::Handle>,
    view: Option<&'a Sender<View>>,
    export: Option<&'a export::Counts>,
}

/// Answers one line from the control socket.
fn control(device: &mut Device, told: &Told, extras: Extras, line: &str) -> String {
    let (window, view) = (extras.window, extras.view);
    let (word, rest) = line.trim().split_once(' ').unwrap_or((line.trim(), ""));
    let answer = match word {
        "frame" if !rest.is_empty() => save_picture(device, Path::new(rest)),
        // What the window shows, from its own layer, written by the thread that draws into it a
        // moment later.
        "shown" if !rest.is_empty() => match view {
            None => Err("the process was started with no window".to_owned()),
            Some(view) => view
                .send(View::Shown(PathBuf::from(rest)))
                .map(|()| "ok, written when the window next draws".to_owned())
                .map_err(|_| "the window draws no more".to_owned()),
        },
        // The window's content gets this size, as if a user had dragged it. The mode list
        // follows from the window's own word that it was resized.
        "window" => match (window, picture::read_modes(&format!("{rest}@60")).ok().as_deref()) {
            (None, _) => Err("the process was started with no window".to_owned()),
            (Some(window), Some(&[(width, height, _)])) => {
                window.resize(width.into(), height.into());
                Ok(format!("ok {width} by {height}"))
            }
            _ => Err("a window's size is written as WxH".to_owned()),
        },
        // A made-up event for the window's input, for runs with no person.
        "input" => match (window, window::TestEvent::read(rest)) {
            (None, _) => Err("the process was started with no window".to_owned()),
            (Some(window), Ok(event)) => {
                window.test_event(event);
                Ok("ok".to_owned())
            }
            (_, Err(why)) => Err(why),
        },
        // The window's close button, in any mode: the guest is asked to power down.
        "close" => match window {
            None => Err("the process was started with no window".to_owned()),
            Some(window) => {
                window.close();
                Ok("ok".to_owned())
            }
        },
        "fullscreen" => match window {
            None => Err("the process was started with no window".to_owned()),
            Some(window) => {
                window.full_screen();
                Ok("ok".to_owned())
            }
        },
        "status" => {
            let size = told.size.load(Ordering::Relaxed);
            let (by_guest, by_host) = device.presents();
            let window = window.map_or(String::new(), |window| {
                SERVING.set(Step::WindowState);
                let (width, height, visible, cursor) = window.state();
                SERVING.set(Step::Control);
                let input = window::input_status().map_or(String::new(), |input| format!(" {input}"));
                format!(" window {width}x{height} seen {visible} cursor {cursor}{input}")
            });
            let export = extras.export.map_or(String::new(), export::Counts::line);
            Ok(format!(
                "ok picture {}x{} presented {by_guest} read {by_host} frames {} drawn {} interrupts {} refused {} spoiled {}{window}{export}",
                size >> 32,
                size & u64::from(u32::MAX),
                told.frames.load(Ordering::Relaxed),
                told.drawn.load(Ordering::Relaxed),
                told.interrupts.load(Ordering::Relaxed),
                told.refused.load(Ordering::Relaxed),
                device.spoiled(),
            ))
        }
        // Without a window, the words the app and the scripts know, as they know them.
        _ if window.is_none() => {
            Err("the commands are: frame PATH, modes WxH@R[,WxH@R...], window WxH, status".to_owned())
        }
        _ => Err(
            "the commands are: frame PATH, shown PATH, modes WxH@R[,WxH@R...], window WxH, fullscreen, close, input EVENT, status"
                .to_owned(),
        ),
    };
    answer.unwrap_or_else(|why| format!("error {why}"))
}

/// The mode list for a control socket's `modes`: one mode is a size, as a window's would be,
/// offered first with the started list's smaller modes behind it; several are the list itself.
fn modes_asked(rest: &str, started: &[ModeText]) -> Result<Vec<Mode>, String> {
    match picture::read_modes(rest)?.as_slice() {
        &[(width, height, hertz)] => {
            let mut list = modes_for_window(width.into(), height.into(), started);
            list[0].hertz = hertz;
            Ok(list)
        }
        list => Ok(modes(list)),
    }
}

fn serve_control(
    device: &mut Device,
    told: &Told,
    extras: Extras,
    listener: &UnixListener,
    (started, offered): (&[ModeText], &Offered),
) {
    let Ok((stream, _)) = listener.accept() else {
        return;
    };
    let _ = stream.set_read_timeout(Some(CONTROL_PATIENCE));
    let _ = stream.set_write_timeout(Some(CONTROL_PATIENCE));
    let mut line = String::new();
    SERVING.set(Step::Control);
    if BufReader::new(&stream).read_line(&mut line).is_err() || line.trim().is_empty() {
        return;
    }
    match line.trim().split_once(' ').unwrap_or((line.trim(), "")) {
        // The answer comes once the framework has taken the list, from the queue it is set on.
        ("modes", rest) => match modes_asked(rest, started) {
            Ok(list) => offer(device, list, offered, move |answer| {
                let _ = writeln!(&stream, "{answer}");
            }),
            Err(why) => {
                let _ = writeln!(&stream, "error {why}");
            }
        },
        _ => {
            let answer = control(device, told, extras, &line);
            let _ = writeln!(&stream, "{answer}");
        }
    }
}

/// Runs until QEMU goes, which is a guest's ordinary end. With a window, the guest's pictures
/// are drawn into it from a thread of their own, and the display's mode list follows the
/// window's size. With a frame name, they are published under it from another thread.
pub fn run(options: &Options, window: Option<&Arc<window::Handle>>) -> io::Result<()> {
    guesthouse_pvg_sys::watch_reasons({
        let seen = AtomicU64::new(0);
        move |reason| note(&seen, || format!("the device refused: {reason}"))
    });
    let told = Arc::new(Told::default());
    // The name first: one another process has already is a mistake to hear of before QEMU starts.
    let publisher =
        options.frames.as_deref().map(export::Publisher::open).transpose().map_err(io::Error::other)?;
    if let Some(name) = &options.frames {
        eprintln!("pvg-host: frames are published as {name}");
    }
    let (view, viewed) = mpsc::channel();
    let to_view = window.map(|_| view.clone());
    let (to_export, exported) = mpsc::channel();
    let stop_export = to_export.clone();
    let (mut from_qemu, to_qemu, mut device) =
        greet(options, &told, window.map(|_| view), publisher.as_ref().map(|_| to_export))?;
    eprintln!("pvg-host: the device is made, {} bytes of registers", device.register_bytes());
    // Declared after the device, so dropped before it.
    let (export_counts, _exporter) = match publisher {
        Some(publisher) => {
            let (counts, thread) = export::start(publisher, device.frame_source(), exported);
            (Some(counts), Some(Exporter { shown: stop_export, thread: Some(thread) }))
        }
        None => (None, None),
    };
    let state = device.state_look();
    let heard = Arc::clone(&told);
    let device_look = Box::new(move || {
        let (interrupts, frames) =
            (heard.interrupts.load(Ordering::Relaxed), heard.frames.load(Ordering::Relaxed));
        format!("interrupts {interrupts}, frames {frames}, {}", state())
    });
    watch::start(window.map(|_| window::look_main as fn() -> watch::Main), device_look);
    // QEMU is up, and so is its monitor socket for the window.
    if let (Some(path), Some(_)) = (&options.monitor, window) {
        crate::input::connect_later(path.clone(), window::input_connected);
    }
    // Declared after the device, so dropped before it.
    let _drawer = match (window, &to_view) {
        (Some(window), Some(view)) => {
            let source = device.frame_source();
            // SAFETY: the layer is the open window's, which lives as long as the process.
            unsafe { source.set_up(window.layer()) };
            let (window, told) = (Arc::clone(window), Arc::clone(&told));
            let thread = std::thread::spawn(move || draw(&source, &window, &viewed, &told));
            Some(Drawer { view: view.clone(), thread: Some(thread) })
        }
        _ => None,
    };

    let control_listener = match &options.control {
        Some(path) => Some(bind_private(path)?),
        None => None,
    };
    let odd_writes = AtomicU64::new(0);
    // The list the device was made with is the one it has.
    let offered: Offered = Arc::new(Mutex::new(Some(modes(&options.modes))));
    let mut window_sized = false;
    // A size the window has had since `Instant`, not given to the guest yet.
    let mut settling: Option<((u32, u32), Instant)> = None;
    let mut waiting = [
        libc::pollfd { fd: from_qemu.as_raw_fd(), events: libc::POLLIN, revents: 0 },
        libc::pollfd {
            fd: control_listener.as_ref().map_or(-1, |listener| listener.as_raw_fd()),
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: window.map_or(-1, |window| window.resized_fd()),
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    loop {
        let timeout = settling.map_or(-1, |(_, since)| {
            SETTLE.saturating_sub(since.elapsed()).as_millis().min(i32::MAX as u128) as i32 + 1
        });
        SERVING.set(Step::Waiting);
        // SAFETY: the array holds its entries for as long as the call runs, and a descriptor of
        // -1 is one `poll` skips.
        let ready = unsafe { libc::poll(waiting.as_mut_ptr(), waiting.len() as _, timeout) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if waiting[1].revents != 0
            && let Some(listener) = &control_listener
        {
            let extras = Extras {
                window: window.map(|window| &**window),
                view: to_view.as_ref(),
                export: export_counts.as_deref(),
            };
            let modes = (options.modes.as_slice(), &offered);
            serve_control(&mut device, &told, extras, listener, modes);
        }
        if waiting[2].revents != 0
            && let Some(size) = window.and_then(|window| window.take_size())
        {
            // The window's first size goes at once: the guest's driver reads its first list
            // while it starts.
            let now = Instant::now();
            let since = if window_sized { now } else { now.checked_sub(SETTLE).unwrap_or(now) };
            window_sized = true;
            settling = Some((size, since));
        }
        if let Some(((width, height), since)) = settling
            && since.elapsed() >= SETTLE
        {
            settling = None;
            let list = modes_for_window(width, height, &options.modes);
            // A window that ends at a size whose list the guest has, as when it opens on a
            // screen and is then told of that screen, changes nothing for the guest.
            let (count, largest) = (list.len(), list[0]);
            offer(&mut device, list, &offered, move |answer| {
                if !answer.ends_with("offered already") {
                    let size = format!(
                        "the window is {width} by {height}: {count} modes, the largest {} by {}",
                        largest.width, largest.height
                    );
                    eprintln!("pvg-host: {size}: {answer}");
                }
            });
        }
        if waiting[0].revents == 0 {
            continue;
        }

        let mut message = [0u8; MESSAGE_BYTES];
        match from_qemu.read_exact(&mut message) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                eprintln!("pvg-host: QEMU has gone");
                return Ok(());
            }
            Err(error) => return Err(error),
        }
        let value = match Request::read(&message) {
            Some(Request::Read(access)) => {
                SERVING.set(Step::Read);
                read(&mut device, access)
            }
            Some(Request::Write(access, value)) => {
                SERVING.set(Step::Write);
                write(&mut device, access, value, &odd_writes);
                0
            }
            Some(Request::Reset) => {
                SERVING.set(Step::Reset);
                match device.reset() {
                    Ok(()) => 0,
                    // The device can't be reset any more, and a process has room for one device.
                    // The guest's graphics end here; leaving without letting the device go is the
                    // one way out that doesn't end in the framework's own assertion.
                    Err(Error::Spoiled) => {
                        eprintln!(
                            "pvg-host: the guest left the display half handed over, and the device can't be reset"
                        );
                        std::mem::forget(device);
                        std::process::exit(1);
                    }
                    Err(error) => return Err(io::Error::other(error)),
                }
            }
            _ => return Err(io::Error::other("QEMU sent a message this process doesn't know")),
        };
        send(&to_qemu, Answer::Reply(value))?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sizes(list: &[Mode]) -> Vec<(u16, u16)> {
        list.iter().map(|mode| (mode.width, mode.height)).collect()
    }

    #[test]
    fn one_mode_on_the_control_socket_is_a_size_and_several_are_the_list() {
        let started = [(2560, 1440, 60), (1920, 1080, 60), (1280, 800, 60)];
        let list = modes_asked("1510x731@60", &started).expect("one mode");
        assert_eq!(sizes(&list), [(1504, 731)]);
        let list = modes_asked("1920x1200@50", &started).expect("one mode");
        assert_eq!(sizes(&list), [(1920, 1200), (1920, 1080), (1280, 800)]);
        assert_eq!(list[0].hertz, 50);
        assert_eq!(list[1].hertz, 60);
        let list = modes_asked("1024x768@60,640x480@60", &started).expect("a list");
        assert_eq!(sizes(&list), [(1024, 768), (640, 480)]);
        assert!(modes_asked("many", &started).is_err());
        assert_eq!(modes_text(&list), "1024x768@60,640x480@60");
    }

    #[test]
    fn a_window_mode_is_a_multiple_of_eight_wide_and_as_high_as_the_window() {
        let list = [(2560, 1440, 60), (1920, 1080, 60), (1280, 800, 60)];
        // The sizes a person made on 2026-11-27, in pixels of a screen of scale 1.
        assert_eq!(sizes(&modes_for_window(1510, 731, &list)), [(1504, 731)]);
        assert_eq!(sizes(&modes_for_window(1782, 832, &list)), [(1776, 832), (1280, 800)]);
        assert_eq!(sizes(&modes_for_window(689, 456, &list)), [(688, 456)]);
        assert_eq!(sizes(&modes_for_window(1316, 852, &list)), [(1312, 852), (1280, 800)]);
        // A window the size of a listed mode has it once, first.
        assert_eq!(sizes(&modes_for_window(1920, 1080, &list)), [(1920, 1080), (1280, 800)]);
        // Too small or too large is held to what a desktop and the display take.
        assert_eq!(sizes(&modes_for_window(100, 50, &list)), [(320, 320)]);
        assert_eq!(
            sizes(&modes_for_window(9000, 9000, &list)),
            [(5120, 2880), (2560, 1440), (1920, 1080), (1280, 800)]
        );
        assert_eq!(
            sizes(&modes_for_window(3440, 1440, &list)),
            [(3440, 1440), (2560, 1440), (1920, 1080), (1280, 800)]
        );
        assert_eq!(sizes(&modes_for_window(6016, 3384, &list))[0], (5120, 2880));
        for mode in modes_for_window(1511, 733, &list) {
            assert_eq!(mode.hertz, 60);
            assert_eq!(mode.width % 8, 0);
        }
    }

    #[test]
    fn a_socket_is_its_users_alone_and_the_mask_is_put_back() {
        let folder = std::env::temp_dir().join(format!("pvg-host-mode-{}", std::process::id()));
        fs::create_dir_all(&folder).expect("a folder");
        let path = folder.join("s.sock");
        fs::write(&path, b"an old file").expect("an old file");
        // SAFETY: as in bind_private; no other test here binds or sets the mask.
        let before = unsafe { libc::umask(0o022) };
        let listener = bind_private(&path).expect("bound");
        // SAFETY: the same.
        let after = unsafe { libc::umask(before) };
        assert_eq!(after, 0o022, "the mask is the one before the bind");
        let mode = fs::metadata(&path).expect("the socket").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "{mode:o}");
        UnixStream::connect(&path).expect("its user connects");
        drop(listener);
        fs::remove_dir_all(&folder).expect("cleaned up");
    }
}
