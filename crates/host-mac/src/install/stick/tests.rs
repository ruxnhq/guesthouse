// SPDX-License-Identifier: GPL-2.0-or-later
use super::*;

fn folder(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("gh-stick-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn the_drivers_stick_is_64_mb_unless_its_files_need_more() {
    let folder = folder("size");
    let small = folder.join("small");
    fs::write(&small, vec![0u8; 1000]).unwrap();
    assert_eq!(megabytes(std::slice::from_ref(&small)), 64);
    let big = folder.join("big");
    File::create(&big).unwrap().set_len(100_000_000).unwrap();
    assert_eq!(megabytes(&[small, big]), 126);
    let _ = fs::remove_dir_all(folder);
}

/// A real stick: made, mounted again, and read back, with its label and file system as the guest
/// finds them.
#[test]
fn a_real_stick_holds_the_files_under_the_label_the_guest_looks_for() {
    let folder = folder("real");
    let (first, second) = (folder.join("install.ps1"), folder.join("driver.sys"));
    fs::write(&first, "write-host hello\n").unwrap();
    fs::write(&second, vec![7u8; 300_000]).unwrap();
    let image = folder.join("stick.img");
    make(&image, &[first, second.clone()]).unwrap();
    assert_eq!(fs::metadata(&image).unwrap().len(), 64 * 1_048_576);

    let mount = folder.join("mount");
    fs::create_dir_all(&mount).unwrap();
    run(
        Command::new(HDIUTIL)
            .args([
                "attach",
                "-quiet",
                "-nobrowse",
                "-readonly",
                "-imagekey",
                "diskimage-class=CRawDiskImage",
            ])
            .arg("-mountpoint")
            .arg(&mount)
            .arg(&image),
        "mount it to read it",
    )
    .unwrap();
    let said = run(Command::new("/usr/sbin/diskutil").arg("info").arg(&mount), "ask for its information");
    let first_back = fs::read_to_string(mount.join("install.ps1"));
    let second_back = fs::read(mount.join("driver.sys"));
    detach(&mount.to_string_lossy()).unwrap();

    let said = said.unwrap();
    assert!(said.contains("Volume Name:") && said.contains(LABEL), "{said}");
    assert!(said.contains("FAT16"), "{said}");
    assert_eq!(first_back.unwrap(), "write-host hello\n");
    assert_eq!(second_back.unwrap(), fs::read(second).unwrap());
    let _ = fs::remove_dir_all(folder);
}

#[test]
fn a_stick_that_cannot_be_made_leaves_no_image_and_says_why() {
    let folder = folder("fails");
    let image = folder.join("stick.img");
    let missing = folder.join("not-there");
    let said = make(&image, &[missing]).unwrap_err();
    assert!(said.contains("not-there"), "{said}");
    assert!(!image.exists());
    let _ = fs::remove_dir_all(folder);
}
