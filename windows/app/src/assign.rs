//! Making a machine that needs an owner the app's user's. Only an administrator asking elevated may
//! give a machine its owner, so the app starts itself again as a helper, through Windows' prompt
//! for an administrator of this PC to allow, and hands it the machine's name and the app's own
//! account. The helper asks the service for that one thing and ends, its exit code saying how it
//! went. Whoever allows it, the machine goes to the app's user, not to the account that allowed it.

// Off Windows only the tests use these.
#![cfg_attr(not(windows), allow(dead_code))]

/// The helper's first argument, then the machine's name and the account it's for.
pub const HELPER: &str = "--assign";

/// How the helper ended, as its exit code.
pub const GIVEN: u32 = 0;
pub const REFUSED: u32 = 1;
pub const UNREACHED: u32 = 2;
pub const NOT_A_HELPER: u32 = 3;

/// The machine and the account the helper was started for, when its arguments (the program's name
/// left out) are a helper's: [`HELPER`], a name a machine may have, and a user's account as a SID.
pub fn helper_args(args: &[String]) -> Option<(&str, &str)> {
    match args {
        [helper, name, owner]
            if helper == HELPER && guesthouse_control::valid_name(name) && users_sid(owner) =>
        {
            Some((name, owner))
        }
        _ => None,
    }
}

/// Whether `sid` reads as a local, domain or Microsoft Entra user's account, which are the ones a
/// machine can be given to. The service checks it again against this PC's users.
fn users_sid(sid: &str) -> bool {
    ["S-1-5-21-", "S-1-12-1-"].iter().any(|prefix| {
        sid.strip_prefix(prefix).is_some_and(|rest| {
            rest.split('-')
                .all(|part| !part.is_empty() && part.len() <= 10 && part.bytes().all(|b| b.is_ascii_digit()))
        })
    })
}

/// What the app says when the helper ended with `code`.
pub fn ended(name: &str, code: u32) -> Result<(), String> {
    match code {
        GIVEN => Ok(()),
        REFUSED => Err(format!("The service didn't make {name} yours. Its log says why.")),
        UNREACHED => Err("The helper couldn't reach the Guesthouse service.".to_owned()),
        _ => Err(format!("The helper ended without making {name} yours.")),
    }
}

#[cfg(windows)]
pub use on_windows::{ask_elevated, helper, own_account};

