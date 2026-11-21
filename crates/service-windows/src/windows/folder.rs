//! The folder the machines are kept in, which the service makes its own: owned by the
//! administrators, and opened by nobody but the system and administrators. The service writes and
//! deletes in it as the system, for a user who isn't an administrator, so no such user may put
//! anything in it, least of all a link that leads the service somewhere else.

use super::security::{ADMINISTRATORS, Descriptor, SYSTEM, TRUSTED_INSTALLER, last_error, sid_text, wide};
use std::fs;
use std::path::Path;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{ERROR_SUCCESS, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    GetNamedSecurityInfoW, SE_FILE_OBJECT, SetNamedSecurityInfoW,
};
use windows_sys::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, GetSecurityDescriptorOwner,
    OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    SECURITY_ATTRIBUTES,
};
use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;

/// Owned by the administrators; the system and administrators may do anything in it and in
/// everything made in it, and nobody else anything. Protected, so a parent folder's rules aren't
/// added.
const PRIVATE: &str = "O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";

/// Makes `root`, and with `parent_too` its parent, the service's own. A folder that is there
/// already must be a folder, not a link, and owned by the system or administrators; one that isn't
/// is refused, as someone else made it and may still have a way in.
pub fn secure(root: &Path, parent_too: bool) -> Result<(), String> {
    let private = Descriptor::new(PRIVATE)?;
    let parent = root.parent().filter(|_| parent_too);
    for folder in parent.into_iter().chain([root]) {
        match fs::symlink_metadata(folder) {
            Err(_) => make(folder, &private)?,
            Ok(kind) if kind.file_type().is_symlink() || !kind.is_dir() => {
                return Err(format!(
                    "{} isn't a folder but a link or a file, so the service won't keep machines there.",
                    folder.display()
                ));
            }
            Ok(_) => {
                let owner = owner(folder)?;
                if ![SYSTEM, ADMINISTRATORS, TRUSTED_INSTALLER].contains(&owner.as_str()) {
                    return Err(format!(
                        "{} was made by {owner}, not by an administrator, so the service won't keep machines there. \
                         Delete it, or name another folder.",
                        folder.display()
                    ));
                }
                protect(folder, &private)?;
            }
        }
    }
    Ok(())
}

fn make(folder: &Path, private: &Descriptor) -> Result<(), String> {
    let path = wide(folder);
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: private.as_ptr(),
        bInheritHandle: 0,
    };
    // SAFETY: the path ends in a zero, and it and the security outlive the call.
    if unsafe { CreateDirectoryW(path.as_ptr(), &attributes) } == 0 {
        return Err(last_error(&format!("Making {}", folder.display())));
    }
    Ok(())
}

/// Gives a folder that's there already the service's owner and rules.
fn protect(folder: &Path, private: &Descriptor) -> Result<(), String> {
    let mut owner: PSID = null_mut();
    let mut dacl: *mut ACL = null_mut();
    let (mut present, mut defaulted) = (0, 0);
    // SAFETY: the descriptor is ours and valid; the owner and list come back pointing into it,
    // which outlives their use below.
    unsafe {
        GetSecurityDescriptorOwner(private.as_ptr(), &mut owner, &mut defaulted);
        GetSecurityDescriptorDacl(private.as_ptr(), &mut present, &mut dacl, &mut defaulted);
    }
    let path = wide(folder);
    let what = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION;
    // SAFETY: the path ends in a zero; the owner and list point into the descriptor, alive here.
    let code = unsafe {
        SetNamedSecurityInfoW(path.as_ptr(), SE_FILE_OBJECT, what, owner, null_mut(), dacl, null_mut())
    };
    if code != ERROR_SUCCESS {
        return Err(format!(
            "Making {} the service's own: {}",
            folder.display(),
            std::io::Error::from_raw_os_error(code as i32)
        ));
    }
    Ok(())
}

/// Who owns a folder, as a SID's text.
fn owner(folder: &Path) -> Result<String, String> {
    let path = wide(folder);
    let mut owner: PSID = null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: the path ends in a zero; the owner points into the descriptor, which comes back
    // allocated for us to free.
    let code = unsafe {
        GetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            null_mut(),
            null_mut(),
            &mut descriptor,
        )
    };
    if code != ERROR_SUCCESS {
        return Err(format!(
            "Reading who owns {}: {}",
            folder.display(),
            std::io::Error::from_raw_os_error(code as i32)
        ));
    }
    // SAFETY: Windows gave a valid SID inside the descriptor, freed only after this.
    let text = unsafe { sid_text(owner) };
    // SAFETY: allocated by Windows for us, and not used after this.
    unsafe { LocalFree(descriptor) };
    text.ok_or_else(|| format!("Reading who owns {}: the owner can't be read.", folder.display()))
}
