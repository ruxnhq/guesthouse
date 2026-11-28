//! The window's app id while it shows a machine's screen, so the compositor can tell the screen from
//! the library: a rule can keep glass and effects off a guest's screen, and give it a border of its
//! own, by this id. The library and the screen are pages of one window, so the window changes its
//! id as the screen comes and goes.
//!
//! A task bar finds a window's name and icon by the launcher entry of its app id. So the screen's
//! has one too, hidden from menus, written next to the app's other entries the first time it's
//! needed, as the app windows viewer writes its programs'.
//!
//! GDK sets the app's own id each time the window is mapped, so the wanted one goes on after that.
//! Only Wayland lets an id change on a window that's up; on X11 the window keeps the app's.

use crate::application::APP_ID;
use gdk_wayland::WaylandToplevel;
use gtk::prelude::*;
use guesthouse_app_core::context::Context;
use std::cell::RefCell;
use std::fs;

/// The screen's app id: the app's, with a part of its own.
pub const SCREEN_APP_ID: &str = "io.github.xnixy.Guesthouse.Screen";
const ENTRY_EXTENSION: &str = ".desktop";

thread_local! {
    /// The id the main window should have, when it isn't the app's own.
    static WANTED: RefCell<Option<&'static str>> = const { RefCell::new(None) };
}

fn entry_text() -> String {
    let program =
        std::env::current_exe().map(|path| path.display().to_string()).unwrap_or("guesthouse".into());
    format!(
        "[Desktop Entry]\nType=Application\nName=Guesthouse\nComment=A machine's screen\nExec={program}\n\
         Icon={APP_ID}\nTerminal=false\nNoDisplay=true\n"
    )
}

/// Writes the screen's launcher entry, unless it's there as it should be.
fn write_entry(context: &Context) {
    let folder = context.paths.applications();
    let path = folder.join(format!("{SCREEN_APP_ID}{ENTRY_EXTENSION}"));
    let text = entry_text();
    if fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
        // Without it the task bar shows no icon for the screen, which is all that's lost.
        let _ = fs::create_dir_all(&folder).and_then(|()| fs::write(&path, text));
    }
}

fn apply(surface: &gtk::gdk::Surface) {
    let id = WANTED.with_borrow(|wanted| wanted.unwrap_or(APP_ID));
    if surface.is_mapped()
        && let Some(top) = surface.downcast_ref::<WaylandToplevel>()
    {
        top.set_application_id(id);
    }
}

/// Keeps the wanted id on the window each time it's mapped. Called once, for the main window.
pub fn follow(window: &gtk::Window) {
    let hook = |window: &gtk::Window| {
        if let Some(surface) = window.surface() {
            surface.connect_mapped_notify(apply);
        }
    };
    if window.is_realized() {
        hook(window);
    } else {
        window.connect_realize(hook);
    }
}

/// Gives the window the screen's id while `showing`, and the app's own after.
pub fn set_showing_screen(context: &Context, window: &gtk::Window, showing: bool) {
    if showing {
        write_entry(context);
    }
    WANTED.set(showing.then_some(SCREEN_APP_ID));
    if let Some(surface) = window.surface() {
        apply(&surface);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_screens_id_is_the_apps_and_its_entry_is_hidden() {
        assert!(SCREEN_APP_ID.starts_with(APP_ID));
        let text = entry_text();
        assert!(text.contains(&format!("Icon={APP_ID}\n")) && text.contains("NoDisplay=true\n"));
    }
}
