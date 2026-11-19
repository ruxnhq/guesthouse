//! Why a VM's QEMU ended, in QEMU's own word for it, heard while it goes.
//!
//! Nobody here is QEMU's parent, so once it's gone all that's left is that it's gone, and a guest
//! that powered itself off looks the same as a QEMU that crashed. But QEMU says why it's shutting
//! down on its monitors just before it does. So while a machine runs, something listens on the
//! watch monitor, which nothing else uses then, and keeps the last reason in the run folder.

use crate::context::Context;
use crate::monitor::WATCH_SOCKET;
use crate::slots;
use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::thread;

/// Where the reason is kept until whoever notices the VM has gone takes it.
const REASON_FILE: &str = "ended-as";
/// QEMU's word for a guest that powered itself off, as Windows' and Linux's own shutdowns do.
pub const GUEST_SHUTDOWN: &str = "guest-shutdown";

fn reason_file(context: &Context, slot: u8) -> PathBuf {
    context.run_folder(Some(slots::number(slot))).join(REASON_FILE)
}

/// The reason in a line from the monitor, if the line is QEMU saying it's shutting down.
fn reason_in(line: &str) -> Option<String> {
    let said: Value = serde_json::from_str(line).ok()?;
    (said["event"] == "SHUTDOWN").then(|| said["data"]["reason"].as_str().map(str::to_owned)).flatten()
}

/// Listens until QEMU hangs up, keeping the last reason it gives. A reason from an earlier VM in
/// this folder isn't this one's, so it goes as soon as this one answers.
fn listen(socket: &Path, note: &Path) -> Option<()> {
    let mut writer = UnixStream::connect(socket).ok()?;
    let mut reader = BufReader::new(writer.try_clone().ok()?);
    let mut line = String::new();
    // Its hello, and then the one command it wants before it says anything else.
    reader.read_line(&mut line).ok()?;
    writer.write_all(format!("{}\n", json!({"execute": "qmp_capabilities"})).as_bytes()).ok()?;
    let _ = fs::remove_file(note);
    loop {
        line.clear();
        if reader.read_line(&mut line).ok()? == 0 {
            return Some(());
        }
        if let Some(reason) = reason_in(&line) {
            let _ = fs::write(note, reason);
        }
    }
}

/// Starts listening to the VM in this slot, on a thread that ends when its QEMU does. With no
/// watch monitor to listen on, as with a QEMU the user runs themselves, nothing is kept.
pub fn watch(context: &Context, slot: u8) {
    let socket = context.run_folder(Some(slots::number(slot))).join(WATCH_SOCKET);
    let note = reason_file(context, slot);
    thread::spawn(move || listen(&socket, &note));
}

/// What QEMU gave as its reason for ending, if anything heard it. It's taken, so it can't be
/// read as the reason for a later VM in this slot.
pub fn take_reason(context: &Context, slot: u8) -> Option<String> {
    let file = reason_file(context, slot);
    let reason = fs::read_to_string(&file).ok();
    let _ = fs::remove_file(file);
    reason.map(|reason| reason.trim().to_owned()).filter(|reason| !reason.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Scratch, scratch_context};
    use std::os::unix::net::UnixListener;

    #[test]
    fn only_qemus_word_about_shutting_down_is_a_reason() {
        let going = r#"{"timestamp": {"seconds": 1}, "event": "SHUTDOWN", "data": {"guest": true, "reason": "guest-shutdown"}}"#;
        assert_eq!(reason_in(going).as_deref(), Some(GUEST_SHUTDOWN));
        assert_eq!(
            reason_in(r#"{"event": "RESET", "data": {"guest": true, "reason": "guest-reset"}}"#),
            None
        );
        assert_eq!(reason_in(r#"{"return": {}}"#), None);
        assert_eq!(reason_in("not json"), None);
    }

    #[test]
    fn the_last_reason_qemu_gives_is_kept_and_an_older_one_is_not() {
        let scratch = Scratch::new();
        let socket = scratch.path("watch.sock");
        let note = scratch.path("ended-as");
        fs::write(&note, "from-the-vm-before").unwrap();
        let server = UnixListener::bind(&socket).unwrap();
        let qemu = thread::spawn(move || {
            let (mut stream, _) = server.accept().unwrap();
            stream.write_all(b"{\"QMP\": {}}\n").unwrap();
            let mut asked = String::new();
            BufReader::new(stream.try_clone().unwrap()).read_line(&mut asked).unwrap();
            stream.write_all(b"{\"return\": {}}\n").unwrap();
            stream.write_all(b"{\"event\": \"SHUTDOWN\", \"data\": {\"guest\": true, \"reason\": \"guest-shutdown\"}}\n").unwrap();
            asked
        });
        listen(&socket, &note);
        assert!(qemu.join().unwrap().contains("qmp_capabilities"));
        assert_eq!(fs::read_to_string(&note).unwrap(), GUEST_SHUTDOWN);
    }

    #[test]
    fn a_reason_is_taken_once() {
        let scratch = Scratch::new();
        let context = scratch_context(&scratch);
        let file = reason_file(&context, 1);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "guest-shutdown\n").unwrap();
        assert_eq!(take_reason(&context, 1).as_deref(), Some(GUEST_SHUTDOWN));
        assert_eq!(take_reason(&context, 1), None);
    }

    #[test]
    fn with_no_monitor_nothing_is_kept() {
        let scratch = Scratch::new();
        let note = scratch.path("ended-as");
        assert_eq!(listen(&scratch.path("gone.sock"), &note), None);
        assert!(!note.exists());
    }
}
