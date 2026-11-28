//! Who is who: security descriptors written as SDDL, the user a token belongs to, and whether a
//! token is an administrator's.

use std::ffi::{OsStr, c_void};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW,
    SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    CheckTokenMembership, CreateWellKnownSid, GetTokenInformation, OWNER_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR, PSID, SECURITY_MAX_SID_SIZE, TOKEN_GROUPS, TOKEN_QUERY, TOKEN_USER, TokenGroups,
    TokenUser, WinBuiltinAdministratorsSid,
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

/// Whether a token's account is in the Administrators group at all, enabled or, as in an
/// administrator's filtered token, for deny only: an account that could elevate.
pub fn in_administrators(token: HANDLE) -> bool {
    let mut needed = 0u32;
    // SAFETY: asking only for the size, with no buffer.
    unsafe { GetTokenInformation(token, TokenGroups, null_mut(), 0, &mut needed) };
    if needed == 0 {
        return false;
    }
    let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
    // SAFETY: the buffer has room for `needed` bytes.
    if unsafe { GetTokenInformation(token, TokenGroups, buffer.as_mut_ptr().cast(), needed, &mut needed) }
        == 0
    {
        return false;
    }
    // SAFETY: Windows wrote a TOKEN_GROUPS at the buffer's start, with its count of groups after it,
    // and their SIDs in the buffer too.
    unsafe {
        let groups = &*buffer.as_ptr().cast::<TOKEN_GROUPS>();
        let all = std::slice::from_raw_parts(groups.Groups.as_ptr(), groups.GroupCount as usize);
        all.iter().any(|group| sid_text(group.Sid).as_deref() == Some(ADMINISTRATORS))
    }
}

/// The owner of the file or folder at `path`, as a SID's text.
pub fn owner_of(path: &std::path::Path) -> Option<String> {
    let text = wide(path);
    let mut owner: PSID = null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: the path ends in a zero; the owner points into the descriptor, freed after it's read.
    let read = unsafe {
        GetNamedSecurityInfoW(
            text.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            null_mut(),
            null_mut(),
            &mut descriptor,
        )
    };
    if read != 0 {
        return None;
    }
    // SAFETY: the owner is a SID inside the descriptor, which is still allocated.
    let found = unsafe { sid_text(owner) };
    // SAFETY: allocated by Windows for us, and not used after this.
    unsafe { LocalFree(descriptor) };
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_belongs_to_the_user_who_made_it_and_windows_own_to_windows() {
        let file = std::env::temp_dir().join(format!("gh-owner-{}", std::process::id()));
        std::fs::write(&file, b"mine").unwrap();
        let mine = owner_of(&file);
        std::fs::remove_file(&file).unwrap();
        assert!(mine.is_some() && mine == own_user() || mine.as_deref() == Some(ADMINISTRATORS), "{mine:?}");
        let windows = std::env::var_os("SystemRoot").map(std::path::PathBuf::from).unwrap();
        let theirs = owner_of(&windows.join("System32")).unwrap();
        assert!(!theirs.starts_with("S-1-5-21-"), "{theirs}");
    }
}
