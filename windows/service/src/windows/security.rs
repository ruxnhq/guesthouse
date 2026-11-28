//! Who is who: security descriptors written as SDDL, the user a token belongs to, and whether a
//! token is an administrator's.

use std::ffi::{OsStr, c_void};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    CheckTokenMembership, CreateWellKnownSid, GetTokenInformation, PSECURITY_DESCRIPTOR, PSID,
    SECURITY_MAX_SID_SIZE, TOKEN_QUERY, TOKEN_USER, TokenUser, WinBuiltinAdministratorsSid,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows_sys::core::BOOL;

/// The system's SID, as text.
pub const SYSTEM: &str = "S-1-5-18";
/// The Administrators group's.
pub const ADMINISTRATORS: &str = "S-1-5-32-544";
/// TrustedInstaller's, which owns Windows' own folders.
pub const TRUSTED_INSTALLER: &str = "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";

/// `text` as Windows takes a string: UTF-16, ending in a zero.
pub fn wide(text: impl AsRef<OsStr>) -> Vec<u16> {
    text.as_ref().encode_wide().chain([0]).collect()
}

/// Windows' last error, as words, with what was being done.
pub fn last_error(doing: &str) -> String {
    format!("{doing}: {}", io::Error::last_os_error())
}

/// A security descriptor made from SDDL, freed when dropped.
pub struct Descriptor(PSECURITY_DESCRIPTOR);

impl Descriptor {
    pub fn new(sddl: &str) -> Result<Self, String> {
        let text = wide(sddl);
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: the text ends in a zero and outlives the call; the descriptor comes back in a
        // place for it, and is ours to free.
        let made = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                null_mut(),
            )
        };
        if made == 0 {
            return Err(last_error(&format!("Reading the security descriptor {sddl}")));
        }
        Ok(Descriptor(descriptor))
    }

    pub fn as_ptr(&self) -> PSECURITY_DESCRIPTOR {
        self.0
    }
}

// SAFETY: the descriptor is memory Windows allocated, read by any thread and freed by one.
unsafe impl Send for Descriptor {}
unsafe impl Sync for Descriptor {}

impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: the descriptor was allocated by Windows for us and isn't used after this.
        unsafe { LocalFree(self.0) };
    }
}

/// A SID as text, such as `S-1-5-18`.
///
/// # Safety
/// `sid` must point at a valid SID.
pub unsafe fn sid_text(sid: PSID) -> Option<String> {
    let mut text: *mut u16 = null_mut();
    // SAFETY: the caller gives a valid SID; the text comes back allocated for us to free.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return None;
    }
    // SAFETY: Windows wrote a string ending in a zero.
    let length = (0..).take_while(|&index| unsafe { *text.add(index) } != 0).count();
    // SAFETY: as above, `length` units are there.
    let units = unsafe { std::slice::from_raw_parts(text, length) };
    let owned = String::from_utf16_lossy(units);
    // SAFETY: allocated by Windows for us, and not used after this.
    unsafe { LocalFree(text.cast()) };
    Some(owned)
}

/// The user a token belongs to, as a SID's text.
pub fn token_user(token: HANDLE) -> Option<String> {
    let mut needed = 0u32;
    // SAFETY: asking only for the size, with no buffer.
    unsafe { GetTokenInformation(token, TokenUser, null_mut(), 0, &mut needed) };
    if needed == 0 {
        return None;
    }
    // u64s, so the TOKEN_USER at its start is aligned.
    let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
    // SAFETY: the buffer has room for `needed` bytes.
    let asked =
        unsafe { GetTokenInformation(token, TokenUser, buffer.as_mut_ptr().cast(), needed, &mut needed) };
    if asked == 0 {
        return None;
    }
    // SAFETY: Windows wrote a TOKEN_USER at the buffer's start, whose SID is in the buffer too.
    unsafe { sid_text((*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid) }
}

/// The user this process runs as.
pub fn own_user() -> Option<String> {
    let mut token: HANDLE = null_mut();
    // SAFETY: the pseudo-handle for this process needs no closing; the token comes back in a place
    // for it.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return None;
    }
    let user = token_user(token);
    // SAFETY: ours, and not used after this.
    unsafe { CloseHandle(token) };
    user
}

/// Whether an impersonation token has the Administrators group enabled, as an elevated
/// administrator's does and a filtered one's doesn't.
pub fn is_administrator(token: HANDLE) -> bool {
    let mut sid = [0u8; SECURITY_MAX_SID_SIZE as usize];
    let mut size = sid.len() as u32;
    // SAFETY: the buffer has room for any SID, and its size is the one given.
    if unsafe {
        CreateWellKnownSid(WinBuiltinAdministratorsSid, null_mut(), sid.as_mut_ptr().cast(), &mut size)
    } == 0
    {
        return false;
    }
    let mut member: BOOL = 0;
    // SAFETY: an impersonation token of the caller's and a SID Windows just wrote.
    let checked = unsafe { CheckTokenMembership(token, sid.as_mut_ptr().cast::<c_void>(), &mut member) };
    checked != 0 && member != 0
}
