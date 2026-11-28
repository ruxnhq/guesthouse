use super::*;
use std::time::Duration;

#[test]
fn the_serial_words_are_the_serial_words_crates() {
    assert_eq!(INSTALL_STEPS[0], "Copying Windows to the disk");
    assert_eq!(install_step(b""), "Copying Windows to the disk");
    assert!(installed(b"nvfwd-install-finished\n"));
    assert_eq!(INSTALL_FINISHED, b"nvfwd-install-finished");
}

#[test]
fn every_wait_and_how_often_it_looks() {
    use windows_guest::*;
    assert_eq!((INSTALL_HOURS, INSTALL_QUIET_MINUTES, BOOT_PROMPT_SECONDS), (2, 10, 6));
    assert_eq!((SETUP_SCRIPT_SECONDS, CHECK_SCRIPT_SECONDS, INSTALL_STOP_SECONDS), (30, 60, 120));
    assert_eq!(FIRMWARE_WAIT, Duration::from_secs(30));
    assert_eq!(SETUP_POLL, Duration::from_secs(5));
    assert_eq!(SHORT_POLL, Duration::from_millis(200));
    assert_eq!(ubuntu_guest::INSTALL_HOURS, 2);
}

#[test]
fn a_disk_is_stuck_once_it_has_not_grown_for_more_than_ten_minutes() {
    use windows_guest::QuietDisk;
    let at = Duration::from_secs;
    let mut disk = QuietDisk::new(at(0));
    assert!(!disk.stuck(at(0), 0));
    assert!(!disk.stuck(at(600), 0), "ten minutes is not more than ten");
    assert!(disk.stuck(at(601), 0));
    let mut disk = QuietDisk::new(at(100));
    assert!(!disk.stuck(at(200), 5));
    assert!(!disk.stuck(at(800), 5), "it grew at 200 s");
    assert!(!disk.stuck(at(800), 4), "less is not growth");
    assert!(disk.stuck(at(801), 5));
    assert!(!disk.stuck(at(802), 6), "growth starts it again");
    assert!(!QuietDisk::new(at(10)).stuck(at(5), 0), "a clock behind is no time");
}

#[test]
fn every_windows_sentence() {
    use windows_guest::*;
    assert_eq!(stopped_during(""), "The VM stopped during the install.");
    assert_eq!(stopped_during("qemu: gone"), "The VM stopped during the install. qemu: gone");
    assert_eq!(
        stuck(),
        "Windows Setup has not written to the disk for 10 minutes, so it is stuck or waiting for an answer. \
         The VM is still up: Show Screen has what it says."
    );
    assert_eq!(not_finished(), "Windows setup did not finish in 2 hours. Show Screen says how far it got.");
    assert_eq!(missing_something("  \n"), "Windows is missing something: the agent went quiet");
    assert_eq!(missing_something(" no driver \n"), "Windows is missing something: no driver");
    let long = format!("{}{}", "a".repeat(5), "é".repeat(300));
    assert_eq!(missing_something(&long), format!("Windows is missing something: {}", "é".repeat(300)));
    assert_eq!(NOT_IN_FOLDER, "This machine's disk is not in its folder, so nothing will install over it.");
    assert_eq!(disc_missing("/d/win.iso"), "/d/win.iso is missing.");
    assert_eq!(
        [PREPARING, STARTING, WAITING_FOR_DESKTOP, CHECKING, SHUTTING_DOWN],
        [
            "Preparing the install disc",
            "Starting the installer",
            "Waiting for the desktop",
            "Checking Windows",
            "Shutting down Windows"
        ]
    );
    assert_eq!((FINISHED, ALL_THERE, STUCK_SCREEN), ("finished", "all there", "stuck-install.ppm"));
}

#[test]
fn every_ubuntu_sentence() {
    use ubuntu_guest::*;
    assert_eq!(PREPARING, "Preparing the install");
    assert_eq!(not_finished(), "The install did not finish in 2 hours. Show Screen says how far it got.");
    assert_eq!(
        not_a_server_disc("u.iso", "/casper/vmlinuz"),
        "u.iso has no /casper/vmlinuz, so it is not an Ubuntu server ISO."
    );
    assert_eq!(
        no_generic_kernel("u.iso"),
        "u.iso holds no generic kernel in its pool, so it is not an Ubuntu server ISO."
    );
}

#[test]
fn a_macos_install_s_names_steps_and_percent() {
    use macos_guest::*;
    assert_eq!((BOOT_PARTS, INSTALLERS, PART), ("boot.part", "macos-installers", "part"));
    assert_eq!(percent(Some(5), Some(10)), Some(50));
    assert_eq!(percent(Some(20), Some(10)), Some(200), "not kept to 100");
    assert_eq!(percent(Some(5), Some(0)), None);
    assert_eq!(percent(Some(5), None), None);
    assert_eq!(percent(None, Some(10)), None);
    assert_eq!(progress(DOWNLOADING, 50), "Downloading macOS from Apple: 50%");
    assert_eq!(not_ready("  qemu-img: no\n"), "The installer could not be made ready: qemu-img: no");
    assert_eq!(NO_RESULT, "reims-util ended without saying what it made.");
    assert_eq!(
        (MAKING_BOOT_IMAGE, GETTING_READY),
        ("Making the Mac's boot image", "Getting the installer ready")
    );
}
