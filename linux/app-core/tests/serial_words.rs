//! What a guest of the Linux host says on its serial port, and how the host reads it, held as it is
//! today: the words the guest's scripts write, the steps of a Windows install read off the firmware's
//! lines, and what the guest says about its GPU and its desktop. Installed guests keep saying these
//! words, so neither side may drift from the other.

#![cfg(target_os = "linux")]

use guesthouse_app_core::actions::{INSTALL_STEPS, install_step};
use guesthouse_app_core::linux::guest_says::{GuestDesktop, GuestGpu, GuestSays, newest};

const USER_DATA: &str = include_str!("../../../guest/linux/setup/seed/user-data.template");
const COMMON: &str = include_str!("../../../guest/linux/setup/guest/common.sh");
const GPU_UP: &str = include_str!("../../../guest/linux/setup/guest/gpu-up.sh");
const OWN_DESKTOP: &str = include_str!("../../../guest/linux/setup/guest/own-desktop.sh");

#[test]
fn the_install_steps_are_these_four_lines() {
    assert_eq!(
        INSTALL_STEPS,
        [
            "Copying Windows to the disk",
            "Windows is setting up its devices",
            "Windows is getting ready",
            "Windows is finishing its setup",
        ]
    );
}

#[test]
fn each_two_starts_of_windows_boot_manager_are_a_step_and_the_last_step_stays() {
    let line = "BdsDxe: loading Boot0004 \"Windows Boot Manager\" from PciRoot(0x0)\n";
    let expected = [0, 0, 1, 1, 2, 2, 3, 3, 3, 3, 3];
    for (starts, step) in expected.into_iter().enumerate() {
        assert_eq!(install_step(line.repeat(starts).as_bytes()), INSTALL_STEPS[step], "{starts} starts");
    }
    assert_eq!(install_step(&line.repeat(40).into_bytes()), INSTALL_STEPS[3]);
}

#[test]
fn the_boot_managers_name_counts_wherever_it_is_and_only_with_its_quotes() {
    assert_eq!(install_step(b"\"Windows Boot Manager\"\"Windows Boot Manager\""), INSTALL_STEPS[1]);
    assert_eq!(install_step(b"Windows Boot Manager\nWindows Boot Manager\n"), INSTALL_STEPS[0]);
    assert_eq!(install_step(b"\"windows boot manager\"\n\"windows boot manager\"\n"), INSTALL_STEPS[0]);
    assert_eq!(install_step(b""), INSTALL_STEPS[0]);
}

#[test]
fn the_ubuntu_install_says_it_finished_on_a_line_of_its_own() {
    assert!(USER_DATA.contains("\n    - echo nvfwd-install-finished > /dev/ttyS0\n"));
}

#[test]
fn the_guest_scripts_write_the_gpu_and_desktop_lines_the_host_reads() {
    assert!(COMMON.contains(
        "tell_host() {\n    [ -w \"$serial\" ] && printf '%s\\n' \"$1\" >> \"$serial\" 2>/dev/null\n"
    ));
    assert!(COMMON.contains("serial=$root/dev/ttyS0\n"));
    assert!(GPU_UP.contains("tell_host \"nvfwd-gpu off $1\"\n"));
    assert!(GPU_UP.contains("tell_host \"nvfwd-gpu on $version\"\n"));
    assert!(OWN_DESKTOP.contains("tell_host \"nvfwd-desktop gpu\"\n"));
    assert!(OWN_DESKTOP.contains("tell_host \"nvfwd-desktop plain $2\"\n"));
}

fn on(version: &str) -> GuestGpu {
    GuestGpu::On { version: version.into() }
}

fn off(reason: &str) -> GuestGpu {
    GuestGpu::Off { reason: reason.into() }
}

