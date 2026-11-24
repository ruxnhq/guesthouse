//! A copy of each machine's last run's logs in the state folder, as the run folder is gone after a
//! reboot and another machine can take the slot. It's made when a run ends and again at the next start
//! in that slot, so a run the app never saw end, as when it crashed, is kept too. Each file is trimmed
//! to a size, and each copy replaces the one before, so it can't grow.

use crate::context::Context;
use crate::report::trim::trim;
use crate::run_files::{
    GUEST_LOGCAT, GUEST_SERIAL, HOST_LOGS, INSTALL_SERIAL, files_since, local_time, noted_run,
};
use crate::vm_host::MACHINE_NOTE;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const KEPT_FOLDER: &str = "kept-runs";
const NEW_SUFFIX: &str = ".new";
/// When the kept run started, as a reader would want it.
pub const STARTED_FILE: &str = "started";
/// The most of each log a copy keeps, from its start and its end.
const MOST_KEPT_BYTES: usize = 4 << 20;

/// Where a machine's kept logs are.
pub fn kept_folder(context: &Context, id: &str) -> PathBuf {
    context.paths.state.join(KEPT_FOLDER).join(id)
}

/// Every log a run can have that a report might take, the guest's serial output and Android's logcat
/// among them, which the report leaves out if the tester says so.
pub fn kept_names() -> Vec<&'static str> {
    HOST_LOGS.iter().copied().chain([GUEST_SERIAL, INSTALL_SERIAL, GUEST_LOGCAT]).collect()
}

/// Copies the run in this run folder into its machine's kept logs, if it has a note and any logs. A
/// copy of the same run replaces itself, with whatever the run wrote since.
pub fn keep_run(context: &Context, run_folder: &Path) -> io::Result<()> {
    let Some((id, since)) = noted_run(run_folder, MACHINE_NOTE) else { return Ok(()) };
    if !crate::machine::valid_id(&id) {
        return Ok(());
    }
    let files = files_since(run_folder, &kept_names(), "", since);
    if files.is_empty() {
        return Ok(());
    }
    let folder = kept_folder(context, &id);
    let mut new = folder.clone().into_os_string();
    new.push(NEW_SUFFIX);
    let new = PathBuf::from(new);
    let _ = fs::remove_dir_all(&new);
    fs::create_dir_all(&new)?;
    for (name, path) in files {
        let text = String::from_utf8_lossy(&fs::read(path)?).into_owned();
        fs::write(new.join(name), trim(&text, MOST_KEPT_BYTES))?;
    }
    fs::write(new.join(STARTED_FILE), local_time(since))?;
    let _ = fs::remove_dir_all(&folder);
    fs::rename(&new, &folder)
}

/// When the kept run started, if there is one.
pub fn kept_started(context: &Context, id: &str) -> Option<String> {
    let text = fs::read_to_string(kept_folder(context, id).join(STARTED_FILE)).ok()?;
    Some(text.trim().to_owned())
}

/// Takes a deleted machine's kept logs with it.
pub fn forget(context: &Context, id: &str) {
    let _ = fs::remove_dir_all(kept_folder(context, id));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Scratch, scratch_context};

    #[test]
    fn a_run_is_kept_and_replaces_the_one_before() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let run = scratch.path("run");
        fs::create_dir_all(&run).unwrap();
        fs::write(run.join(MACHINE_NOTE), "w11").unwrap();
        fs::write(run.join("daemon.out"), "first run").unwrap();
        keep_run(&context, &run).unwrap();
        let kept = kept_folder(&context, "w11");
        assert_eq!(fs::read_to_string(kept.join("daemon.out")).unwrap(), "first run");
        assert!(kept_started(&context, "w11").is_some());
        // The next run: the first one's daemon output is older than its note.
        let old = fs::File::options().append(true).open(run.join("daemon.out")).unwrap();
        old.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(60)).unwrap();
        fs::write(run.join(MACHINE_NOTE), "w11").unwrap();
        fs::write(run.join("qemu.log"), "second run").unwrap();
        keep_run(&context, &run).unwrap();
        assert!(!kept.join("daemon.out").exists(), "the copy before is replaced whole");
        assert_eq!(fs::read_to_string(kept.join("qemu.log")).unwrap(), "second run");
    }

    #[test]
    fn a_big_log_is_trimmed() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let run = scratch.path("run");
        fs::create_dir_all(&run).unwrap();
        fs::write(run.join(MACHINE_NOTE), "w11").unwrap();
        fs::write(run.join("daemon.out"), "line\n".repeat(MOST_KEPT_BYTES)).unwrap();
        keep_run(&context, &run).unwrap();
        let size = fs::metadata(kept_folder(&context, "w11").join("daemon.out")).unwrap().len();
        assert!(size <= MOST_KEPT_BYTES as u64 + 64, "{size} bytes");
    }

    #[test]
    fn a_folder_with_no_note_keeps_nothing() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let run = scratch.path("run");
        fs::create_dir_all(&run).unwrap();
        fs::write(run.join("daemon.out"), "whose?").unwrap();
        keep_run(&context, &run).unwrap();
        assert!(!context.paths.state.join(KEPT_FOLDER).exists());
    }

    #[test]
    fn forgetting_a_machine_takes_its_logs() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        fs::create_dir_all(kept_folder(&context, "w11")).unwrap();
        forget(&context, "w11");
        assert!(!kept_folder(&context, "w11").exists());
    }
}
