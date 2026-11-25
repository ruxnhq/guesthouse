//! App windows, the viewer's half: one process for a machine that shows each program window of its
//! Windows guest as a window of its own on this desktop. See `guesthouse_app_core::app_windows` for the
//! link to the guest's helper and the cells it reports.
//!
//! The process owns the machine's frames and input once, in a screen that is in no window (the hub),
//! and gives every frame to every view. A view is a toplevel for a window cell, or a popup surface
//! against its parent's for a popup cell, and shows its cell's part of the frame with no copy. A new
//! view has the newest frame at once, so a window opened on a still screen is not black.
//!
//! A host window takes the size the compositor gives it (a tiling layout, a maximize, the user's drag)
//! and the guest's window is asked for that size; the picture is cut or in the middle, never
//! stretched, until the guest's cell has it. A cell that changes size makes a window that is its own
//! to size that size. A guest desktop that is not the normal one keeps the windows as they are and takes no input for them.
//! A link that goes down leaves the windows with their last picture, and the helper's next layout
//! after it comes back says what became of them.

mod capabilities;
mod cell_picture;
mod feed;
mod framed;
mod hub;
pub mod identity;
pub mod launch;
pub mod plan;
mod published;
mod secure;
mod view;

use super::frames::FrameRoute;
use super::lab_crop::find_machine;
use super::overlay::OverlayChoice;
use super::screen::Screen;
use super::window_size::GuestSizer;
use crate::application::{APP_ID, use_built_in_icons};
use adw::prelude::*;
use gtk::{gdk, gio, glib};
use guesthouse_app_core::app_windows::{connect, viewer_lock};
use guesthouse_app_core::context::Context;
use guesthouse_app_core::host_sensors::sensors_file;
use guesthouse_app_core::log::{log_panics, write_log};
use guesthouse_app_core::machine::Machine;
use guesthouse_app_core::screen_gpu::memory_frames;
use guesthouse_app_core::tasks::slot_of;
use guesthouse_daemon::slot::Slot;
use hub::{Hub, Sizer};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

/// The argument that asks for this mode. Nothing a user or the desktop passes looks like it.
pub const ARGUMENT: &str = "--app-windows";
const USAGE: &str = "Usage: guesthouse --app-windows NAME

NAME is the machine's name or id. Its Windows programs each get a window of their own, for as long
as the machine runs. App windows has to be on for the machine (guesthouse-vm set NAME app-windows on),
and the machine's own screen in the app has to be closed, as the machine serves one viewer.";
const NOT_WAYLAND: &str = "App windows need a Wayland desktop session, and this one is not (an X11 \
session, or GDK_BACKEND set to x11). Log in with a Wayland session to use them; the machine's own screen \
works in either.";
/// Another id than the app's, or a running app would be asked to activate and this process would end.
const APP_ID_SUFFIX: &str = ".AppWindows";
/// Linux's numbers for the signals that end the process: a terminal's Ctrl+C, `kill`, and a terminal
/// closing.
const SIGNALS: [i32; 3] = [2, 15, 1];

/// What was asked for on the command line.
#[derive(Debug, PartialEq, Eq)]
pub struct Request {
    pub machine: String,
}

/// The request in these arguments, the program's name left out. `None` if they don't ask for this at
/// all, and the usage if they ask for it wrongly.
pub fn request(arguments: &[String]) -> Option<Result<Request, String>> {
    let (first, rest) = arguments.split_first()?;
    if first != ARGUMENT {
        return None;
    }
    Some(match rest {
        [machine] => Ok(Request { machine: machine.clone() }),
        _ => Err(USAGE.to_owned()),
    })
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
    // One viewer per machine: a second, however it came to be started, ends here.
    let _lock = match viewer_lock::take(&context, slot) {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            println!("The app windows of {} are already shown by another viewer.", machine.name);
            return glib::ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!("The app windows viewer could not take its lock: {error}");
            return glib::ExitCode::FAILURE;
        }
    };
    let run_folder = context.run_folder(Some(slot));
    let running = {
        let (context, machine) = (context.clone(), machine.clone());
        move || slot_of(&context, &machine).is_some()
    };
    published::start(run_folder.clone());
    let code = run_with(context, machine, Some(slot), run_folder.clone(), running);
    // While the lock is still held, so that the Library never sees a file of a viewer that is gone.
    let _ = guesthouse_app_core::app_windows::open_windows::remove_from(&run_folder);
    code
}

