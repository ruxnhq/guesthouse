//! The service on Windows: the service manager's way in ([`scm`]), the pipe the app asks on
//! ([`pipe`]), the folder the machines are kept in ([`folder`]), and the work, which is
//! `guesthouse-host-windows`'s.

mod folder;
mod pipe;
mod scm;
mod security;

use crate::requests::{Service, Work};
use crate::store::Store;
use guesthouse_control::{CardInfo, PIPE};
use guesthouse_hcs::gpu;
use guesthouse_hcs::host;
use guesthouse_hcs::system::ComputeSystem;
use guesthouse_host_windows::events::Sink;
use guesthouse_host_windows::settings::{BootSettings, PrepareSettings};
use guesthouse_host_windows::windows::{boot, prepare};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long running guests have to shut down when the service stops, before they're stopped.
const GRACE: Duration = Duration::from_secs(60);

/// The work itself, as `guesthouse-hcs-boot` does it.
pub struct HostWork;

impl Work for HostWork {
    fn prepare(&self, settings: &PrepareSettings, sink: &dyn Sink) -> Result<(), String> {
        prepare::prepare(settings, sink)
    }

    fn boot(
        &self,
        settings: &BootSettings,
        sink: Arc<dyn Sink>,
        stop: Arc<AtomicBool>,
    ) -> Result<(), String> {
        boot::boot(settings, sink, stop)
    }

    fn cards(&self) -> Vec<CardInfo> {
        match gpu::cards() {
            Ok(cards) => cards
                .iter()
                .map(|card| CardInfo { name: card.name.clone(), nvidia: card.is_nvidia() })
                .collect(),
            Err(error) => {
                log(&format!("The cards couldn't be listed: {error}"));
                Vec::new()
            }
        }
    }

    fn end(&self, id: &str) {
        if let Some(machine) = ComputeSystem::open(id) {
            log(&format!("{id} was still running, with nothing watching it, so it's stopped."));
            if let Err(error) = machine.terminate() {
                log(&format!("{id} couldn't be stopped: {error}"));
            }
        }
    }
}

/// Where the service writes what nobody is watching for: its start and stop, and refusals.
static LOG: OnceLock<PathBuf> = OnceLock::new();
/// Whether it runs at a prompt, where the log is printed too.
static AT_PROMPT: AtomicBool = AtomicBool::new(false);

pub(crate) fn log(text: &str) {
    if AT_PROMPT.load(Ordering::Relaxed) {
        eprintln!("{text}");
    }
    let Some(path) = LOG.get() else { return };
    let seconds = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs());
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{seconds} {text}");
    }
}

const USAGE: &str = "\
guesthouse-service run [--machines FOLDER]
    Serves the app from this prompt, which must be elevated, until Ctrl+C.
guesthouse-service service [--machines FOLDER]
    How the service manager starts it.

The machines are kept in %ProgramData%\\Guesthouse\\machines unless --machines names another
folder. The service makes that folder its own, for the system and administrators only, so it
must be a new folder, or one of the service's, in a place ordinary users can't change.";

pub fn main(args: Vec<String>) -> ExitCode {
    let (word, options) = args.split_first().map_or(("", &[][..]), |(word, rest)| (word.as_str(), rest));
    let root = match options {
        [] => None,
        [flag, folder] if flag == "--machines" => Some(PathBuf::from(folder)),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match word {
        "run" => run_here(root),
        "service" => scm::run(root),
        _ => {
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn run_here(root: Option<PathBuf>) -> ExitCode {
    AT_PROMPT.store(true, Ordering::Relaxed);
    if !host::is_elevated() {
        eprintln!(
            "The service needs an elevated prompt, as HCS does. Open Terminal with Run as administrator and run it again."
        );
        return ExitCode::FAILURE;
    }
    let ctrl_c = match host::catch_ctrl_c() {
        Ok(flag) => flag,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let service = match start(root) {
        Ok(service) => service,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "Serving the app on {PIPE}, with the machines in {}. Ctrl+C stops.",
        service.machines().store.root().display()
    );
    while !ctrl_c.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(250));
    }
    println!(
        "Stopping. Running guests are asked to shut down, and stopped if they haven't in {} s.",
        GRACE.as_secs()
    );
    service.stop_all(GRACE);
    ExitCode::SUCCESS
}

/// The machines' folder: the one asked for, or the service's own under ProgramData.
fn machines_folder(asked: Option<PathBuf>) -> (PathBuf, bool) {
    match asked {
        Some(folder) => (folder, false),
        None => {
            let data = std::env::var_os("ProgramData")
                .map_or_else(|| PathBuf::from(r"C:\ProgramData"), PathBuf::from);
            (data.join("Guesthouse").join("machines"), true)
        }
    }
}

/// Makes the machines' folder the service's own, then serves the pipe on threads of its own.
pub(crate) fn start(asked: Option<PathBuf>) -> Result<Arc<Service<HostWork>>, String> {
    let (root, own_parent) = machines_folder(asked);
    let root = std::path::absolute(&root).map_err(|error| format!("{}: {error}", root.display()))?;
    folder::secure(&root, own_parent)?;
    let _ = LOG.set(log_path(&root, own_parent));
    log(&format!("The service starts, version {}.", env!("CARGO_PKG_VERSION")));
    let service = Service::new(Store::new(root), HostWork);
    pipe::listen(PIPE, Arc::clone(&service))?;
    Ok(service)
}

/// The log goes beside the machines' folder when that's the service's own too, or in it.
fn log_path(root: &Path, own_parent: bool) -> PathBuf {
    match root.parent().filter(|_| own_parent) {
        Some(parent) => parent.join("service.log"),
        None => root.join("service.log"),
    }
}
