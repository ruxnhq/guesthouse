//! Lab only: a window that shows one rectangle of a running machine's screen, and nothing else.
//!
//! `guesthouse --lab-crop-window NAME X Y W H TITLE [FILE]` opens it, for the machine of that name or
//! id. A program's window can move in the guest, so a FILE given is read again whenever it changes,
//! and the "X Y W H" in it moves the rectangle, and resizes the window with it.
//! It's the first piece of showing one of the guest's programs as a window of its own: the window is
//! the rectangle at its own size, the pointer lands where it points in the guest, and keys go as they
//! do in the viewer. Nothing else of the viewer comes along. There is no pill, no capture, no shared
//! clipboard or file drops, and the guest's screen keeps its size, as no `WindowSize` is ever made
//! here: a window the size of one program must not become the size of the guest's desktop.
//!
//! It's a process of its own, with an application id of its own, so the app's window can be open
//! beside it. The daemon serves one viewer and QEMU one keyboard and mouse, though, so the machine's
//! own screen in the app has to be closed first.

use super::crop::Crop;
use super::frames::FrameRoute;
use super::overlay::OverlayChoice;
use super::screen::Screen;
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
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// The first argument that asks for this window. Nothing a user or the desktop passes looks like it.
pub const ARGUMENT: &str = "--lab-crop-window";
const USAGE: &str = "guesthouse --lab-crop-window NAME X Y W H TITLE [FILE]";
/// How often the rectangle's file is looked at. A program moved in the guest can wait that long.
const FOLLOW_EVERY: Duration = Duration::from_millis(500);
/// Another id than the app's, or a running app would be asked to activate and this process would end.
const CROP_APP_ID_SUFFIX: &str = ".LabCrop";

/// What was asked for on the command line.
#[derive(Debug, PartialEq, Eq)]
pub struct Request {
    pub machine: String,
    pub crop: Crop,
    pub title: String,
    /// The file to take the rectangle from whenever it changes.
    pub follow: Option<PathBuf>,
}

/// The request in these arguments, the program's name left out. `None` if they don't ask for this
/// window at all, and the reason if they ask for it wrongly.
pub fn request(arguments: &[String]) -> Option<Result<Request, String>> {
    let (first, rest) = arguments.split_first()?;
    if first != ARGUMENT {
        return None;
    }
    let (machine, x, y, width, height, title, follow) = match rest {
        [machine, x, y, width, height, title] => (machine, x, y, width, height, title, None),
        [machine, x, y, width, height, title, file] => {
            (machine, x, y, width, height, title, Some(PathBuf::from(file)))
        }
        _ => return Some(Err(format!("Usage: {USAGE}"))),
    };
    let Some(crop) = Crop::parse(&[x, y, width, height].map(String::as_str).join(" ")) else {
        return Some(Err(format!("X Y W H are four whole numbers, W and H above 0. Usage: {USAGE}")));
    };
    Some(Ok(Request { machine: machine.clone(), crop, title: title.clone(), follow }))
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

fn open_window(
    application: &adw::Application,
    context: &Arc<Context>,
    machine: &Machine,
    run_folder: PathBuf,
    request: &Request,
) {
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
    // The desktop's usual title bar and nothing more. GTK draws its own, so without a header there
    // would be no title and no way to move or close the window.
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(screen.widget()));
    // Not resizable: the window is the rectangle's size, and follows the rectangle.
    let window = adw::ApplicationWindow::builder()
        .application(application)
        .title(&request.title)
        .resizable(false)
        .content(&toolbar)
        .build();
    window.present();
    // The own surface waits for the page it's on to settle. This window has no page to slide in.
    screen.page_shown();
    if let Some(file) = &request.follow {
        follow_file(&screen, &window, file.clone(), context.clone());
    }
    let _ = write_log(
        context,
        &format!("lab crop window: {} at {:?}, titled {}", machine.name, request.crop, request.title),
    );
}

/// Takes the rectangle from `file` each time it changes, for as long as the window is open. A file
/// that isn't there or doesn't hold a rectangle leaves the one shown as it is.
fn follow_file(screen: &Screen, window: &adw::ApplicationWindow, file: PathBuf, context: Arc<Context>) {
    let screen = screen.clone();
    let window = window.downgrade();
    let mut seen: Option<SystemTime> = None;
    glib::timeout_add_local(FOLLOW_EVERY, move || {
        if window.upgrade().is_none() {
            return glib::ControlFlow::Break;
        }
        let changed = fs::metadata(&file).and_then(|found| found.modified()).ok();
        if changed.is_some() && changed != seen {
            seen = changed;
            match fs::read_to_string(&file).ok().as_deref().and_then(Crop::parse) {
                Some(crop) => {
                    screen.set_crop(Some(crop));
                    let _ = write_log(&context, &format!("lab crop window: now at {crop:?}"));
                }
                None => {
                    let _ =
                        write_log(&context, &format!("lab crop window: no X Y W H in {}", file.display()));
                }
            }
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
        let wanted = Request { machine: "Win11".to_owned(), crop, title, follow: None };
        assert_eq!(request(&given), Some(Ok(wanted)));
    }

    #[test]
    fn a_file_after_the_title_is_followed() {
        let given = arguments("--lab-crop-window Win11 10 20 300 400 Calc /run/cell");
        let followed = request(&given).and_then(Result::ok).and_then(|request| request.follow);
        assert_eq!(followed, Some(PathBuf::from("/run/cell")));
    }

    #[test]
    fn a_request_that_is_wrong_says_so() {
        assert!(matches!(request(&arguments("--lab-crop-window Win11 10 20 300")), Some(Err(_))));
        let too_many = arguments("--lab-crop-window Win11 10 20 300 400 Calc /run/cell more");
        assert!(matches!(request(&too_many), Some(Err(_))));
        assert!(matches!(request(&arguments("--lab-crop-window Win11 10 20 0 400 Calc")), Some(Err(_))));
        assert!(matches!(request(&arguments("--lab-crop-window Win11 a 20 300 400 Calc")), Some(Err(_))));
    }
}
