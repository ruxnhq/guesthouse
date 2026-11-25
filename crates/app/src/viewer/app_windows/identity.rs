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
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, OnceLock};

/// The viewer's one identity of programs, set at its start.
static IDENTITY: OnceLock<Arc<Identity>> = OnceLock::new();

pub struct Identity {
    context: Arc<Context>,
    machine: Machine,
    slot: Option<Slot>,
    apps: Mutex<Vec<App>>,
    /// The programs seen so far, by app id, so that an icon or a Start menu list that comes after a
    /// window opened is written for it too.
    seen: Mutex<BTreeMap<String, Seen>>,
    /// The app ids whose icon has been asked of the helper, so each is asked once while it is in flight
    /// and not again once it is on disk.
    asked: Mutex<BTreeSet<String>>,
}

/// What a window said of its program, and the cell to ask the icon of.
#[derive(Clone)]
struct Seen {
    key: String,
    name: String,
    cell: String,
}

/// Starts the identity of this viewer's machine. The Start menu's programs, with their icons, are asked
/// of the helper in a thread of their own, as it takes a moment and no window waits for it.
pub fn init(context: Arc<Context>, machine: Machine, slot: Option<Slot>) {
    let identity = Arc::new(Identity {
        context,
        machine,
        slot,
        apps: Mutex::default(),
        seen: Mutex::default(),
        asked: Mutex::default(),
    });
    if IDENTITY.set(identity.clone()).is_ok() {
        std::thread::spawn(move || identity.fetch_apps());
    }
}

/// What is known of the program with this app id, for what the app's Library is told of the open
/// windows: its key, its name and where its icon is saved.
pub fn describe(id: &str) -> Option<guesthouse_app_core::app_windows::open_windows::OpenProgram> {
    IDENTITY.get()?.describe(id)
}

/// The app id for a window of this cell's program. `None` before `init`, or when no entry could be
/// written, which leaves the window with the viewer's own id as it always had.
pub fn app_id_for(cell: &Cell) -> Option<String> {
    IDENTITY.get()?.app_id_for(cell)
}

thread_local! {
    /// The app id each toplevel has now, by its address: the one it was opened with, or the one
    /// `set_now` gave it since, which a later map gives it again.
    static CURRENT: RefCell<HashMap<usize, String>> = RefCell::default();
}

fn window_key(window: &gtk::Window) -> usize {
    window.as_ptr() as usize
}

