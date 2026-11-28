// SPDX-License-Identifier: GPL-2.0-or-later
//! One machine's frames read by more than one program at once, as the screen window and
//! `rmhost serve` do: `testsource` publishes, `rmhost serve` (with `-p`, the lab way, so the
//! viewer pairs unasked) and `rmhost stats` read, and `viewcheck` is the viewer. Both readers
//! must get frames over the same seconds. Needs the built rmhost, testsource and viewcheck;
//! without them it says so loudly and passes.
#![cfg(target_os = "macos")]

use guesthouse_core::tools::{self, Places, Tool};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Duration;

struct Guard(Child);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn(program: &Path, args: &[&str]) -> Guard {
    Guard(
        Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    )
}

#[test]
fn a_viewer_through_rmhost_serve_and_a_second_reader_both_get_frames_from_one_publisher() {
    let Ok(found) = tools::locate(Tool::Rmhost, &Places::on_this_mac(Tool::Rmhost, None, None)) else {
        eprintln!("\n*** readers SKIPPED: rmhost is not built here (set GUESTHOUSE_RMHOST) ***\n");
        return;
    };
    let rmhost = found.path;
    let (testsource, viewcheck) = (rmhost.with_file_name("testsource"), rmhost.with_file_name("viewcheck"));
    if !testsource.is_file() || !viewcheck.is_file() {
        eprintln!("\n*** readers SKIPPED: testsource or viewcheck is not built beside rmhost ***\n");
        return;
    }
    let root = std::env::temp_dir().join(format!("gh-readers-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let name = format!("gh.readers.{}", std::process::id());

    let _source = spawn(&testsource, &["-n", &name, "-t", "20"]);
    thread::sleep(Duration::from_millis(500));
    let state = root.join("serve");
    let _serve = spawn(
        &rmhost,
        &["serve", "-n", &name, "-p", "-l", "127.0.0.1:7886", "-s", &state.display().to_string()],
    );
    thread::sleep(Duration::from_secs(1));
    // The second reader: five seconds of frames, counted, while the viewer is connected.
    let stats = Command::new(&rmhost)
        .args(["stats", "5", "-n", &name])
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    thread::sleep(Duration::from_millis(500));
    let view = Command::new(&viewcheck)
        .args(["127.0.0.1:7886", &root.join("viewer").display().to_string(), "4"])
        .output()
        .unwrap();
    let stats = stats.wait_with_output().unwrap();

    let said = String::from_utf8_lossy(&view.stdout).into_owned();
    let seen: u32 = said
        .lines()
        .find(|l| l.contains(" frames ("))
        .and_then(|l| l.strip_prefix("viewcheck: ")?.split_whitespace().next()?.parse().ok())
        .unwrap_or(0);
    assert!(seen > 100, "the viewer behind rmhost serve got {seen} frames: {said}");

    let text = String::from_utf8_lossy(&stats.stderr).into_owned() + &String::from_utf8_lossy(&stats.stdout);
    let received: u32 = text
        .lines()
        .find(|l| l.contains("frames received"))
        .and_then(|l| l.split(" s: ").nth(1)?.split_whitespace().next()?.parse().ok())
        .unwrap_or(0);
    assert!(received > 100, "the second reader got {received} frames: {text}");
    let _ = std::fs::remove_dir_all(&root);
    eprintln!("readers: RAN, viewer {seen} frames, second reader {received} frames");
}
