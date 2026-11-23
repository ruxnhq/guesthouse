//! The runner that goes into an Ubuntu guest, run for real with `sh`, a fake `apt-get` and a fake
//! `systemctl` first in the path, and folders of the test's own.

use crate::testing::Scratch;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

/// The runner from the repo, which is what a release copies into a guest.
fn runner() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../linux/recipe/run.sh")
}

struct Guest<'a> {
    scratch: &'a Scratch,
}

impl<'a> Guest<'a> {
    fn new(scratch: &'a Scratch) -> Self {
        scratch.write("guest/job", "");
        scratch.program("bin/apt-get", "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$GUEST/apt-calls\"\n[ ! -f \"$GUEST/apt-fails\" ] || exit 100\n");
        scratch.program("bin/systemctl", "#!/bin/sh\ncat \"$GUEST/jobs\" 2>/dev/null\n");
        Self { scratch }
    }

    fn job(&self, text: &str) {
        self.scratch.write("guest/job", text);
    }

    fn run(&self, words: &[&str]) -> Output {
        let path = format!("{}:{}", self.scratch.path("bin").display(), std::env::var("PATH").unwrap());
        Command::new("sh")
            .arg(runner())
            .args(words)
            .env("PATH", path)
            .env("GUEST", self.scratch.path("guest"))
            .env("GUESTHOUSE_RECIPE_FOLDER", self.scratch.path("guest"))
            .env("GUESTHOUSE_FIRST_BOOT_MARKER", self.scratch.path("guest/first-boot-done"))
            .env("GUESTHOUSE_UPDATE_PAUSE", "0")
            .output()
            .unwrap()
    }

    fn apt_calls(&self) -> String {
        fs::read_to_string(self.scratch.path("guest/apt-calls")).unwrap_or_default()
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn a_package_is_one_argument_after_two_dashes() {
    let scratch = Scratch::new();
    let guest = Guest::new(&scratch);
    guest.job("step 1 apt git\nstep 2 apt build-essential\n");
    assert!(guest.run(&["run", "2"]).status.success());
    assert_eq!(guest.apt_calls(), "install -y -o DPkg::Lock::Timeout=1800 -- build-essential\n");
}

#[test]
fn a_name_that_is_not_a_package_name_reaches_nothing() {
    let scratch = Scratch::new();
    let guest = Guest::new(&scratch);
    for (number, bad) in
        ["x;touch pwned", "-o Foo=1", "$(touch pwned)", "A", "a b", "`touch pwned`"].into_iter().enumerate()
    {
        guest.job(&format!("step {} apt {bad}\n", number + 1));
        let out = guest.run(&["run", &(number + 1).to_string()]);
        assert_eq!(out.status.code(), Some(2), "{bad}: {}", text(&out.stderr));
    }
    assert_eq!(guest.apt_calls(), "");
    assert!(!scratch.path("pwned").exists() && !scratch.path("guest/pwned").exists());
}

#[test]
fn a_script_runs_from_a_file_of_its_own_and_its_status_is_the_steps() {
    let scratch = Scratch::new();
    let guest = Guest::new(&scratch);
    let script = "echo \"hello from $0\" | sed 's/script.*//'\necho 'step 1 apt evil'\nexit 7\n";
    guest.job(&format!("step 1 apt git\nstep 2 script {}\n", crate::text::base64_encode(script.as_bytes())));
    let out = guest.run(&["run", "2"]);
    assert_eq!(out.status.code(), Some(7));
    let said = text(&out.stdout);
    assert!(said.contains("hello from") && said.contains("step 1 apt evil"), "{said}");
    assert_eq!(guest.apt_calls(), "", "what the script printed is not read as the job");
    let leftovers: Vec<_> = fs::read_dir(scratch.path("guest"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("script."))
        .collect();
    assert!(leftovers.is_empty(), "the script's file is removed");
}

#[test]
fn a_step_the_job_does_not_have_or_a_word_it_does_not_know_is_refused() {
    let scratch = Scratch::new();
    let guest = Guest::new(&scratch);
    guest.job("step 1 apt git\nstep 2 rm -rf\n");
    assert_eq!(guest.run(&["run", "9"]).status.code(), Some(2));
    assert_eq!(guest.run(&["run", "2"]).status.code(), Some(2));
    assert_eq!(guest.run(&["run", "1; id"]).status.code(), Some(2));
    assert_eq!(guest.run(&["run"]).status.code(), Some(2));
    assert_eq!(guest.run(&["nothing"]).status.code(), Some(2));
    assert_eq!(guest.apt_calls(), "");
}

#[test]
fn the_guest_is_ready_only_after_its_first_boot_is_over_and_it_has_restarted() {
    let scratch = Scratch::new();
    let guest = Guest::new(&scratch);
    assert_eq!(guest.run(&["ready"]).status.code(), Some(1), "no marker yet");
    scratch.write("guest/first-boot-done", "");
    assert_eq!(guest.run(&["ready"]).status.code(), Some(1), "the marker is newer than this boot");
    let marker = scratch.path("guest/first-boot-done");
    assert!(Command::new("touch").args(["-d", "@1"]).arg(&marker).status().unwrap().success());
    assert_eq!(guest.run(&["ready"]).status.code(), Some(0));
    scratch.write("guest/jobs", "1 reboot.target start waiting\n");
    assert_eq!(guest.run(&["ready"]).status.code(), Some(1), "a restart is on its way");
}

#[test]
fn the_guest_is_not_ready_while_its_gpu_step_waits_or_runs_and_is_once_it_has_ended() {
    let scratch = Scratch::new();
    let guest = Guest::new(&scratch);
    let marker = scratch.write("guest/first-boot-done", "");
    assert!(Command::new("touch").args(["-d", "@1"]).arg(&marker).status().unwrap().success());
    for (job, what) in [("start waiting", "waits for its turn"), ("start running", "is setting the GPU up")] {
        scratch.write("guest/jobs", format!("JOB UNIT TYPE STATE\n7 nvfwd-gpu.service {job}\n"));
        assert_eq!(guest.run(&["ready"]).status.code(), Some(1), "the GPU step {what}");
    }
    // Ended, with the GPU on or off, or a guest that has no GPU step: no job for it.
    scratch.write("guest/jobs", "JOB UNIT TYPE STATE\n9 nvfwd-desktop.service start waiting\n");
    assert_eq!(guest.run(&["ready"]).status.code(), Some(0));
    scratch.write("guest/jobs", "No jobs running.\n");
    assert_eq!(guest.run(&["ready"]).status.code(), Some(0));
}

#[test]
fn the_package_lists_are_tried_again_while_something_else_holds_them() {
    let scratch = Scratch::new();
    let guest = Guest::new(&scratch);
    scratch.program(
        "bin/apt-get",
        "#!/bin/sh\necho x >> \"$GUEST/tries\"\n[ \"$(wc -l < \"$GUEST/tries\")\" -ge 3 ]\n",
    );
    assert!(guest.run(&["prepare"]).status.success());
    assert_eq!(fs::read_to_string(scratch.path("guest/tries")).unwrap().lines().count(), 3);
}
