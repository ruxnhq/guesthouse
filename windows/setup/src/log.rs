//! The installer's lines: to its own log (`%TEMP%\guesthouse-setup.log`, or `--log`), from
//! each of its processes, and, once the install folder is there, to `setup.log` in it too, where
//! setup.ps1 writes, so that one file tells the whole install.
//!
//! The installer's own log is the user's file, made by the starter as the user. An elevated
//! process never makes it and adds to it only while it is a plain file the user owns
//! (`system::plain_and_owned`), opened without following a reparse point; otherwise its lines go
//! to `setup.log` only.

use std::io::{self, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Storage::FileSystem::{
    FILE_APPEND_DATA, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, READ_CONTROL, SYNCHRONIZE, WRITE_OWNER,
};
use windows::Win32::System::SystemInformation::GetLocalTime;

use crate::system;

static FILE: OnceLock<PathBuf> = OnceLock::new();
static ELEVATED: OnceLock<bool> = OnceLock::new();
static ALSO: Mutex<Option<PathBuf>> = Mutex::new(None);
static GATE: Mutex<()> = Mutex::new(());

pub fn init(file: Option<PathBuf>, elevated: bool) {
    let file = file.unwrap_or_else(|| std::env::temp_dir().join("guesthouse-setup.log"));
    let _ = FILE.set(file);
    let _ = ELEVATED.set(elevated);
}

/// The installer's own log.
pub fn path() -> PathBuf {
    FILE.get().cloned().unwrap_or_else(|| std::env::temp_dir().join("guesthouse-setup.log"))
}

/// Also write to this file (the install folder's setup.log), or stop.
pub fn also(file: Option<PathBuf>) {
    *ALSO.lock().unwrap() = file;
}

/// The time as setup.ps1 writes it (`Get-Date -Format s`).
pub fn stamp() -> String {
    // SAFETY: plain call.
    let t = unsafe { GetLocalTime() };
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

pub fn say(line: &str) {
    let _gate = GATE.lock();
    let pid = std::process::id();
    let elevated = ELEVATED.get().copied().unwrap_or(true);
    append_own(&path(), &format!("{} [{pid}] {line}\n", stamp()), elevated);
    if let Some(also) = ALSO.lock().unwrap().clone() {
        append(&also, &format!("{} setup.exe: {line}\r\n", stamp()));
    }
}

/// A file's lines, added to the installer's own log as they are (the install folder's setup.log,
/// before the folder goes).
pub fn keep(file: &Path) {
    let Ok(text) = std::fs::read_to_string(file) else { return };
    say(&format!("{} as it was:", file.display()));
    let _gate = GATE.lock();
    append_own(&path(), &format!("{}\n", text.trim_end()), ELEVATED.get().copied().unwrap_or(true));
}

/// The installer's own log, opened to add to it: by a part another process started elevated, only
/// an existing plain file the user owns; by the process that was opened, which the path came to
/// on its own command line, made if it isn't there. One that is elevated gives a log it makes to its
/// own user, so the parts it starts may add to it.
pub fn open_own(path: &Path, elevated: bool) -> io::Result<std::fs::File> {
    if !elevated {
        if system::is_elevated() {
            let made = std::fs::OpenOptions::new()
                .access_mode((FILE_APPEND_DATA | WRITE_OWNER | READ_CONTROL | SYNCHRONIZE).0)
                .create_new(true)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
                .open(path);
            if let Ok(file) = made {
                system::give_to_own_user(&file)?;
                return Ok(file);
            }
        }
        return std::fs::OpenOptions::new().create(true).append(true).open(path);
    }
    let file = std::fs::OpenOptions::new()
        .access_mode((FILE_APPEND_DATA | READ_CONTROL | FILE_READ_ATTRIBUTES | SYNCHRONIZE).0)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)?;
    system::plain_and_owned(&file)?;
    Ok(file)
}

fn append_own(path: &Path, text: &str, elevated: bool) {
    for _ in 0..5 {
        match open_own(path, elevated) {
            Ok(mut file) => {
                let _ = file.write_all(text.as_bytes());
                return;
            }
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(32) => {
                std::thread::sleep(std::time::Duration::from_millis(20))
            }
            Err(_) => return,
        }
    }
}

fn append(path: &Path, text: &str) {
    // Opened and closed for each line: setup.ps1 appends to the same file between them.
    for _ in 0..5 {
        match std::fs::OpenOptions::new().create(true).append(true).open(path) {
            Ok(mut file) => {
                let _ = file.write_all(text.as_bytes());
                return;
            }
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("guesthouse-setup-test-log-{name}-{}", std::process::id()));
        let _ = std::process::Command::new("cmd").args(["/c", "rd", "/s", "/q"]).arg(&dir).status();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn elevated_lines_go_to_the_users_own_file() {
        let dir = folder("plain");
        let log = dir.join("guesthouse-setup.log");
        append_own(&log, "made by the starter\n", false);
        append_own(&log, "from the elevated part\n", true);
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "made by the starter\nfrom the elevated part\n");
    }

    #[test]
    fn elevated_lines_make_no_file() {
        let dir = folder("missing");
        let log = dir.join("guesthouse-setup.log");
        append_own(&log, "from the elevated part\n", true);
        assert!(!log.exists(), "the elevated part made the log");
    }

    #[test]
    fn elevated_lines_follow_no_hard_link() {
        let dir = folder("hardlink");
        let other = dir.join("other.txt");
        std::fs::write(&other, "other\n").unwrap();
        let log = dir.join("guesthouse-setup.log");
        std::fs::hard_link(&other, &log).unwrap();
        append_own(&log, "from the elevated part\n", true);
        assert_eq!(std::fs::read_to_string(&other).unwrap(), "other\n", "a line went through the hard link");
    }

    #[test]
    fn elevated_lines_follow_no_junction() {
        let dir = folder("junction");
        let elsewhere = dir.join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        let link = dir.join("link");
        let made = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/j"])
            .arg(&link)
            .arg(&elsewhere)
            .output()
            .unwrap();
        assert!(made.status.success(), "mklink /j: {}", String::from_utf8_lossy(&made.stdout));
        append_own(&link.join("guesthouse-setup.log"), "from the elevated part\n", true);
        assert!(!elsewhere.join("guesthouse-setup.log").exists(), "a file was made through the junction");
    }

    #[test]
    fn a_log_given_to_its_user_is_theirs_to_add_to() {
        let dir = folder("given");
        let log = dir.join("guesthouse-setup.log");
        let file = std::fs::OpenOptions::new()
            .access_mode((FILE_APPEND_DATA | WRITE_OWNER | READ_CONTROL | SYNCHRONIZE).0)
            .create_new(true)
            .open(&log)
            .unwrap();
        system::give_to_own_user(&file).unwrap();
        system::plain_and_owned(&file).unwrap();
        drop(file);
        append_own(&log, "from the elevated part\n", true);
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "from the elevated part\n");
    }

    #[test]
    fn a_file_of_windows_is_not_the_users() {
        let hosts =
            PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join(r"System32\drivers\etc\hosts");
        let file = std::fs::OpenOptions::new()
            .access_mode((READ_CONTROL | FILE_READ_ATTRIBUTES).0)
            .open(&hosts)
            .unwrap();
        let refused = system::plain_and_owned(&file).unwrap_err();
        assert!(refused.to_string().contains("someone else's"), "{refused}");
    }
}