/// The process, for a machine that is running and a folder its sockets are in. `running` is asked now
/// and then, and ends the process when it says no. Public for a lab that stands in for the machine.
pub fn run_with(
    context: Arc<Context>,
    machine: Machine,
    slot: Option<Slot>,
    run_folder: PathBuf,
    running: impl Fn() -> bool + 'static,
) -> glib::ExitCode {
    let link = match connect(&context, &machine, slot) {
        Ok(link) => link,
        Err(failure) => {
            eprintln!("{failure}");
            return glib::ExitCode::FAILURE;
        }
    };
    identity::init(context.clone(), machine.clone(), slot);
    let application = adw::Application::builder()
        .application_id(format!("{APP_ID}{APP_ID_SUFFIX}"))
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let icons = context.paths.runtime.join("icons");
    application.connect_startup(move |_| use_built_in_icons(&icons));
    let pieces = std::cell::RefCell::new(Some((context, machine, run_folder, slot, link, running)));
    // Kept here so the signals and the hub live as long as the application runs.
    let hub: Rc<std::cell::RefCell<Option<Rc<Hub>>>> = Rc::default();
    application.connect_activate(move |application| {
        let Some((context, machine, run_folder, slot, link, running)) = pieces.borrow_mut().take() else {
            return;
        };
        // Each window's picture is a surface of the view's own under the compositor's control, which
        // only a Wayland session has.
        if gdk::Display::default().and_downcast::<gdk_wayland::WaylandDisplay>().is_none() {
            eprintln!("{NOT_WAYLAND}");
            let _ = write_log(&context, NOT_WAYLAND);
            application.quit();
            return;
        }
        let started = open(application, &context, &machine, run_folder, slot, link, running);
        for signal in SIGNALS {
            let hub = Rc::downgrade(&started);
            glib_unix::unix_signal_add_local_once(signal, move || {
                if let Some(hub) = hub.upgrade() {
                    hub.note(&format!("signal {signal}: the app windows end"));
                    hub.end();
                }
            });
        }
        *hub.borrow_mut() = Some(started);
    });
    application.run_with_args::<&str>(&[])
}

fn open(
    application: &adw::Application,
    context: &Arc<Context>,
    machine: &Machine,
    run_folder: PathBuf,
    slot: Option<Slot>,
    link: guesthouse_app_core::app_windows::MachineLink,
    running: impl Fn() -> bool + 'static,
) -> Rc<Hub> {
    let in_memory = machine.has_gpu() && memory_frames(context, machine);
    let logging = context.clone();
    let log = move |text: &str| {
        let _ = write_log(&logging, text);
    };
    let overlay = OverlayChoice { shown: false, sensors: sensors_file(context, machine) };
    let screen = Screen::new(run_folder, FrameRoute::of(machine), in_memory, overlay, false, || {}, log);
    let sizer = match std::env::var_os("GUESTHOUSE_LAB_SIZE_FILE") {
        Some(path) => Sizer::File(path.into()),
        None => GuestSizer::start(context.clone(), machine.clone(), slot).map_or(Sizer::Not, Sizer::Guest),
    };
    Hub::start(application, context.clone(), machine.name.clone(), screen, link, sizer, running)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(text: &str) -> Vec<String> {
        text.split(' ').map(str::to_owned).collect()
    }

    #[test]
    fn only_its_own_argument_asks_for_app_windows() {
        assert_eq!(request(&[]), None);
        assert_eq!(request(&arguments("--at-login")), None);
        assert_eq!(request(&arguments("--lab-crop-window Win11 1 2 3 4 T")), None);
        assert_eq!(
            request(&arguments("--app-windows Win11")),
            Some(Ok(Request { machine: "Win11".to_owned() }))
        );
    }

    #[test]
    fn a_request_that_is_wrong_says_so() {
        assert!(matches!(request(&arguments("--app-windows")), Some(Err(_))));
        assert!(matches!(request(&arguments("--app-windows a b")), Some(Err(_))));
    }
}
