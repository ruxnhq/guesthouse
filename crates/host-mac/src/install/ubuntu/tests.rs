// SPDX-License-Identifier: GPL-2.0-or-later
use super::*;

const SUMS: &str = "\
c2afd538d66fdd77377d03f1ed2ac76a34f1c116baecc9a8170d68f833121f57 *ubuntu-26.04-desktop-arm64.iso
d31e9b3d01ac0fa8e488d4f8d5a7acfeee869f387674eaedf94335e174155901 *ubuntu-26.04-live-server-arm64+largemem.iso
c9aa567e6560b2eddae3af03fc686002e35b6fee96f97fd5df3271e846439fdd *ubuntu-26.04-live-server-arm64.iso
0e9f326f55dcc4244f15336ca1221e6d2f36722a4954fc544570da0a12067c3e *ubuntu-26.04-live-server-ppc64el.iso
";

#[test]
fn the_image_is_the_plain_arm64_server_disc_with_its_published_checksum() {
    let image = pick_image(SUMS).unwrap();
    assert_eq!(image.name, "ubuntu-26.04-live-server-arm64.iso");
    assert_eq!(image.sha256, "c9aa567e6560b2eddae3af03fc686002e35b6fee96f97fd5df3271e846439fdd");
    assert_eq!(
        image.url,
        "https://cdimage.ubuntu.com/ubuntu/releases/26.04/release/ubuntu-26.04-live-server-arm64.iso"
    );
}

#[test]
fn a_list_with_no_such_image_or_a_bad_checksum_is_said_not_guessed() {
    assert!(pick_image("").unwrap_err().contains("no arm64 server image"));
    assert!(pick_image("abc *ubuntu-26.04-live-server-arm64.iso").unwrap_err().contains("not a SHA-256"));
    // The text form of a line, with two spaces, reads too.
    let text = format!("{}  ubuntu-26.04-live-server-arm64.iso", "a".repeat(64));
    assert_eq!(pick_image(&text).unwrap().sha256, "a".repeat(64));
}

#[test]
fn the_answers_install_with_no_questions_and_say_how_it_ended_on_the_serial_port() {
    let text = user_data("nick", "guesthouse", "Europe/Berlin", Desktop::Weston, None);
    assert!(text.starts_with("#cloud-config\nautoinstall:"));
    assert!(text.contains("username: nick"));
    // The password comes from the account file on the disc, so no command the installer prints
    // and no log it keeps holds it.
    assert!(text.contains("chpasswd") && text.contains(ACCOUNT_FILE));
    assert_eq!(account_file("nick", "k7mn-p3xq-ab9d-w4ht"), "nick:k7mn-p3xq-ab9d-w4ht\n");
    assert!(!text.contains("passwd -d"));
    assert!(text.contains(FINISHED) && text.contains(FAILED));
    assert!(text.contains("shutdown: poweroff"));
    assert!(text.contains("size: largest"));
    assert!(text.contains("timezone: 'Europe/Berlin'"));
}

#[test]
fn the_keyboard_is_the_mac_s_and_us_when_it_is_not_known() {
    use crate::keyboard_layout::Xkb;
    let german = Some(Xkb { layout: "de", variant: "" });
    let swiss = Some(Xkb { layout: "ch", variant: "fr" });
    let text = |keyboard| user_data("nick", "guesthouse", "Europe/Berlin", Desktop::Weston, keyboard);
    assert!(text(german).contains("  keyboard:\n    layout: de\n  storage:"));
    assert!(text(swiss).contains("  keyboard:\n    layout: ch\n    variant: fr\n  storage:"));
    assert!(text(None).contains("  keyboard:\n    layout: us\n  storage:"));
}

