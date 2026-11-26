use guesthouse_app_core::remote_view::{self, Lan, Mdns, Settings};
use guesthouse_relay::signal;
use std::path::Path;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// How often it looks at whether it was asked to end and whether QEMU is still there. It is also how
/// long a machine's stop leaves the program behind, at most.
const LOOK_AGAIN: Duration = Duration::from_millis(200);
const PROCESSES: &str = "/proc";

pub fn main() -> ExitCode {
    let settings = match Settings::parse(std::env::args().skip(1)) {
        Ok(settings) => settings,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    match run(&settings) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("guesthouse-remote-view: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(settings: &Settings) -> std::io::Result<()> {
    let processes = Path::new(PROCESSES);
    signal::watch_for_end()?;
    remote_view::write_pid_file(&settings.run_folder, processes)?;
    let result = serve_until_done(settings, processes);
    let _ = std::fs::remove_file(settings.run_folder.join(remote_view::PID_FILE));
    let _ = std::fs::remove_file(remote_view::socket_path(&settings.run_folder));
    result
}

fn serve_until_done(settings: &Settings, processes: &Path) -> std::io::Result<()> {
    let listener = remote_view::listener(&settings.run_folder)?;
    let lan = Arc::new(Mutex::new(Lan::with_announcer(settings, Box::new(Mdns::default()))));
    {
        let lan = Arc::clone(&lan);
        // It ends with the process: nothing waits for it, and a request in flight is not worth waiting for.
        thread::Builder::new().name("control".into()).spawn(move || {
            remote_view::serve(&listener, |request| {
                lan.lock().unwrap_or_else(|held| held.into_inner()).handle(request)
            });
        })?;
    }
    while !signal::asked_to_end() && settings.qemu.is_none_or(|qemu| qemu.is_running(processes)) {
        thread::sleep(LOOK_AGAIN);
    }
    lan.lock().unwrap_or_else(|held| held.into_inner()).stop();
    Ok(())
}
