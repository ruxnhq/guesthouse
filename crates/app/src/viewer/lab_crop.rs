//! Lab only: a window that shows one rectangle of a running machine's screen, and nothing else.
//!
//! It's the first piece of showing one of the guest's programs as a window of its own. The window is
//! the rectangle at its own size with no title bar of the host's: the program's own title bar, in
//! the picture, is the window's, and its buttons act on the program. The pointer lands where it
//! points in the guest and keys go as they do in the viewer. Nothing else of the viewer comes along.
//! There is no pill, no capture, no shared clipboard or file drops, and the guest's screen keeps its
//! size, as no `WindowSize` is ever made here: a window the size of one program must not become the
//! size of the guest's desktop.
//!
//! It's a process of its own, with an application id of its own, so the app's window can be open
//! beside it. The daemon serves one viewer and QEMU one keyboard and mouse, though, so the machine's
//! own screen in the app has to be closed first.

use super::crop::Crop;
use super::frames::FrameRoute;
use super::overlay::OverlayChoice;
use super::screen::Screen;
use super::title_drag::DragStrip;
use crate::application::{APP_ID, use_built_in_icons};
use adw::prelude::*;
use gtk::{gio, glib};
use guesthouse_app_core::context::Context;
use guesthouse_app_core::host_sensors::sensors_file;
use guesthouse_app_core::input::INPUT_SOCKET;
use guesthouse_app_core::log::{log_panics, write_log};
use guesthouse_app_core::machine::{Machine, load_all};
use guesthouse_app_core::screen_gpu::memory_frames;
use guesthouse_app_core::tasks::slot_of;
use guesthouse_frame_socket::SOCKET_NAME as FRAMES_SOCKET;
use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// The first argument that asks for this window. Nothing a user or the desktop passes looks like it.
pub const ARGUMENT: &str = "--lab-crop-window";
const FOLLOW_OPTION: &str = "--follow";
const DRAG_OPTION: &str = "--drag";
const USAGE: &str =
    "Usage: guesthouse --lab-crop-window NAME X Y W H TITLE [--follow FILE] [--drag HEIGHT RIGHT]

NAME is the machine's name or id, and X Y W H the rectangle of its screen to show, in the guest's
pixels. TITLE names the window in the taskbar.

--drag HEIGHT RIGHT: a press on the top HEIGHT pixels of the rectangle, short of RIGHT pixels at its
right, moves the window once dragged, as the program's title bar does in the guest. RIGHT leaves its
caption buttons to the guest. Calculator's at 100% are --drag 32 138.

--follow FILE: FILE is read again whenever it changes. Replace it by a rename, so it's never read half
written. It holds one of:
  X Y W H     the guest's window is there now. The rectangle moves, and the window takes its size.
  minimized   the guest's window is minimized, and so this one minimizes itself.
  closed      the guest's window is gone. This one closes, and the program ends with 0.
The window writes FILE.host, by a rename too, when the user does something the guest must follow:
  restored    the user brought this window back after the guest's was minimized.
  closed      the user closed this window.";
/// How often the guest window's file is looked at. A program moved in the guest can wait that long.
const FOLLOW_EVERY: Duration = Duration::from_millis(500);
/// Another id than the app's, or a running app would be asked to activate and this process would end.
const CROP_APP_ID_SUFFIX: &str = ".LabCrop";
/// What this window writes for the caller, beside the file it reads.
const HOST_FILE_SUFFIX: &str = "host";
const WRITING_SUFFIX: &str = "part";
const MINIMIZED: &str = "minimized";
const CLOSED: &str = "closed";
const RESTORED: &str = "restored";
/// Linux's numbers for the signals that end the window: a terminal's Ctrl+C, `kill`, and a terminal
/// closing.
const SIGHUP: i32 = 1;
const SIGINT: i32 = 2;
const SIGTERM: i32 = 15;
const SIGNALS: [i32; 3] = [SIGINT, SIGTERM, SIGHUP];

/// What was asked for on the command line.
#[derive(Debug, PartialEq, Eq)]
pub struct Request {
    pub machine: String,
    pub crop: Crop,
    pub title: String,
    /// The file that says where the guest's window is, and what has become of it.
    pub follow: Option<PathBuf>,
    pub drag: Option<DragStrip>,
}

/// The request in these arguments, the program's name left out. `None` if they don't ask for this
/// window at all, and the usage if they ask for it wrongly.
pub fn request(arguments: &[String]) -> Option<Result<Request, String>> {
    let (first, rest) = arguments.split_first()?;
    if first != ARGUMENT {
        return None;
    }
    Some(read_request(rest).ok_or_else(|| USAGE.to_owned()))
}

