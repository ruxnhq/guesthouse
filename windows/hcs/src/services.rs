//! Windows' services, only as far as asking whether one is installed and running.

use crate::text::wide;
use windows_sys::Win32::System::Services::{
    CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatus, SC_MANAGER_CONNECT,
    SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_STATUS,
};

/// How a service stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// No service has this name, as with Hyper-V's `vmms` where Hyper-V isn't installed.
    Missing,
    Stopped,
    Running,
}

/// Whether the service `name` is installed, and running. A service that can't be asked about, as
/// the manager can't be opened, reads as missing.
pub fn standing(name: &str) -> Standing {
    // SAFETY: the null names are the local machine and the default database.
    let manager = unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT) };
    if manager.is_null() {
        return Standing::Missing;
    }
    let name = wide(name);
    // SAFETY: the manager is open and the name ends in a zero.
    let service = unsafe { OpenServiceW(manager, name.as_ptr(), SERVICE_QUERY_STATUS) };
    let standing = if service.is_null() {
        Standing::Missing
    } else {
        let mut status: SERVICE_STATUS = SERVICE_STATUS {
            dwServiceType: 0,
            dwCurrentState: 0,
            dwControlsAccepted: 0,
            dwWin32ExitCode: 0,
            dwServiceSpecificExitCode: 0,
            dwCheckPoint: 0,
            dwWaitHint: 0,
        };
        // SAFETY: the service is open and `status` is a place for its status.
        let asked = unsafe { QueryServiceStatus(service, &mut status) };
        // SAFETY: the handle came from OpenServiceW and isn't used after this.
        unsafe { CloseServiceHandle(service) };
        if asked != 0 && status.dwCurrentState == SERVICE_RUNNING {
            Standing::Running
        } else {
            Standing::Stopped
        }
    };
    // SAFETY: the handle came from OpenSCManagerW and isn't used after this.
    unsafe { CloseServiceHandle(manager) };
    standing
}
