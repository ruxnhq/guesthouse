//! Guesthouse's app on a Windows host, `guesthouse-app.exe`: a window that lists the machines and
//! starts, stops, makes and deletes them, shows what each is doing, and says how to reach its guest.
//!
//! The Guesthouse service does all of that, as the system, because HCS needs it. The app runs as its
//! user and asks the service on its pipe, through `guesthouse-control`, and holds no rule of its own.
//! It's WinUI 3 through Microsoft's windows-reactor: `app.rs` is the app's state and what changes
//! it, `window.rs` what's shown, `bridge.rs` the calls to the service, `screen.rs` remoteman's viewer, and `system.rs` the rest of
//! what's asked of Windows.

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod bridge;
#[cfg(windows)]
mod screen;
#[cfg(windows)]
mod system;
#[cfg(windows)]
mod window;

#[cfg(not(windows))]
fn main() {
    eprintln!(
        "guesthouse-app is Guesthouse's app for a Windows host, and runs only there. On Linux, run guesthouse."
    );
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    // With no console, a panic would otherwise leave nothing behind.
    std::panic::set_hook(Box::new(|info| {
        let path = system::panic_file();
        if let Some(folder) = path.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        let _ = std::fs::write(path, format!("{info}\n"));
    }));
    let start = app::Start::new();
    if let Err(error) = windows_reactor::App::run_component::<app::Main>(start) {
        let _ = std::fs::write(system::panic_file(), format!("the window couldn't open: {error}\n"));
        std::process::exit(1);
    }
}