fn read_request(arguments: &[String]) -> Option<Request> {
    let [machine, x, y, width, height, title, options @ ..] = arguments else { return None };
    let crop = Crop::parse(&[x, y, width, height].map(String::as_str).join(" "))?;
    let mut request =
        Request { machine: machine.clone(), crop, title: title.clone(), follow: None, drag: None };
    let mut options = options.iter();
    while let Some(option) = options.next() {
        match option.as_str() {
            FOLLOW_OPTION if request.follow.is_none() => request.follow = Some(options.next()?.into()),
            DRAG_OPTION if request.drag.is_none() => {
                let height = options.next()?.parse().ok().filter(|height| *height > 0)?;
                let right = options.next()?.parse().ok().filter(|right| *right >= 0)?;
                request.drag = Some(DragStrip { height, right });
            }
            _ => return None,
        }
    }
    Some(request)
}

/// What the guest window's file says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GuestWindow {
    At(Crop),
    Minimized,
    Closed,
}

fn read_guest_window(text: &str) -> Option<GuestWindow> {
    match text.trim() {
        MINIMIZED => Some(GuestWindow::Minimized),
        CLOSED => Some(GuestWindow::Closed),
        rectangle => Crop::parse(rectangle).map(GuestWindow::At),
    }
}

/// Where this window is, as far as following the guest's window goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shown {
    Up,
    /// Asked to minimize, and not yet seen to lose the focus.
    Minimizing,
    /// Minimized for the guest. The focus coming back means the user brought it back.
    Minimized,
    /// Closing because the guest's window closed or the process was told to end, which the caller
    /// already knows.
    Ending,
}

/// The machine of this name, or of this id.
fn find_machine(context: &Context, wanted: &str) -> Result<Machine, String> {
    let (machines, _) = load_all(context);
    machines
        .into_iter()
        .find(|machine| machine.name == wanted || machine.id == wanted)
        .ok_or_else(|| format!("There is no machine called {wanted} in {}", context.paths.machines.display()))
}

pub fn run(request: Request) -> glib::ExitCode {
    let context = match Context::from_process() {
        Ok(context) => Arc::new(context),
        Err(error) => {
            eprintln!("{error}");
            return glib::ExitCode::FAILURE;
        }
    };
    log_panics(&context);
    let machine = match find_machine(&context, &request.machine) {
        Ok(machine) => machine,
        Err(text) => {
            eprintln!("{text}");
            return glib::ExitCode::FAILURE;
        }
    };
    let Some(slot) = slot_of(&context, &machine) else {
        eprintln!("{} isn't running", machine.name);
        return glib::ExitCode::FAILURE;
    };
    let run_folder = context.run_folder(Some(slot));
    // Said before GTK starts, so a lab can see the machine was found with no display to open.
    let there = |name: &str| if run_folder.join(name).exists() { "there" } else { "not there yet" };
    println!(
        "{} in {}: frames {}, input {}",
        machine.name,
        run_folder.display(),
        there(FRAMES_SOCKET),
        there(INPUT_SOCKET)
    );
    let application = adw::Application::builder()
        .application_id(format!("{APP_ID}{CROP_APP_ID_SUFFIX}"))
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let icons = context.paths.runtime.join("icons");
    application.connect_startup(move |_| use_built_in_icons(&icons));
    application.connect_activate(move |application| {
        open_window(application, &context, &machine, run_folder.clone(), &request);
    });
    application.run_with_args::<&str>(&[])
}

