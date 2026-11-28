// SPDX-License-Identifier: GPL-2.0-or-later
//! The built program against a QEMU played by hand: this test's end of the socket says what
//! QEMU's forwarding device says, and the guest's memory is a shared memory object made here.
//! It calls Apple's framework through the program, so it is a Mac's, and runs where the build
//! has a profile.
#![cfg(target_os = "macos")]

mod played;

use played::{COLOUR, HEIGHT, PATIENCE, WIDTH, ask};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

#[test]
fn a_guest_reaches_its_device_through_the_socket_and_shows_a_picture() {
    if !played::runnable() {
        return;
    }
    let mut guest = played::start(&[]);
    // A swap to the mapping.
    guest.swap();
    assert!(guest.qemu.interrupts.contains(&0), "the device's interrupt came on its own vector");

    // The picture, as a file on request.
    let control = guest.control.clone();
    let picture = guest.program.folder.join("frame.ppm");
    let since = Instant::now();
    let answer = loop {
        let answer = ask(&control, &format!("frame {}", picture.display()));
        if answer.starts_with("ok") || since.elapsed() > PATIENCE {
            break answer;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(answer, format!("ok {WIDTH} {HEIGHT}"));
    let file = fs::read(&picture).expect("the picture's file");
    let header = format!("P6\n{WIDTH} {HEIGHT}\n255\n");
    assert!(file.starts_with(header.as_bytes()));
    let [blue, green, red, _] = COLOUR.to_le_bytes();
    assert!(
        file[header.len()..].as_chunks::<3>().0.iter().all(|pixel| *pixel == [red, green, blue]),
        "every pixel is the guest's"
    );
    assert_eq!(file.len(), header.len() + (WIDTH * HEIGHT * 3) as usize);

    let mode = fs::metadata(&control).expect("the control socket").permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "the control socket is its user's alone: {mode:o}");
    let status = ask(&control, "status");
    assert!(status.starts_with(&format!("ok picture {WIDTH}x{HEIGHT} presented 1 ")), "{status}");
    assert!(status.ends_with("refused 1 spoiled false"), "{status}");
    // One mode is a size, as a window's: first, with the smaller modes the program started with
    // behind it. Several are the list as it is. The answer comes once the framework took it.
    let answer = ask(&control, "modes 800x600@60");
    assert!(answer.starts_with("ok 3 modes: 800x600@60,640x480@60,320x200@60 ("), "{answer}");
    let answer = ask(&control, "modes 800x600@60");
    assert_eq!(answer, "ok 3 modes: 800x600@60,640x480@60,320x200@60, offered already");
    let answer = ask(&control, "modes 1024x768@60,640x480@60");
    assert!(answer.starts_with("ok 2 modes: 1024x768@60,640x480@60 ("), "{answer}");
    assert!(ask(&control, "modes many").starts_with("error "));
    // What the Mac app expects: a bare word is read as an empty list, and with no window the
    // unknown word's answer lists the words the program knows.
    assert_eq!(ask(&control, "modes"), "error '' is no list of modes like 1920x1080@60,1280x800@60");
    assert_eq!(
        ask(&control, "bogus"),
        "error the commands are: frame PATH, modes WxH@R[,WxH@R...], window WxH, status"
    );

    // QEMU goes, and the program with it.
    drop(guest.qemu);
    let since = Instant::now();
    let status = loop {
        if let Some(status) = guest.program.child.try_wait().expect("the program's end") {
            break status;
        }
        assert!(since.elapsed() < PATIENCE, "the program outlived QEMU");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(status.success(), "{status}");
}
