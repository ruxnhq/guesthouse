//! The service manager's way in. The service starts when Windows does, before anyone signs in,
//! serves the pipe until it's stopped, and then asks running guests to shut down, as it does when
//! Windows shuts down too: it takes the longer warning Windows gives before shutting down, as a
//! guest needs more than the few seconds of the last one.

use super::security::wide;
use super::{GRACE, log, start};
use std::ffi::c_void;
use std::path::PathBuf;
use std::process::ExitCode;
use std::ptr::null_mut;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::time::Duration;
use windows_sys::Win32::Foundation::{ERROR_CALL_NOT_IMPLEMENTED, NO_ERROR};
use windows_sys::Win32::System::Services::{
    RegisterServiceCtrlHandlerExW, SERVICE_ACCEPT_PRESHUTDOWN, SERVICE_ACCEPT_STOP,
    SERVICE_CONTROL_INTERROGATE, SERVICE_CONTROL_PRESHUTDOWN, SERVICE_CONTROL_STOP, SERVICE_RUNNING,
    SERVICE_START_PENDING, SERVICE_STATUS, SERVICE_STATUS_CURRENT_STATE, SERVICE_STOP_PENDING,
    SERVICE_STOPPED, SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS, SetServiceStatus,
    StartServiceCtrlDispatcherW,
};
use windows_sys::core::PWSTR;

/// The service's name with the service manager, as the installer registers it.
pub const NAME: &str = "Guesthouse";

static STOP: AtomicBool = AtomicBool::new(false);
static STATUS: AtomicPtr<c_void> = AtomicPtr::new(null_mut());
/// The folder asked for on the service's command line, if any.
static ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Runs as the service, until the service manager stops it. Fails at once when the service
/// manager didn't start this process.
pub fn run(root: Option<PathBuf>) -> ExitCode {
    let _ = ROOT.set(root);
    let mut name = wide(NAME);
    let table = [
        SERVICE_TABLE_ENTRYW { lpServiceName: name.as_mut_ptr(), lpServiceProc: Some(service_main) },
        SERVICE_TABLE_ENTRYW { lpServiceName: null_mut(), lpServiceProc: None },
    ];
    // SAFETY: a table ending in an empty entry, whose name outlives the dispatcher.
    if unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } == 0 {
        eprintln!(
            "This is how the service manager starts the service. To serve from a prompt, use `guesthouse-service run`."
        );
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn report(state: SERVICE_STATUS_CURRENT_STATE, exit: u32, wait: Duration) {
    let handle = STATUS.load(Ordering::SeqCst);
    if handle.is_null() {
        return;
    }
    let accepted =
        if state == SERVICE_RUNNING { SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_PRESHUTDOWN } else { 0 };
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: accepted,
        dwWin32ExitCode: exit,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: wait.as_millis() as u32,
    };
    // SAFETY: the handle the service manager gave, and a whole status.
    unsafe { SetServiceStatus(handle, &status) };
}

unsafe extern "system" fn handler(
    control: u32,
    _kind: u32,
    _data: *mut c_void,
    _context: *mut c_void,
) -> u32 {
    match control {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_PRESHUTDOWN => {
            STOP.store(true, Ordering::SeqCst);
            NO_ERROR
        }
        SERVICE_CONTROL_INTERROGATE => NO_ERROR,
        _ => ERROR_CALL_NOT_IMPLEMENTED,
    }
}

unsafe extern "system" fn service_main(_count: u32, _arguments: *mut PWSTR) {
    let name = wide(NAME);
    // SAFETY: the name ends in a zero; the handler lives as long as the process.
    let handle = unsafe { RegisterServiceCtrlHandlerExW(name.as_ptr(), Some(handler), null_mut()) };
    if handle.is_null() {
        return;
    }
    STATUS.store(handle, Ordering::SeqCst);
    report(SERVICE_START_PENDING, NO_ERROR, Duration::from_secs(10));
    let service = match start(ROOT.get().cloned().flatten()) {
        Ok(service) => service,
        Err(error) => {
            log(&format!("The service couldn't start: {error}"));
            report(SERVICE_STOPPED, 1, Duration::ZERO);
            return;
        }
    };
    report(SERVICE_RUNNING, NO_ERROR, Duration::ZERO);
    while !STOP.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(250));
    }
    log("The service stops. Running guests are asked to shut down.");
    report(SERVICE_STOP_PENDING, NO_ERROR, GRACE + Duration::from_secs(15));
    service.stop_all(GRACE);
    log("The service has stopped.");
    report(SERVICE_STOPPED, NO_ERROR, Duration::ZERO);
}