/// The window, on its way to the screen. Public for the page checks, which open one beside the app's.
pub fn open_window(
    application: &adw::Application,
    context: &Arc<Context>,
    machine: &Machine,
    run_folder: PathBuf,
    request: &Request,
) -> gtk::ApplicationWindow {
    let overlay = OverlayChoice { shown: false, sensors: sensors_file(context, machine) };
    let in_memory = machine.has_gpu() && memory_frames(context, machine);
    let logging = context.clone();
    let log = move |text: &str| {
        let _ = write_log(&logging, text);
    };
    let screen = Screen::new(
        run_folder,
        FrameRoute::of(machine),
        in_memory,
        overlay,
        machine.direct_frames(),
        || {},
        log,
    );
    // The desktop's own shortcuts, Alt+Tab among them, stay the desktop's: this is one program's
    // window among the others, not a whole desktop to hand the keyboard to.
    screen.ask_shortcuts_on_focus(|| false);
    screen.set_crop(Some(request.crop));
    screen.set_title_drag(request.drag);
    // GTK's window and not libadwaita's, which keeps a minimum size and rounded corners of its own.
    // Not resizable: the window is the rectangle's size, and follows the rectangle.
    let window = gtk::ApplicationWindow::builder()
        .application(application)
        .title(&request.title)
        .decorated(false)
        .resizable(false)
        .child(screen.widget())
        .build();
    let shown = Rc::new(Cell::new(Shown::Up));
    let host_file = request.follow.as_deref().map(host_file);
    follow_window_state(&screen, &window, shown.clone(), host_file, context.clone());
    end_on_signals(&window, shown.clone());
    window.present();
    // The own surface waits for the page it's on to settle. This window has no page to slide in.
    screen.page_shown();
    if let Some(file) = &request.follow {
        follow_file(&screen, &window, file.clone(), shown, context.clone());
    }
    let _ = write_log(
        context,
        &format!("lab crop window: {} at {:?}, titled {}", machine.name, request.crop, request.title),
    );
    window
}

fn host_file(follow: &Path) -> PathBuf {
    let mut name = follow.as_os_str().to_owned();
    name.push(format!(".{HOST_FILE_SUFFIX}"));
    PathBuf::from(name)
}

/// Writes the word for the caller, by a rename, so it never reads half of it.
fn tell_caller(host_file: &Path, word: &str) {
    let mut writing = host_file.as_os_str().to_owned();
    writing.push(format!(".{WRITING_SUFFIX}"));
    let _ = fs::write(&writing, format!("{word}\n")).and_then(|()| fs::rename(&writing, host_file));
}

/// Lets go of everything held in the guest whenever the window loses the focus or goes out of
/// sight, keeps the own surface off while it's out of sight, and tells the caller when the user
/// brings the window back after the guest's was minimized, or closes it.
fn follow_window_state(
    screen: &Screen,
    window: &gtk::ApplicationWindow,
    shown: Rc<Cell<Shown>>,
    host_file: Option<PathBuf>,
    context: Arc<Context>,
) {
    let looked = screen.clone();
    let seen = shown.clone();
    let told = host_file.clone();
    let follow = move |window: &gtk::ApplicationWindow| {
        let active = window.is_active();
        // Each change in the log, as what KWin does around a minimize is only seen on a desktop.
        let state = format!("active {active}, out of sight {}, {:?}", window.is_suspended(), seen.get());
        let _ = write_log(&context, &format!("lab crop window: {state}"));
        if !active || window.is_suspended() {
            looked.let_go();
        }
        // A minimized window's subsurface still had frames committed to it, and on KWin the window
        // didn't come back. Without the own surface GTK holds the last frame, and draws nothing new
        // until the window is seen again.
        if window.is_suspended() {
            looked.page_leaving();
        } else {
            looked.page_shown();
        }
        match seen.get() {
            Shown::Minimizing if !active => seen.set(Shown::Minimized),
            Shown::Minimized if active => {
                seen.set(Shown::Up);
                if let Some(file) = &told {
                    tell_caller(file, RESTORED);
                }
            }
            _ => {}
        }
    };
    let on_active = follow.clone();
    window.connect_is_active_notify(move |window| on_active(window));
    window.connect_suspended_notify(move |window| follow(window));
    let closing = screen.clone();
    window.connect_close_request(move |_| {
        closing.let_go();
        if shown.get() != Shown::Ending
            && let Some(file) = &host_file
        {
            tell_caller(file, CLOSED);
        }
        glib::Propagation::Proceed
    });
}

/// Closes the window on a signal to end, so what the guest has held down is let go first. Killed
/// with a button down, the guest's button stayed down and Windows took no clicks after it.
fn end_on_signals(window: &gtk::ApplicationWindow, shown: Rc<Cell<Shown>>) {
    for signal in SIGNALS {
        let window = window.downgrade();
        let shown = shown.clone();
        glib_unix::unix_signal_add_local_once(signal, move || {
            if let Some(window) = window.upgrade() {
                shown.set(Shown::Ending);
                window.close();
            }
        });
    }
}