/// Gives the window the app id once its surface is mapped, and the one it has then each time it is
/// mapped again.
pub fn set_when_mapped(window: &gtk::Window, id: &str) {
    super::published::add(window, id);
    let address = window_key(window);
    CURRENT.with(|current| current.borrow_mut().insert(address, id.to_owned()));
    window.connect_destroy(move |_| {
        CURRENT.with(|current| current.borrow_mut().remove(&address));
    });
    let hook = move |window: &gtk::Window| {
        let Some(surface) = window.surface() else { return };
        let apply = {
            move |surface: &gtk::gdk::Surface| {
                let id = CURRENT.with(|current| current.borrow().get(&address).cloned());
                if surface.is_mapped()
                    && let Some(id) = id
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

/// Gives an already mapped window another app id, as its program turned out to be another.
pub fn set_now(window: &gtk::Window, id: &str) {
    super::published::change(window, id);
    CURRENT.with(|current| {
        if let Some(now) = current.borrow_mut().get_mut(&window_key(window)) {
            id.clone_into(now);
        }
    });
    if let Some(surface) = window.surface()
        && surface.is_mapped()
        && let Some(top) = surface.downcast_ref::<WaylandToplevel>()
    {
        top.set_application_id(id);
    }
}

impl Identity {
    fn log(&self, text: &str) {
        let _ = write_log(&self.context, &format!("app windows identity: {text}"));
    }

    fn describe(&self, id: &str) -> Option<guesthouse_app_core::app_windows::open_windows::OpenProgram> {
        let seen = self.seen.lock().ok()?.get(id).cloned()?;
        let program = program_of(&seen.key, &seen.name, &self.apps.lock().ok()?);
        Some(guesthouse_app_core::app_windows::open_windows::OpenProgram {
            key: seen.key,
            name: program.name,
            windows: 0,
            icon: launcher::icon_path(&self.context, id),
        })
    }

    fn app_id_for(self: &Arc<Self>, cell: &Cell) -> Option<String> {
        let key = program_key(cell);
        let seen = Seen { key, name: cell.name.clone(), cell: cell.id.clone() };
        self.write_for(&seen)
    }

    /// Writes the entry for what a window said, and asks for the icon if it has none yet.
    fn write_for(self: &Arc<Self>, seen: &Seen) -> Option<String> {
        let program = program_of(&seen.key, &seen.name, &self.apps.lock().ok()?);
        let id = launcher::app_id(&self.machine, &program.exe);
        match launcher::ensure_for_window(&self.context, &self.machine, &program) {
            Ok(id) => {
                self.seen.lock().ok()?.insert(id.clone(), seen.clone());
                let has_icon = program.icon.is_some() || launcher::icon_of(&self.context, &id).is_some();
                if !has_icon {
                    self.ask_icon(&id, &seen.cell);
                }
                Some(id)
            }
            Err(failure) => {
                self.log(&format!("no launcher entry for {id}: {failure}"));
                None
            }
        }
    }

    /// Asks the helper for the icon of the program of this cell, in a thread, and writes the entry
    /// again when it comes.
    fn ask_icon(self: &Arc<Self>, id: &str, cell: &str) {
        if !self.asked.lock().is_ok_and(|mut asked| asked.insert(id.to_owned())) {
            return;
        }
        let (me, id, cell) = (self.clone(), id.to_owned(), cell.to_owned());
        std::thread::spawn(move || {
            let png = me.fetch_icon(&cell);
            match png {
                Ok(png) => {
                    if let Some(seen) = me.seen.lock().ok().and_then(|seen| seen.get(&id).cloned()) {
                        me.write_with_icon(&seen, png);
                    }
                }
                Err(why) => {
                    me.log(&format!("no icon for {id}: {why}"));
                    // A later window of the program asks again.
                    if let Ok(mut asked) = me.asked.lock() {
                        asked.remove(&id);
                    }
                }
            }
        });
    }

    fn fetch_icon(&self, cell: &str) -> std::result::Result<Vec<u8>, String> {
        let mut link = connect_side(&self.context, &self.machine, self.slot).map_err(|f| f.to_string())?;
        link.wait_ready().map_err(|f| f.to_string())?;
        let patience = self.context.app_windows.timing.apps_request;
        let answer = link
            .client
            .ask(Request::Icon { id: cell.to_owned(), size: ICON_SIDE }, patience)
            .map_err(|f| f.to_string())?;
        if !answer.ok {
            return Err(answer.error);
        }
        answer
            .icon
            .as_deref()
            .and_then(base64_decode)
            .ok_or_else(|| "the helper sent no picture".to_owned())
            .and_then(|png| {
                if launcher::valid_icon(&png) {
                    Ok(png)
                } else {
                    Err("the helper's picture is not a small PNG".to_owned())
                }
            })
    }

    fn write_with_icon(&self, seen: &Seen, png: Vec<u8>) {
        let Ok(apps) = self.apps.lock() else { return };
        let mut program = program_of(&seen.key, &seen.name, &apps);
        program.icon = Some(png);
        if let Err(failure) = launcher::ensure_for_window(&self.context, &self.machine, &program) {
            self.log(&format!("the icon could not be written: {failure}"));
        }
    }

    fn fetch_apps(self: &Arc<Self>) {
        let Ok(mut link) = connect_side(&self.context, &self.machine, self.slot) else { return };
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
        // Windows that opened before the list came get its names and icons now.
        let seen: Vec<Seen> =
            self.seen.lock().map(|seen| seen.values().cloned().collect()).unwrap_or_default();
        for seen in seen {
            self.write_for(&seen);
        }
    }
}

/// The side of the icon asked for: sharp on a scaled screen, and scaled down by the desktop elsewhere.
const ICON_SIDE: u32 = 96;

/// What the window's program is known by: what its helper says, else the program's file name, which
/// is all an older helper tells.
pub fn program_key(cell: &Cell) -> String {
    if cell.app.is_empty() { cell.exe.clone() } else { cell.app.clone() }
}

/// The file name of a path of the guest, in lower case.
fn file_name(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_lowercase()
}

/// What is known of the program whose window has this key (a path, an `aumid:`, or a bare file name)
/// and says it is called `name`: the Start menu program that starts it, if the list has one, with its
/// name and icon, else the window's own word for it, else a name made from the file's.
pub fn program_of(key: &str, name: &str, apps: &[App]) -> Program {
    let found = apps
        .iter()
        .filter(|app| {
            !app.target.is_empty()
                && if key.contains(['\\', '/']) || launcher::is_aumid(key) {
                    launcher::normal_key(&app.target) == launcher::normal_key(key)
                } else {
                    file_name(&app.target) == file_name(key)
                }
        })
        .min_by_key(|app| app.minor);
    match found {
        Some(app) => Program {
            name: app.name.clone(),
            exe: key.to_owned(),
            launch: Launch::App(app.id.clone()),
            icon: base64_decode(&app.icon).filter(|png| !png.is_empty()),
        },
        None => {
            let named = name.trim();
            let name = if named.is_empty() {
                let wanted = file_name(key);
                let stem = wanted.strip_suffix(".exe").unwrap_or(&wanted);
                let mut made = stem.to_owned();
                if let Some(first) = made.get(..1) {
                    made = first.to_uppercase() + &made[1..];
                }
                made
            } else {
                named.to_owned()
            };
            Program {
                name,
                exe: key.to_owned(),
                launch: Launch::Path { path: launcher::launch_path(key), args: String::new() },
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
        let program = program_of("MSPaint.exe", "", &apps);
        assert_eq!(program.name, "Paint");
        assert_eq!(program.launch, Launch::App("Accessories/Paint".into()));
        assert_eq!(program.icon.as_deref(), Some(&b"\x89PNG\r\n\x1a\n"[..]));
    }

    #[test]
    fn a_program_with_no_shortcut_is_named_from_its_file_and_has_no_icon() {
        let program = program_of("calculator.exe", "", &[app("A/B", "B", r"C:\b.exe", false)]);
        assert_eq!((program.name.as_str(), program.icon), ("Calculator", None));
        assert_eq!(program.launch, Launch::Path { path: "calculator.exe".into(), args: String::new() });
    }

    #[test]
    fn a_window_that_says_what_it_is_gets_its_own_name_and_a_store_app_its_own_id() {
        let machine = Machine::new("work", "Office PC", "/d", "/v");
        let calculator = program_of("aumid:Microsoft.WindowsCalculator_8wekyb3d8bbwe!App", "Calculator", &[]);
        let notepad = program_of("aumid:Microsoft.WindowsNotepad_8wekyb3d8bbwe!App", "Notepad", &[]);
        assert_eq!((calculator.name.as_str(), notepad.name.as_str()), ("Calculator", "Notepad"));
        let (a, b) = (launcher::app_id(&machine, &calculator.exe), launcher::app_id(&machine, &notepad.exe));
        assert_ne!(a, b, "two Store apps are two programs though one process hosts both");
        assert!(a.starts_with("guesthouse-work-microsoft-windowscalculator-"), "{a}");
    }

    #[test]
    fn a_copy_of_a_program_in_another_folder_is_another_program_and_the_same_path_one_program() {
        let machine = Machine::new("work", "Office PC", "/d", "/v");
        let id = |path: &str| launcher::app_id(&machine, &program_of(path, "Winver", &[]).exe);
        assert_ne!(id(r"C:\Windows\System32\winver.exe"), id(r"C:\Users\me\custom\winver.exe"));
        assert_eq!(id(r"C:\Windows\System32\winver.exe"), id(r"c:/windows/system32/WINVER.EXE"));
    }

    #[test]
    fn a_window_and_the_start_menu_entry_of_its_program_get_one_app_id() {
        let machine = Machine::new("work", "Office PC", "/d", "/v");
        let apps = [app("Accessories/Paint", "Paint", r"C:\Windows\System32\mspaint.exe", false)];
        let window = program_of(r"c:\windows\system32\mspaint.exe", "Paint (guest's words)", &apps);
        assert_eq!(window.name, "Paint", "the user's own entry names it");
        let page = guesthouse_app_core::app_windows::program_of(&apps[0], None);
        assert_eq!(launcher::app_id(&machine, &window.exe), launcher::app_id(&machine, &page.exe));
    }
}
