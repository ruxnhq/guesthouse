//! What the viewer tells the app's Library of its open windows: which programs, and how many windows
//! each. The windows are the viewer's own toplevels (popups are not), kept here as they are given
//! their program's app id, and written to the machine's run folder as a small file whenever the set
//! changes. See `guesthouse_app_core::app_windows::open_windows` for the file.

use super::identity;
use gtk::glib;
use gtk::prelude::*;
use guesthouse_app_core::app_windows::open_windows::{OpenProgram, remove_from, write_to};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

/// How often what the identity learns later (a name, an icon) is looked for, with no window changing.
const TICK: Duration = Duration::from_secs(2);

#[derive(Default)]
struct Published {
    folder: PathBuf,
    /// Each open toplevel, by its address, with its program's app id.
    windows: BTreeMap<usize, (glib::WeakRef<gtk::Window>, String)>,
    written: Option<Vec<OpenProgram>>,
}

thread_local! {
    static PUBLISHED: RefCell<Option<Published>> = const { RefCell::new(None) };
}

/// Starts publishing into this run folder. Called once, on the main thread, before the loop runs.
pub fn start(folder: PathBuf) {
    PUBLISHED.with(|published| *published.borrow_mut() = Some(Published { folder, ..Published::default() }));
    glib::timeout_add_local(TICK, || {
        publish();
        glib::ControlFlow::Continue
    });
}

fn key(window: &gtk::Window) -> usize {
    window.as_ptr() as usize
}

/// A toplevel got its program's app id.
pub fn add(window: &gtk::Window, id: &str) {
    PUBLISHED.with(|published| {
        if let Some(published) = published.borrow_mut().as_mut() {
            published.windows.insert(key(window), (window.downgrade(), id.to_owned()));
        }
    });
    let address = key(window);
    window.connect_destroy(move |_| {
        PUBLISHED.with(|published| {
            if let Some(published) = published.borrow_mut().as_mut() {
                published.windows.remove(&address);
            }
        });
        publish();
    });
    publish();
}

/// A toplevel turned out to be of another program.
pub fn change(window: &gtk::Window, id: &str) {
    PUBLISHED.with(|published| {
        if let Some(published) = published.borrow_mut().as_mut()
            && let Some(found) = published.windows.get_mut(&key(window))
        {
            found.1 = id.to_owned();
        }
    });
    publish();
}

/// Writes the file if what is open is not what it says already.
fn publish() {
    PUBLISHED.with(|published| {
        let mut published = published.borrow_mut();
        let Some(published) = published.as_mut() else { return };
        published.windows.retain(|_, (window, _)| window.upgrade().is_some());
        let mut counts: BTreeMap<&str, u32> = BTreeMap::new();
        for (_, id) in published.windows.values() {
            *counts.entry(id).or_default() += 1;
        }
        let mut programs: Vec<OpenProgram> = counts
            .into_iter()
            .filter_map(|(id, windows)| {
                identity::describe(id).map(|program| OpenProgram { windows, ..program })
            })
            .collect();
        programs.sort_by_key(|program| program.shown_name().to_lowercase());
        if published.written.as_ref() == Some(&programs) {
            return;
        }
        let done = write_to(&published.folder, &programs);
        if done.is_err() {
            let _ = remove_from(&published.folder);
        }
        published.written = Some(programs);
    });
}