/// Follows the guest's window as `file` says, each time it changes, for as long as the window is
/// open. A file that isn't there or says nothing it knows leaves things as they are.
fn follow_file(
    screen: &Screen,
    window: &gtk::ApplicationWindow,
    file: PathBuf,
    shown: Rc<Cell<Shown>>,
    context: Arc<Context>,
) {
    let screen = screen.clone();
    let window = window.downgrade();
    let mut seen: Option<SystemTime> = None;
    glib::timeout_add_local(FOLLOW_EVERY, move || {
        let Some(window) = window.upgrade() else { return glib::ControlFlow::Break };
        let changed = fs::metadata(&file).and_then(|found| found.modified()).ok();
        if changed.is_none() || changed == seen {
            return glib::ControlFlow::Continue;
        }
        seen = changed;
        let said = fs::read_to_string(&file).ok().as_deref().and_then(read_guest_window);
        let _ = write_log(&context, &format!("lab crop window: the guest's window is {said:?}"));
        match said {
            Some(GuestWindow::At(crop)) => {
                screen.set_crop(Some(crop));
                // The guest's window came back without this one, so this one asks to come back too.
                // A compositor may not bring back a window that asks for itself.
                if shown.replace(Shown::Up) != Shown::Up {
                    window.present();
                }
            }
            Some(GuestWindow::Minimized) => {
                if shown.get() == Shown::Up {
                    shown.set(Shown::Minimizing);
                    screen.let_go();
                    window.minimize();
                }
            }
            Some(GuestWindow::Closed) => {
                shown.set(Shown::Ending);
                window.close();
                return glib::ControlFlow::Break;
            }
            None => {}
        }
        glib::ControlFlow::Continue
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(text: &str) -> Vec<String> {
        text.split(' ').map(str::to_owned).collect()
    }

    #[test]
    fn only_the_lab_argument_asks_for_the_window() {
        assert_eq!(request(&[]), None);
        assert_eq!(request(&arguments("--at-login")), None);
        assert_eq!(request(&arguments("Win11 10 20 300 400 Calculator")), None);
    }

    #[test]
    fn the_window_takes_a_machine_a_rectangle_and_a_title() {
        let mut given = arguments("--lab-crop-window Win11 10 20 300 400");
        given.push("Calculator in Windows".to_owned());
        let crop = Crop { x: 10, y: 20, width: 300, height: 400 };
        let title = "Calculator in Windows".to_owned();
        let wanted = Request { machine: "Win11".to_owned(), crop, title, follow: None, drag: None };
        assert_eq!(request(&given), Some(Ok(wanted)));
    }

    #[test]
    fn a_file_and_a_drag_strip_come_after_the_title_in_either_order() {
        let given = arguments("--lab-crop-window Win11 10 20 300 400 Calc --drag 32 138 --follow /run/cell");
        let found = request(&given).and_then(Result::ok).unwrap();
        assert_eq!(found.follow, Some(PathBuf::from("/run/cell")));
        assert_eq!(found.drag, Some(DragStrip { height: 32, right: 138 }));
        let given = arguments("--lab-crop-window Win11 10 20 300 400 Calc --follow /run/cell");
        assert_eq!(request(&given).and_then(Result::ok).unwrap().drag, None);
    }

    #[test]
    fn a_request_that_is_wrong_says_so() {
        for wrong in [
            "--lab-crop-window Win11 10 20 300",
            "--lab-crop-window Win11 10 20 0 400 Calc",
            "--lab-crop-window Win11 a 20 300 400 Calc",
            "--lab-crop-window Win11 10 20 300 400 Calc /run/cell",
            "--lab-crop-window Win11 10 20 300 400 Calc --follow",
            "--lab-crop-window Win11 10 20 300 400 Calc --drag 32",
            "--lab-crop-window Win11 10 20 300 400 Calc --drag 0 138",
            "--lab-crop-window Win11 10 20 300 400 Calc --follow a --follow b",
        ] {
            assert!(matches!(request(&arguments(wrong)), Some(Err(_))), "{wrong}");
        }
    }

    #[test]
    fn the_guest_windows_file_says_where_it_is_or_what_became_of_it() {
        let crop = Crop { x: 10, y: 20, width: 300, height: 400 };
        assert_eq!(read_guest_window("10 20 300 400\n"), Some(GuestWindow::At(crop)));
        assert_eq!(read_guest_window("minimized\n"), Some(GuestWindow::Minimized));
        assert_eq!(read_guest_window(" closed"), Some(GuestWindow::Closed));
        assert_eq!(read_guest_window("maximized"), None);
        assert_eq!(read_guest_window(""), None);
    }

    #[test]
    fn the_callers_file_is_beside_the_one_it_writes() {
        assert_eq!(host_file(Path::new("/run/cell")), PathBuf::from("/run/cell.host"));
    }
}