#[cfg(windows)]
mod on_windows {
    use super::*;
    use guesthouse_control::{Answer, Request, client};
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, GetLastError, HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetExitCodeProcess, INFINITE, OpenProcessToken, WaitForSingleObject,
    };
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    /// Runs the helper with the program's arguments, its name left out, and returns its exit code.
    pub fn helper(args: &[String]) -> u32 {
        let Some((name, owner)) = helper_args(args) else { return NOT_A_HELPER };
        match client::ask(&Request::Assign { name: name.to_owned(), owner: owner.to_owned() }) {
            Ok(Answer::Ok) => GIVEN,
            Ok(_) => REFUSED,
            Err(_) => UNREACHED,
        }
    }

    /// Starts the helper for `name`, through Windows' prompt, and waits for it. `Ok` once the
    /// machine is `owner`'s.
    pub fn ask_elevated(name: &str, owner: &str) -> Result<(), String> {
        let exe =
            std::env::current_exe().map_err(|error| format!("The app couldn't find itself: {error}"))?;
        let wide = |text: &str| text.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        let file: Vec<u16> = exe.as_os_str().encode_wide().chain(Some(0)).collect();
        let (verb, parameters) = (wide("runas"), wide(&format!("{HELPER} {name} {owner}")));
        // SAFETY: a plain struct of numbers and pointers, all of which may be zero.
        let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
        info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = parameters.as_ptr();
        info.nShow = SW_HIDE;
        // SAFETY: the strings outlive the call, and the process's handle is ours after it.
        if unsafe { ShellExecuteExW(&mut info) } == 0 {
            // SAFETY: a plain question about the call just made.
            return Err(match unsafe { GetLastError() } {
                ERROR_CANCELLED => format!("{name} still needs an owner: the prompt was turned down."),
                error => format!("Windows didn't start the helper (error {error})."),
            });
        }
        let process: HANDLE = info.hProcess;
        if process.is_null() {
            return Err("Windows started the helper but gave no way to wait for it.".to_owned());
        }
        let mut code = NOT_A_HELPER;
        // SAFETY: the process's handle is ours, waited on, read and closed here.
        unsafe {
            WaitForSingleObject(process, INFINITE);
            if GetExitCodeProcess(process, &mut code) == 0 {
                code = NOT_A_HELPER;
            }
            CloseHandle(process);
        }
        ended(name, code)
    }

    /// The account this app runs as, as a SID's text.
    pub fn own_account() -> Option<String> {
        let mut token: HANDLE = null_mut();
        // SAFETY: the pseudo-handle for this process needs no closing, and `token` is a place for it.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return None;
        }
        let user = token_user(token);
        // SAFETY: ours, and not used after this.
        unsafe { CloseHandle(token) };
        user
    }

    fn token_user(token: HANDLE) -> Option<String> {
        let mut needed = 0u32;
        // SAFETY: asking only for the size, with no buffer.
        unsafe { GetTokenInformation(token, TokenUser, null_mut(), 0, &mut needed) };
        if needed == 0 {
            return None;
        }
        // u64s, so the TOKEN_USER at its start is aligned.
        let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
        // SAFETY: the buffer has room for `needed` bytes.
        if unsafe { GetTokenInformation(token, TokenUser, buffer.as_mut_ptr().cast(), needed, &mut needed) }
            == 0
        {
            return None;
        }
        let mut text: *mut u16 = null_mut();
        // SAFETY: Windows wrote a TOKEN_USER at the buffer's start, whose SID is in the buffer too,
        // and the text comes back allocated for us to free.
        if unsafe { ConvertSidToStringSidW((*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid, &mut text) } == 0
        {
            return None;
        }
        // SAFETY: Windows wrote a string ending in a zero.
        let length = (0..).take_while(|&index| unsafe { *text.add(index) } != 0).count();
        // SAFETY: as above, `length` units are there.
        let owned = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
        // SAFETY: allocated by Windows for us, and not used after this.
        unsafe { LocalFree(text.cast()) };
        Some(owned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn the_helper_takes_a_machines_name_and_a_users_account_and_nothing_else() {
        let user = "S-1-5-21-3623811015-3361044348-30300820-1013";
        assert_eq!(helper_args(&args(&[HELPER, "win-11_a", user])), Some(("win-11_a", user)));
        assert_eq!(
            helper_args(&args(&[HELPER, "win", "S-1-12-1-1-2-3-4"])),
            Some(("win", "S-1-12-1-1-2-3-4"))
        );
        for wrong in [
            args(&[HELPER, "win", "S-1-5-18"]),
            args(&[HELPER, "win", "S-1-5-32-544"]),
            args(&[HELPER, "win", "S-1-5-21-"]),
            args(&[HELPER, "win", "S-1-5-21-1--2"]),
            args(&[HELPER, "win", "S-1-5-21-1-2 x"]),
            args(&[HELPER, "w in", user]),
            args(&[HELPER, "", user]),
            args(&[HELPER, "win"]),
            args(&[HELPER, "win", user, "more"]),
            args(&["--other", "win", user]),
            args(&[]),
        ] {
            assert_eq!(helper_args(&wrong), None, "{wrong:?}");
        }
    }

    #[test]
    fn the_helpers_exit_code_is_said_in_words() {
        assert_eq!(ended("win", GIVEN), Ok(()));
        assert!(ended("win", REFUSED).unwrap_err().contains("didn't make win yours"));
        assert!(ended("win", UNREACHED).unwrap_err().contains("couldn't reach"));
        assert!(ended("win", 259).is_err());
    }
}