#[test]
fn the_time_zone_is_the_mac_s_as_its_link_names_it_and_utc_otherwise() {
    use std::path::Path;
    let zone = |link: &str| time_zone_of(Some(Path::new(link)));
    assert_eq!(zone("/var/db/timezone/zoneinfo/Europe/Berlin"), "Europe/Berlin");
    assert_eq!(zone("/usr/share/zoneinfo/America/Argentina/Buenos_Aires"), "America/Argentina/Buenos_Aires");
    assert_eq!(zone("/var/db/timezone/zoneinfo/Etc/GMT+1"), "Etc/GMT+1");
    assert_eq!(zone("/var/db/timezone/zoneinfo/../../etc/passwd"), "Etc/UTC");
    assert_eq!(zone("/var/db/timezone/zoneinfo/Europe/Ber'lin"), "Etc/UTC");
    assert_eq!(zone("/etc/somewhere"), "Etc/UTC");
    assert_eq!(time_zone_of(None), "Etc/UTC");
    // This Mac's own reads as a name.
    assert!(!mac_time_zone().is_empty());
}

#[test]
fn the_seed_names_the_instance() {
    assert_eq!(meta_data("ubuntu"), "instance-id: guesthouse-ubuntu\nlocal-hostname: ubuntu\n");
}

#[test]
fn the_driver_step_says_why_the_download_failed() {
    let said = driver_download_failed("https://files.ruxn.com/x could not be reached: timed out.");
    assert!(
        said.contains("could not be downloaded: https://files.ruxn.com/x could not be reached: timed out.")
    );
    assert!(said.contains("open Ubuntu without it"));
    assert!(!said.contains("not packaged"));
}

#[test]
fn the_installer_runs_with_no_questions_and_its_console_on_the_screen() {
    assert!(INSTALLER_COMMAND_LINE.starts_with("autoinstall ds=nocloud "));
    let (installer, installed) = INSTALLER_COMMAND_LINE.split_once(" --- ").unwrap();
    assert!(!installer.contains("ttyAMA0"));
    assert!(installed.contains("console=ttyAMA0,115200"));
}

#[test]
fn the_answers_put_the_drivers_service_into_the_installed_system_before_saying_finished() {
    let text = user_data("nick", "guesthouse", "Europe/Berlin", Desktop::Weston, None);
    let enable = text.find(&format!("systemctl enable {DRIVER_SERVICE_FILE}")).unwrap();
    assert!(text.contains("mount -o ro LABEL=CIDATA"));
    assert!(text.contains("/target/usr/local/sbin/guesthouse-driver"));
    assert!(enable < text.find(&format!("echo {FINISHED}")).unwrap());
}

#[test]
fn the_drivers_service_runs_install_sh_from_the_ruxn_disk_once_a_version() {
    assert!(DRIVER_SERVICE.contains("ExecStart=/bin/sh /usr/local/sbin/guesthouse-driver"));
    assert!(DRIVER_SERVICE.contains("WantedBy=multi-user.target"));
    assert!(DRIVER_RUNNER.contains("/dev/disk/by-label/RUXN"));
    assert!(DRIVER_RUNNER.contains("sh ./install.sh"));
    assert!(DRIVER_RUNNER.contains(crate::install::driver::LINUX.manifest));
    assert!(DRIVER_RUNNER.contains("driver-version"));
    // Worked: into the desktop; failed: off, so Try Again is a start of its own.
    assert!(DRIVER_RUNNER.contains("systemctl --no-block reboot"));
    assert!(DRIVER_RUNNER.contains("systemctl --no-block poweroff"));
    assert!(DRIVER_RUNNER.contains(&format!("{} exit-", "driver failed")));
}

#[test]
fn the_drivers_service_reads_the_version_from_the_manifest_the_package_carries() {
    use std::io::Write;
    // The manifest as the package's scripts write it (drivers/linux-pvg/package/lib.sh), given to
    // the service's own line on standard input in place of the disk's copy.
    let manifest = "{\"version\": \"0.1.7\", \"files\": [\n  {\"name\": \"guesthouse-linux-driver-0.1.7-install.sh\", \
                    \"as\": \"install.sh\", \"sha256\": \"00\"}\n]}\n";
    let read = DRIVER_RUNNER.lines().find(|l| l.starts_with("version=$(sed")).unwrap();
    let script = format!("mnt=/dev\nmanifest=stdin\n{read}\nprintf %s \"$version\"");
    let mut sh = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(script)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    sh.stdin.take().unwrap().write_all(manifest.as_bytes()).unwrap();
    let out = sh.wait_with_output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "0.1.7");
}

