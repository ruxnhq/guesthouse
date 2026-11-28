use super::guest_says::{DESKTOP_PREFIX, PREFIX};
use super::*;

/// The Linux host's guest scripts, which write the words read here.
const USER_DATA: &str = include_str!("../../../../guest/linux/setup/seed/user-data.template");
const COMMON: &str = include_str!("../../../../guest/linux/setup/guest/common.sh");
const GPU_UP: &str = include_str!("../../../../guest/linux/setup/guest/gpu-up.sh");
const OWN_DESKTOP: &str = include_str!("../../../../guest/linux/setup/guest/own-desktop.sh");

#[test]
fn only_a_line_of_its_own_counts_as_finished() {
    assert_eq!(INSTALL_FINISHED, b"nvfwd-install-finished");
    assert!(installed(b"booting\nnvfwd-install-finished\npowering off\n"));
    assert!(installed(b"nvfwd-install-finished"));
    assert!(installed(b"nvfwd-install-finished\r\n"));
    assert!(installed(b"nvfwd-install-finished and more\n"));
    assert!(!installed(b"echo nvfwd-install-finished > /dev/ttyS0\n"));
    assert!(!installed(b" nvfwd-install-finished\n"));
    assert!(!installed(b"guesthouse-install-finished\n"));
    assert!(!installed(b"booting\rnvfwd-install-finished\n"));
    assert!(!installed(b""));
}

#[test]
fn each_two_starts_of_windows_boot_manager_are_a_step_and_the_last_step_stays() {
    assert_eq!(
        INSTALL_STEPS,
        [
            "Copying Windows to the disk",
            "Windows is setting up its devices",
            "Windows is getting ready",
            "Windows is finishing its setup",
        ]
    );
    let line = "BdsDxe: loading Boot0004 \"Windows Boot Manager\" from PciRoot(0x0)\n";
    for (starts, step) in [0, 0, 1, 1, 2, 2, 3, 3, 3, 3, 3].into_iter().enumerate() {
        assert_eq!(install_step(line.repeat(starts).as_bytes()), INSTALL_STEPS[step], "{starts} starts");
    }
    assert_eq!(install_step(b"Windows Boot Manager\nWindows Boot Manager\n"), INSTALL_STEPS[0]);
    assert_eq!(count(b"abab", b"ab"), 2);
    assert_eq!(count(b"aaa", b"aa"), 2);
    assert_eq!(count(b"", b"ab"), 0);
}

/// No line's end is held, as a checkout on Windows may give these files Windows' line endings.
#[test]
fn the_guest_scripts_write_the_words_read_here() {
    let finished = String::from_utf8_lossy(INSTALL_FINISHED);
    assert!(USER_DATA.contains(&format!("    - echo {finished} > /dev/ttyS0")));
    assert!(COMMON.contains("    [ -w \"$serial\" ] && printf '%s\\n' \"$1\" >> \"$serial\" 2>/dev/null"));
    assert!(GPU_UP.contains(&format!("tell_host \"{PREFIX}off $1\"")));
    assert!(GPU_UP.contains(&format!("tell_host \"{PREFIX}on $version\"")));
    assert!(OWN_DESKTOP.contains(&format!("tell_host \"{DESKTOP_PREFIX}gpu\"")));
    assert!(OWN_DESKTOP.contains(&format!("tell_host \"{DESKTOP_PREFIX}plain $2\"")));
}
