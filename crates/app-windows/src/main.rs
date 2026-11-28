//! Guesthouse's app on a Windows host, `guesthouse-app.exe`: a window that lists the machines and
//! starts, stops, makes, changes and deletes them, shows what each is doing, and says how to reach
//! its guest.
//!
//! The Guesthouse service does all of that, as the system, because HCS needs it. The app runs as its
//! user and asks the service on its pipe, through `guesthouse-control`, and holds no rule of its own.
//! It's WinUI 3 through Microsoft's windows-reactor: `app.rs` is the app's state and what changes
//! it, `window.rs` what's shown, `bridge.rs` the calls to the service, `screen.rs` remoteman's
//! viewer, `console.rs` a machine's own screen in Windows' Remote Desktop control, `report.rs` a bug
//! report's making and sending, `log.rs` a log's lines, `pace.rs` how often the window is told
//! things, `palette.rs` what Ctrl+K offers and finds, `remote_view.rs` what the Remote View group shows and asks, `shell.rs` how
//! the window gets about, `preferences.rs` the app's own settings, `update.rs` whether a newer
//! Guesthouse is out, `hyper_v.rs` what's said of Windows' Hyper-V feature, which bridging needs,
//! and `system.rs` the rest of what's asked of Windows.

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod bridge;
#[cfg(windows)]
mod console;
#[cfg(any(windows, test))]
mod hyper_v;
#[cfg(any(windows, test))]
mod log;
#[cfg(any(windows, test))]
mod pace;
#[cfg(any(windows, test))]
mod palette;
#[cfg(any(windows, test))]
mod preferences;
#[cfg(any(windows, test))]
mod remote_view;
#[cfg(windows)]
mod report;
#[cfg(windows)]
mod screen;
#[cfg(any(windows, test))]
mod shell;
#[cfg(windows)]
mod system;
#[cfg(any(windows, test))]
mod update;
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
    let ran = windows_reactor::App::run_component::<app::Main>(start);
    // Nobody is left to answer a device that comes to pair. Only what the service's machines had
    // open is noted, so a preview sends nothing.
    bridge::close_pairing_left_open();
    if let Err(error) = ran {
        let _ = std::fs::write(system::panic_file(), format!("the window couldn't open: {error}\n"));
        std::process::exit(1);
    }
}