fn line(sum: char, name: &str) -> String {
    format!("{} *{name}\n", sum.to_string().repeat(64))
}

#[test]
fn the_newest_point_release_in_the_list_is_the_one_taken() {
    // As the folder lists it: the release and its point releases together, other images and
    // architectures among them, the first point release's lines after the release's own.
    let mut sums = String::new();
    sums += &line('a', "ubuntu-26.04-live-server-arm64.iso");
    sums += &line('b', "ubuntu-26.04-live-server-arm64+largemem.iso");
    sums += &line('c', "ubuntu-26.04.1-live-server-arm64+largemem.iso");
    sums += &line('d', "ubuntu-26.04.1-live-server-arm64.iso");
    sums += &line('e', "ubuntu-26.04.1-live-server-s390x.iso");
    sums += &line('f', "ubuntu-26.04.1-desktop-arm64.iso");
    let image = pick_image(&sums).unwrap();
    assert_eq!(image.name, "ubuntu-26.04.1-live-server-arm64.iso");
    assert_eq!(image.sha256, "d".repeat(64));
    assert!(image.url.ends_with("/release/ubuntu-26.04.1-live-server-arm64.iso"));
}

#[test]
fn a_point_release_after_that_is_taken_too_and_the_order_in_the_list_does_not_matter() {
    let mut sums = String::new();
    sums += &line('2', "ubuntu-26.04.2-live-server-arm64.iso");
    sums += &line('1', "ubuntu-26.04.1-live-server-arm64.iso");
    sums += &line('0', "ubuntu-26.04-live-server-arm64.iso");
    sums += &line('9', "ubuntu-26.04.10-live-server-arm64.iso");
    assert_eq!(
        pick_image(&sums).unwrap().name,
        "ubuntu-26.04.10-live-server-arm64.iso",
        "by number, not by text"
    );
    let without_ten: String =
        sums.lines().filter(|l| !l.contains(".10-")).map(|l| format!("{l}\n")).collect();
    assert_eq!(pick_image(&without_ten).unwrap().name, "ubuntu-26.04.2-live-server-arm64.iso");
}

#[test]
fn a_list_of_only_the_release_with_no_point_release_still_gives_the_release() {
    assert_eq!(
        pick_image(&line('a', "ubuntu-26.04-live-server-arm64.iso")).unwrap().name,
        "ubuntu-26.04-live-server-arm64.iso"
    );
}

#[test]
fn names_that_only_look_alike_are_left_out() {
    for name in [
        "ubuntu-26.04.-live-server-arm64.iso",
        "ubuntu-26.04.x-live-server-arm64.iso",
        "ubuntu-26.04.1.2-live-server-arm64.iso",
        "ubuntu-26.045-live-server-arm64.iso",
        "ubuntu-26.04.1-live-server-arm64.iso.zsync",
    ] {
        assert!(pick_image(&line('a', name)).is_err(), "{name}");
    }
}

#[test]
fn the_answers_name_gnome_or_plasma_for_the_driver_package_and_nothing_for_weston() {
    let weston = user_data("nick", "guesthouse", "Europe/Berlin", Desktop::Weston, None);
    assert!(!weston.contains("/etc/guesthouse/desktop"));
    for (desktop, word) in [(Desktop::Gnome, "gnome"), (Desktop::Plasma, "plasma")] {
        let text = user_data("nick", "guesthouse", "Europe/Berlin", desktop, None);
        let line = text.find(&format!("echo {word} > /target/etc/guesthouse/desktop")).unwrap();
        assert!(line < text.find(&format!("echo {FINISHED}")).unwrap());
        assert_eq!(Desktop::parse(word), Some(desktop));
    }
    assert_eq!(Desktop::parse("kde"), None);
}
