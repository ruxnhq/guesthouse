//! What a macOS machine's install does today, held before its plain half moves: the steps it says,
//! how far reims-util says it is, the names its parts are made under, that a part already made is
//! kept so an install run again goes on where it stopped, and every sentence.

use super::*;
use crate::actions::tests::machine;
use crate::testing::{Scratch, scratch_context};
use crate::vm_host::stand_in::RecordingHost;

/// A reims-util that keeps its words in `asked` beside it and prints `said`, then exits with `code`.
fn reims_util(context: &crate::context::Context, scratch: &Scratch, said: &str, errors: &str, code: i32) {
    let program = macos_part::file(&context.paths, REIMS_UTIL);
    let asked = program.with_file_name("asked");
    scratch.program(
        program.strip_prefix(scratch.root()).unwrap().to_str().unwrap(),
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\ncat <<'END'\n{said}END\nprintf '%s' '{errors}' >&2\nexit {code}\n",
            asked.display()
        ),
    );
}

fn asked(context: &crate::context::Context) -> String {
    fs::read_to_string(macos_part::file(&context.paths, "asked")).unwrap_or_default()
}

/// A part already there is kept: the boot image with its firmware, and the installer.
#[test]
fn a_part_already_made_is_kept_and_nothing_is_said_of_it() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let mut mac = machine(&context);
    mac.opencore = scratch.write("mac/opencore.qcow2", "boot image").display().to_string();
    mac.iso = scratch.write("mac/installer.img", "installer").display().to_string();
    let host = RecordingHost::default();
    let task = Task::new(&context, &host, "install", mac);
    make_boot_image(&task, &scratch.path("mac")).unwrap();
    make_installer(&task).unwrap();
    assert!(task.said.lock().unwrap().is_empty());
    assert_eq!(asked(&context), "", "reims-util was never run");
}

/// Apple's installer: what reims-util is asked, each percent it says once, and the image made ready
/// beside the installer's place as `.part`, which goes when qemu-img can't.
#[test]
fn apples_installer_is_fetched_by_reims_util_and_made_ready_under_another_name() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let mut mac = machine(&context);
    mac.iso = scratch.path("mac/installer.img").display().to_string();
    fs::create_dir_all(scratch.path("mac")).unwrap();
    // What an earlier try left half written out.
    scratch.write("mac/installer.part", "half an installer");
    let image = scratch.write("apple/BaseSystem.dmg", "not a dmg");
    reims_util(
        &context,
        &scratch,
        &format!(
            "{{\"event\":\"progress\",\"done\":0,\"total\":10}}\n{{\"event\":\"progress\",\"done\":0,\"total\":10}}\n\
             {{\"event\":\"progress\",\"done\":5}}\n{{\"event\":\"progress\",\"done\":5,\"total\":0}}\n\
             not json\n{{\"event\":\"progress\",\"done\":5,\"total\":10}}\n\
             {{\"event\":\"progress\",\"done\":10,\"total\":10}}\n{{\"event\":\"done\",\"result\":{{\"image\":\"{}\"}}}}\n",
            image.display()
        ),
        "",
        0,
    );
    let host = RecordingHost::default();
    let task = Task::new(&context, &host, "install", mac);
    let error = make_installer(&task).unwrap_err();
    assert!(error.0.starts_with("The installer could not be made ready: "), "{error}");
    assert_eq!(error.0, error.0.trim_end(), "qemu-img's words are trimmed");
    assert_eq!(
        task.said.lock().unwrap().as_slice(),
        [
            "Downloading macOS from Apple",
            "Downloading macOS from Apple: 0%",
            "Downloading macOS from Apple: 50%",
            "Downloading macOS from Apple: 100%",
            "Getting the installer ready",
        ]
    );
    let kept = context.paths.store.join("macos-installers");
    assert_eq!(asked(&context), format!("--json online-installer tahoe -o {}\n", kept.display()));
    assert!(kept.is_dir());
    assert!(!scratch.path("mac/installer.part").exists() && !scratch.path("mac/installer.img").exists());
}

/// The boot image is made in `boot.part` beside the machine's files, from the guest drivers' list.
#[test]
fn the_boot_image_is_made_in_boot_part() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let mut mac = machine(&context);
    mac.opencore = scratch.path("mac/opencore.qcow2").display().to_string();
    mac.mac = "52:54:00:12:34:56".to_owned();
    let kexts = macos_part::file(&context.paths, KEXTS_FOLDER);
    fs::create_dir_all(&kexts).unwrap();
    fs::write(kexts.join(KEXT_SET), "[[kext]]\nBundlePath = \"Any.kext\"\n").unwrap();
    reims_util(&context, &scratch, "", "", 0);
    let host = RecordingHost::default();
    let task = Task::new(&context, &host, "install", mac);
    let folder = scratch.path("mac");
    let error = make_boot_image(&task, &folder).unwrap_err();
    assert_eq!(error.0, "reims-util ended without saying what it made.");
    assert_eq!(task.said.lock().unwrap().as_slice(), ["Making the Mac's boot image"]);
    let parts = folder.join("boot.part");
    assert_eq!(
        asked(&context),
        format!(
            "--json opencore -o {} --rom 52:54:00:12:34:56 --add-kext {}\n",
            parts.join("opencore.qcow2").display(),
            kexts.join("Any.kext").display()
        )
    );
    assert!(parts.is_dir(), "a boot image that wasn't made leaves its folder for the next try to clear");
}

/// What reims-util says when it fails, in each way.
#[test]
fn reims_util_s_refusals() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let host = RecordingHost::default();
    let task = Task::new(&context, &host, "install", machine(&context));
    let words = ["anything".to_owned()];
    reims_util(&context, &scratch, "{\"event\":\"error\",\"message\":\"Apple said no.\"}\n", "", 1);
    assert_eq!(reims_util_run(&task, &words), "Apple said no.");
    reims_util(&context, &scratch, "", "  it broke  \n", 3);
    assert_eq!(reims_util_run(&task, &words), "it broke");
    reims_util(&context, &scratch, "{\"event\":\"done\",\"result\":{}}\n", "", 2);
    assert_eq!(reims_util_run(&task, &words), "reims-util ended without saying what it made.");
}

fn reims_util_run(task: &Task, words: &[String]) -> String {
    super::reims_util(task, "Doing", words).unwrap_err().0
}
