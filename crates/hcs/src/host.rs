//! The host process itself: whether it's elevated, Ctrl+C, and new GUIDs.

use crate::error::{Error, Result, check};
use crate::text::guid_text;
use std::sync::atomic::{AtomicBool, Ordering};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows_sys::Win32::System::Com::CoCreateGuid;
use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows_sys::core::{BOOL, GUID};

/// Whether this process runs with an administrator's full token. HCS, HCN and attaching a disk all
/// refuse a process without one.
pub fn is_elevated() -> bool {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: the pseudo-handle for this process needs no closing, and `token` is a place for the
    // token's handle.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return false;
    }
    let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut written = 0;
    // SAFETY: the buffer is a TOKEN_ELEVATION and its size is the one given.
    let asked = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            (&raw mut elevation).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut written,
        )
    };
    // SAFETY: the token's handle is ours and isn't used after this.
    unsafe { CloseHandle(token) };
    asked != 0 && elevation.TokenIsElevated != 0
}

static STOP: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn on_console_event(_event: u32) -> BOOL {
    STOP.store(true, Ordering::SeqCst);
    // Handled: the process stays, so it can stop its machine and tidy up first.
    1
}

/// Makes Ctrl+C, Ctrl+Break and closing the console set a flag instead of ending the process, and
/// gives the flag.
pub fn catch_ctrl_c() -> Result<&'static AtomicBool> {
    // SAFETY: the handler only stores to an atomic, which is safe from the console's thread.
    if unsafe { SetConsoleCtrlHandler(Some(on_console_event), 1) } == 0 {
        return Err(Error::last("Catching Ctrl+C"));
    }
    Ok(&STOP)
}

pub fn new_guid() -> Result<GUID> {
    let mut guid = GUID::from_u128(0);
    // SAFETY: `guid` is a place for a GUID.
    check(unsafe { CoCreateGuid(&mut guid) }, "Making a GUID")?;
    Ok(guid)
}

/// A GUID as HCS writes one, with no braces.
pub fn guid_string(guid: &GUID) -> String {
    guid_text(guid)
}