#[test]
fn each_gpu_line_is_read_as_today() {
    let cases: [(&[u8], Option<GuestGpu>); 14] = [
        (b"nvfwd-gpu on 615.78.08\n", Some(on("615.78.08"))),
        (b"nvfwd-gpu on 615.78.08\r\n", Some(on("615.78.08"))),
        (b"nvfwd-gpu on 615.78.08", Some(on("615.78.08"))),
        (b"nvfwd-gpu on  615.78.08\n", Some(on("615.78.08"))),
        (b"nvfwd-gpu on 615.78.08 \n", Some(on("615.78.08"))),
        (b"nvfwd-gpu off no nvfwd device.\n", Some(off("no nvfwd device."))),
        (b"nvfwd-gpu off   spaced  out  \n", Some(off("spaced  out"))),
        (b"nvfwd-gpu off caf\xc3\xa9 \xe2\x80\x94 x\n", Some(off("caf  x"))),
        (b"nvfwd-gpu off\n", None),
        (b"nvfwd-gpu off \n", None),
        (b"nvfwd-gpu On 615.78.08\n", None),
        (b" nvfwd-gpu on 615.78.08\n", None),
        (b"nvfwd-gpu\ton 615.78.08\n", None),
        (b"guesthouse: nvfwd-gpu on 615.78.08\n", None),
    ];
    for (serial, gpu) in cases {
        assert_eq!(newest(serial).map(|says| says.gpu), gpu, "{}", String::from_utf8_lossy(serial));
    }
}

#[test]
fn each_desktop_line_is_read_as_today() {
    let cases: [(&str, Option<GuestDesktop>); 7] = [
        ("nvfwd-desktop gpu", Some(GuestDesktop::Gpu)),
        ("nvfwd-desktop gpu\r", Some(GuestDesktop::Gpu)),
        ("nvfwd-desktop gpu ", Some(GuestDesktop::Gpu)),
        (
            "nvfwd-desktop plain the card <never> lit",
            Some(GuestDesktop::Plain { reason: "the card never lit".into() }),
        ),
        ("nvfwd-desktop plain ", None),
        ("nvfwd-desktop GPU", None),
        ("nvfwd-desktop  gpu", None),
    ];
    for (line, desktop) in cases {
        let serial = format!("nvfwd-gpu on 615.78.08\n{line}\n");
        assert_eq!(newest(serial.as_bytes()).unwrap().desktop, desktop, "{line:?}");
    }
}

#[test]
fn each_sentence_for_the_machines_row_is_as_today() {
    let says = |gpu, desktop| GuestSays { gpu, desktop };
    let plain = || Some(GuestDesktop::Plain { reason: "it never lit".into() });
    let cases = [
        (says(on("615.78.08"), None), "The guest has its GPU, on driver 615.78.08.", false),
        (
            says(on("615.78.08"), Some(GuestDesktop::Gpu)),
            "The guest has its GPU, on driver 615.78.08, and its desktop is on it.",
            false,
        ),
        (
            says(on("615.78.08"), plain()),
            "The guest has its GPU, on driver 615.78.08. Its desktop is on the plain screen: it never lit",
            true,
        ),
        (says(off("the build failed"), None), "The guest has no GPU on this boot: the build failed", true),
        (
            says(off("the build failed"), Some(GuestDesktop::Gpu)),
            "The guest has no GPU on this boot: the build failed",
            true,
        ),
        (says(off("the build failed"), plain()), "The guest has no GPU on this boot: the build failed", true),
    ];
    for (said, text, plain_desktop) in cases {
        assert_eq!(said.text(), text);
        assert_eq!(said.plain_desktop(), plain_desktop, "{text}");
    }
    assert_eq!(on("615.78.08").text(), "The guest has its GPU, on driver 615.78.08.");
    assert_eq!(off("x").text(), "The guest has no GPU on this boot: x");
}

#[test]
fn a_reason_keeps_240_typed_characters_at_most_and_invalid_bytes_are_dropped() {
    let mut serial = b"nvfwd-gpu off ".to_vec();
    serial.extend(b"\xff\xfe");
    serial.extend(std::iter::repeat_n(b'y', 300));
    let Some(GuestGpu::Off { reason }) = newest(&serial).map(|says| says.gpu) else { panic!("read") };
    assert_eq!(reason, "y".repeat(240));
}
