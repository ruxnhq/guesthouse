// SPDX-License-Identifier: GPL-2.0-or-later
//! Finding a process again. A pid alone is not enough: after a restart of the Mac or a long
//! time the number may belong to something else. A process is named by its pid and the time it
//! started, which a recycled pid will not share.

use std::fs;
use std::io;
use std::mem;
use std::path::Path;

/// A process: its pid and the moment it started, as seconds and microseconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pid {
    pub pid: i32,
    pub started: String,
}

/// The process's state value for a zombie: gone, only not yet collected by its parent.
const ZOMBIE: u32 = 5;

/// The process with this pid as it is now, or `None` when there is none (or it is a zombie).
pub fn identify(pid: i32) -> Option<Pid> {
    // SAFETY: `info` is plain old data and the call writes at most its size into it.
    let info = unsafe {
        let mut info: libc::proc_bsdinfo = mem::zeroed();
        let size = mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let got = libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        );
        if got != size {
            return None;
        }
        info
    };
    if info.pbi_status == ZOMBIE {
        return None;
    }
    Some(Pid { pid, started: format!("{}.{:06}", info.pbi_start_tvsec, info.pbi_start_tvusec) })
}

/// Whether the very process `pid` names is still there.
pub fn is_running(pid: &Pid) -> bool {
    identify(pid.pid).is_some_and(|now| now.started == pid.started)
}

/// Sends SIGTERM to the process, but only if it is still the one `pid` names.
pub fn terminate(pid: &Pid) -> bool {
    // SAFETY: kill has no memory effects.
    is_running(pid) && unsafe { libc::kill(pid.pid, libc::SIGTERM) } == 0
}

/// Saves `pid` as "PID STARTED" in `path`.
pub fn write_file(path: &Path, pid: &Pid) -> io::Result<()> {
    fs::write(path, format!("{} {}\n", pid.pid, pid.started))
}

/// Reads a file `write_file` made.
pub fn read_file(path: &Path) -> Option<Pid> {
    let text = fs::read_to_string(path).ok()?;
    let (pid, started) = text.trim().split_once(' ')?;
    Some(Pid { pid: pid.parse().ok()?, started: started.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn knows_itself_and_nothing_at_a_free_pid() {
        let me = identify(std::process::id() as i32).unwrap();
        assert!(is_running(&me));
        assert!(!is_running(&Pid { pid: me.pid, started: "1.000000".into() }));
        assert!(identify(i32::MAX).is_none());
    }

    #[test]
    fn a_child_is_seen_until_it_is_collected_and_a_stale_pid_file_does_not_match() {
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let pid = identify(child.id() as i32).unwrap();
        assert!(is_running(&pid));
        let file = std::env::temp_dir().join(format!("gh-pid-{}", std::process::id()));
        write_file(&file, &pid).unwrap();
        assert_eq!(read_file(&file), Some(pid.clone()));
        let _ = fs::remove_file(&file);
        assert!(terminate(&pid));
        child.wait().unwrap();
        assert!(!is_running(&pid));
        assert!(!terminate(&pid));
    }

    #[test]
    fn a_broken_pid_file_reads_as_nothing() {
        let file = std::env::temp_dir().join(format!("gh-pid-bad-{}", std::process::id()));
        fs::write(&file, "not a pid").unwrap();
        assert_eq!(read_file(&file), None);
        let _ = fs::remove_file(&file);
        assert_eq!(read_file(&file), None);
    }
}
