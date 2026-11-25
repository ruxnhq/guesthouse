//! What the desktop's task bar sees of each program's windows: an app id, and through the launcher
//! entry of that name, a title and an icon.
//!
//! One GTK process has one application id, which every toplevel it makes would carry, so a task bar
//! would show all of a machine's windows as one program with no icon. GDK can give a single toplevel
//! another `xdg_toplevel` app id (`gdk_wayland_toplevel_set_application_id`, read by KWin 6.8 as the
//! window's `desktopFileName` and `resourceClass`, which the task bar looks the entry up by), and
//! only after the surface is mapped, as GTK sets its own at the map. So each window gets the app id of
//! its program, `guesthouse-MACHINE-PROGRAM`, and a launcher entry of that name is made for it: the
//! user's own if they added one, else a hidden one (`NoDisplay=true`) that this writes, with the
//! program's name and, when the Start menu has a shortcut for the program, its icon.
//!
//! A program with no Start menu shortcut (a Store app) gets a name made from its file name and no icon:
//! the helper has no way yet to give the icon of a window's program.

use gdk_wayland::WaylandToplevel;
use gtk::prelude::*;
use guesthouse_app_core::app_windows::launcher::{self, Program};
use guesthouse_app_core::app_windows::{App, Cell, Launch, Request, connect_side};
use guesthouse_app_core::context::Context;
use guesthouse_app_core::log::write_log;
use guesthouse_app_core::machine::Machine;
use guesthouse_app_core::text::base64_decode;
use guesthouse_daemon::slot::Slot;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

/// The viewer's one identity of programs, set at its start.
static IDENTITY: OnceLock<Arc<Identity>> = OnceLock::new();

pub struct Identity {
    context: Arc<Context>,
    machine: Machine,
    apps: Mutex<Vec<App>>,
    /// The program files seen so far, by app id, so that an icon that comes after a window opened is
    /// written for it too.
    seen: Mutex<BTreeMap<String, String>>,
}

/// Starts the identity of this viewer's machine. The Start menu's programs, with their icons, are asked
/// of the helper in a thread of their own, as it takes a moment and no window waits for it.
pub fn init(context: Arc<Context>, machine: Machine, slot: Option<Slot>) {
    let identity = Arc::new(Identity { context, machine, apps: Mutex::default(), seen: Mutex::default() });
    if IDENTITY.set(identity.clone()).is_ok() {
        std::thread::spawn(move || identity.fetch_apps(slot));
    }
}

/// The app id for a window of this cell's program. `None` before `init`, or when no entry could be
/// written, which leaves the window with the viewer's own id as it always had.
pub fn app_id_for(cell: &Cell) -> Option<String> {
    IDENTITY.get()?.app_id_for(&cell.exe)
}

/// Gives the window the app id once its surface is mapped.
pub fn set_when_mapped(window: &gtk::Window, id: &str) {
    let id = id.to_owned();
    let hook = move |window: &gtk::Window| {
        let Some(surface) = window.surface() else { return };
        let apply = {
            let id = id.clone();
            move |surface: &gtk::gdk::Surface| {
                if surface.is_mapped()
                    && let Some(top) = surface.downcast_ref::<WaylandToplevel>()
                {
                    top.set_application_id(&id);
                }
            }
        };
        apply(&surface);
        surface.connect_mapped_notify(apply);
    };
    if window.is_realized() {
        hook(window);
    } else {
        window.connect_realize(hook);
    }
}

impl Identity {
    fn log(&self, text: &str) {
        let _ = write_log(&self.context, &format!("app windows identity: {text}"));
    }

    fn app_id_for(&self, exe: &str) -> Option<String> {
        let program = program_of(exe, &self.apps.lock().ok()?);
        match launcher::ensure_for_window(&self.context, &self.machine, &program) {
            Ok(id) => {
                self.seen.lock().ok()?.insert(id.clone(), exe.to_owned());
                Some(id)
            }
            Err(failure) => {
                self.log(&format!("no launcher entry for {exe}: {failure}"));
                None
            }
        }
    }

    fn fetch_apps(&self, slot: Option<Slot>) {
        let Ok(mut link) = connect_side(&self.context, &self.machine, slot) else { return };
        if let Err(failure) = link.wait_ready() {
            return self.log(&format!("the Start menu was not asked: {failure}"));
        }
        let patience = self.context.app_windows.timing.apps_request;
        match link.client.ask(Request::Apps { icons: true }, patience) {
            Ok(answer) if answer.ok => {
                if let Ok(mut apps) = self.apps.lock() {
                    *apps = answer.apps.unwrap_or_default();
                }
            }
            Ok(answer) => {
                return self.log(&format!("the helper would not list its programs: {}", answer.error));
            }
            Err(failure) => return self.log(&format!("the helper did not list its programs: {failure}")),
        }
        // Windows that opened before the icons came get them now.
        let seen: Vec<String> =
            self.seen.lock().map(|seen| seen.values().cloned().collect()).unwrap_or_default();
        for exe in seen {
            self.app_id_for(&exe);
        }
    }
}

/// The file name of a path of the guest, in lower case.
fn file_name(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_lowercase()
}

/// What is known of the program whose window has this `exe`: the Start menu program that starts it, if
/// the list has one, with its name and icon, else a name made from the file's.
pub fn program_of(exe: &str, apps: &[App]) -> Program {
    let wanted = file_name(exe);
    let found = apps
        .iter()
        .filter(|app| !app.target.is_empty() && file_name(&app.target) == wanted)
        .min_by_key(|app| app.minor);
    match found {
        Some(app) => Program {
            name: app.name.clone(),
            exe: app.target.clone(),
            launch: Launch::App(app.id.clone()),
            icon: base64_decode(&app.icon).filter(|png| !png.is_empty()),
        },
        None => {
            let stem = wanted.strip_suffix(".exe").unwrap_or(&wanted);
            let mut name = stem.to_owned();
            if let Some(first) = name.get(..1) {
                name = first.to_uppercase() + &name[1..];
            }
            Program {
                name,
                exe: exe.to_owned(),
                launch: Launch::Path { path: exe.to_owned(), args: String::new() },
                icon: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, name: &str, target: &str, minor: bool) -> App {
        App { id: id.into(), name: name.into(), target: target.into(), icon: "iVBORw0KGgo=".into(), minor }
    }

    #[test]
    fn a_window_takes_its_name_and_icon_from_the_start_menu_program_that_starts_it() {
        let apps = [
            app("Accessories/Paint Help", "Paint Help", r"C:\Windows\System32\mspaint.exe", true),
            app("Accessories/Paint", "Paint", r"C:\Windows\System32\mspaint.exe", false),
        ];
        let program = program_of("MSPaint.exe", &apps);
        assert_eq!(program.name, "Paint");
        assert_eq!(program.launch, Launch::App("Accessories/Paint".into()));
        assert_eq!(program.icon.as_deref(), Some(&b"\x89PNG\r\n\x1a\n"[..]));
    }

    #[test]
    fn a_program_with_no_shortcut_is_named_from_its_file_and_has_no_icon() {
        let program = program_of("calculator.exe", &[app("A/B", "B", r"C:\b.exe", false)]);
        assert_eq!((program.name.as_str(), program.icon), ("Calculator", None));
        assert_eq!(program.launch, Launch::Path { path: "calculator.exe".into(), args: String::new() });
    }
}
