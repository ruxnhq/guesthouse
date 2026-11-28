use super::names::{for_windows, is_one_name, not_among};
use super::*;
use crate::testing::{QemuStandIn, Scratch};
use std::fs;
use std::io::BufRead;
use std::net::{Ipv4Addr, TcpListener};
use std::os::unix::fs::symlink;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, channel};
use std::thread;

const TOKEN: &str = "secret";

/// A helper that gives each connection the next answer, and passes on what it was sent.
fn fake_helper(answers: &[&[u8]]) -> (Helper, Receiver<Vec<u8>>, QemuStandIn) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let answers: Vec<Vec<u8>> = answers.iter().map(|answer| answer.to_vec()).collect();
    let (sent, requests) = channel();
    thread::spawn(move || {
        for answer in answers {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = Vec::new();
            reader.read_until(b'\n', &mut request).unwrap();
            // A put's last word is how many bytes follow.
            let line = String::from_utf8_lossy(&request).into_owned();
            let words: Vec<&str> = line.split_whitespace().collect();
            if words.get(1) == Some(&"put") {
                let mut body = vec![0; words[4].parse().unwrap()];
                reader.read_exact(&mut body).unwrap();
                request.extend(body);
            }
            sent.send(request).unwrap();
            stream.write_all(&answer).unwrap();
        }
    });
    let qemu = QemuStandIn::forwarding(crate::network::GUEST_FILES_PORT, port);
    (Helper::new(qemu.end(), TOKEN), requests, qemu)
}

fn text(request: &[u8]) -> String {
    String::from_utf8_lossy(request).into_owned()
}

#[test]
fn names_windows_refuses_are_changed_and_others_are_not() {
    assert_eq!(for_windows("report 2026.pdf"), "report 2026.pdf");
    assert_eq!(for_windows("a:b?c"), "a_b_c");
    assert_eq!(for_windows("ends in a dot."), "ends in a dot_");
    assert_eq!(for_windows("nul.txt"), "_nul.txt");
    assert_eq!(for_windows("COM1"), "_COM1");
    assert_eq!(for_windows("console.log"), "console.log");
}

#[test]
fn a_second_file_of_the_same_name_gets_a_number() {
    let taken = vec!["Notes.txt".to_owned(), "notes (2).txt".to_owned()];
    assert_eq!(not_among("notes.txt".to_owned(), &taken), "notes (3).txt");
    assert_eq!(not_among("other".to_owned(), &taken), "other");
    assert_eq!(not_among(".bashrc".to_owned(), &[".bashrc".to_owned()]), ".bashrc (2)");
}

#[test]
fn a_guests_name_is_one_name_or_nothing() {
    assert!(is_one_name("photo.jpg"));
    assert!(!is_one_name(".."));
    assert!(!is_one_name(""));
    assert!(!is_one_name("a/b"));
}

#[test]
fn a_dropped_folder_comes_before_what_is_in_it_and_links_are_left_out() {
    let scratch = Scratch::new();
    fs::create_dir_all(scratch.path("trip/day 1")).unwrap();
    fs::write(scratch.path("trip/day 1/a:b.txt"), "x").unwrap();
    fs::write(scratch.path("lone.txt"), "y").unwrap();
    symlink("/etc/passwd", scratch.path("trip/link")).unwrap();
    symlink("/etc", scratch.path("way out")).unwrap();
    let dropped = [scratch.path("trip"), scratch.path("lone.txt"), scratch.path("way out")];
    let found: Vec<(String, bool)> =
        dropped_files(&dropped).into_iter().map(|file| (file.name, file.is_folder)).collect();
    let expected = [("trip", true), ("trip/day 1", true), ("trip/day 1/a_b.txt", false), ("lone.txt", false)];
    assert_eq!(found, expected.map(|(name, is_folder)| (name.to_owned(), is_folder)));
}

#[test]
fn two_dropped_files_of_one_name_both_arrive() {
    let scratch = Scratch::new();
    for folder in ["one", "two"] {
        fs::create_dir_all(scratch.path(folder)).unwrap();
        fs::write(scratch.path(folder).join("same.txt"), folder).unwrap();
    }
    let dropped = [scratch.path("one/same.txt"), scratch.path("two/same.txt")];
    let names: Vec<String> = dropped_files(&dropped).into_iter().map(|file| file.name).collect();
    assert_eq!(names, ["same.txt", "same (2).txt"]);
}

#[test]
fn sending_makes_the_folders_and_puts_the_files_with_the_token_first() {
    let scratch = Scratch::new();
    fs::create_dir_all(scratch.path("trip")).unwrap();
    fs::write(scratch.path("trip/a.txt"), "hello").unwrap();
    let (helper, requests, _qemu) = fake_helper(&[b"ok\n", b"ok\n"]);
    let files = dropped_files(&[scratch.path("trip")]);
    send(&helper, "d1", &files, &AtomicBool::new(false)).unwrap();
    assert_eq!(text(&requests.recv().unwrap()), "secret folder d1 dHJpcA==\n");
    assert_eq!(text(&requests.recv().unwrap()), "secret put d1 dHJpcC9hLnR4dA== 5\nhello");
}

#[test]
fn sending_stops_when_it_is_told_to_and_when_the_guest_refuses() {
    let scratch = Scratch::new();
    fs::write(scratch.path("a.txt"), "hello").unwrap();
    let files = dropped_files(&[scratch.path("a.txt")]);
    let (helper, _requests, _qemu) = fake_helper(&[b"failed There is not enough space on the disk.\n"]);
    let stopped = send(&helper, "d1", &files, &AtomicBool::new(true)).unwrap_err();
    assert_eq!(stopped.kind(), ErrorKind::Interrupted);
    let refused = send(&helper, "d1", &files, &AtomicBool::new(false)).unwrap_err();
    assert!(refused.to_string().contains("not enough space"));
}

#[test]
fn the_drag_says_whether_a_press_can_reach_it_and_how_it_ended() {
    let (helper, requests, _qemu) =
        fake_helper(&[b"shown\n", b"covered\n", b"desktop\n", b"", b"files 0\n", b"files 1\n"]);
    assert!(helper.drag("d1").unwrap());
    assert!(!helper.drag("d1").unwrap());
    assert_eq!(helper.wait(Until::Ended, Duration::from_millis(250)).unwrap(), DragState::OnDesktop);
    assert_eq!(text(&requests.iter().nth(2).unwrap()), "secret wait ended 250\n");
    // A helper that isn't there closes without a word, and an older app's says so.
    assert!(!helper.answers());
    assert!(!helper.answers());
    assert!(helper.answers());
}

#[test]
fn a_caught_drag_lists_its_files_and_they_land_under_the_folder() {
    let listing = b"files\nfolder 0 dHJpcA==\nfile 5 dHJpcC9hLnR4dA==\nfile 2 Yi50eHQ=\n";
    let (helper, requests, _qemu) = fake_helper(&[listing, b"ok 5\nhello", b"ok 2\nhi"]);
    let entries = helper.catch(Duration::from_millis(100)).unwrap().unwrap();
    assert_eq!(entries[1], Entry { name: "trip/a.txt".to_owned(), is_folder: false, size: 5 });
    let scratch = Scratch::new();
    let dragged = fetch(&helper, &entries, &scratch.path("out"), &AtomicBool::new(false)).unwrap();
    assert_eq!(dragged, [scratch.path("out/trip"), scratch.path("out/b.txt")]);
    assert_eq!(fs::read_to_string(scratch.path("out/trip/a.txt")).unwrap(), "hello");
    assert_eq!(fs::read_to_string(scratch.path("out/b.txt")).unwrap(), "hi");
    assert_eq!(text(&requests.iter().nth(1).unwrap()), "secret get dHJpcC9hLnR4dA==\n");
}

#[test]
fn no_drag_in_the_guest_is_nothing_caught() {
    let (helper, _requests, _qemu) = fake_helper(&[b"none\n"]);
    assert_eq!(helper.catch(Duration::from_millis(100)).unwrap(), None);
}

#[test]
fn a_guests_name_that_leads_out_of_the_folder_is_refused() {
    let (helper, _requests, _qemu) = fake_helper(&[]);
    let scratch = Scratch::new();
    for name in ["../out.txt", "/etc/passwd", "a//b"] {
        let entries = [Entry { name: name.to_owned(), is_folder: false, size: 1 }];
        let refused = fetch(&helper, &entries, &scratch.path("in"), &AtomicBool::new(false)).unwrap_err();
        assert_eq!(refused.kind(), ErrorKind::InvalidData);
    }
    assert!(!scratch.path("out.txt").exists());
}

#[test]
fn a_file_that_ends_early_is_an_error() {
    let (helper, _requests, _qemu) = fake_helper(&[b"ok 9\nshort"]);
    let got = helper.get("a.txt", &mut Vec::new()).unwrap_err();
    assert_eq!(got.kind(), ErrorKind::UnexpectedEof);
}

/// The helper says which version it is, and the app starts its own when it's another.
#[test]
fn the_helper_in_the_repo_is_the_version_the_app_expects() {
    let helper = concat!(env!("CARGO_MANIFEST_DIR"), "/../../guest/windows/files.ps1");
    let helper = fs::read_to_string(helper).unwrap();
    assert!(helper.contains(&format!("const string Version = \"{VERSION}\";")));
    let port = format!("const int Port = {};", crate::network::GUEST_FILES_PORT);
    assert!(helper.contains(&port));
}

/// Each name as Windows gets it today, and each copy's number, before the rules move.
#[test]
fn each_name_maps_as_today() {
    for (name, windows) in [
        ("plain.txt", "plain.txt"),
        ("a<b>c:d\"e/f\\g|h?i*j", "a_b_c_d_e_f_g_h_i_j"),
        ("tab\there\u{7f}", "tab_here_"),
        ("new\nline", "new_line"),
        ("caf\u{e9} \u{1f600}.txt", "caf\u{e9} \u{1f600}.txt"),
        ("trailing space ", "trailing space_"),
        ("dots and spaces. .", "dots and spaces. _"),
        ("..", "._"),
        (".", "_"),
        ("", "_"),
        ("CON", "_CON"),
        ("con.tar.gz", "_con.tar.gz"),
        ("Lpt9.txt", "_Lpt9.txt"),
        ("LPT0", "LPT0"),
        ("COM10", "COM10"),
        ("CONSOLE", "CONSOLE"),
        ("aux ", "aux_"),
        ("nul.", "nul_"),
        (".nul", ".nul"),
    ] {
        assert_eq!(for_windows(name), windows, "{name:?}");
    }
    let taken: Vec<String> =
        ["Photo.JPG", "photo (2).jpg", "archive.tar.gz", "README"].map(str::to_owned).into();
    for (name, kept) in [
        ("photo.jpg", "photo (3).jpg"),
        ("archive.tar.gz", "archive.tar (2).gz"),
        ("readme", "readme (2)"),
        ("new.txt", "new.txt"),
    ] {
        assert_eq!(not_among(name.to_owned(), &taken), kept, "{name}");
    }
    assert_eq!(not_among("x".to_owned(), &["X".to_owned(), "x (2)".to_owned(), "X (3)".to_owned()]), "x (4)");
    assert_eq!(not_among(".env".to_owned(), &[".ENV".to_owned()]), ".env (2)");
    for (part, one) in [
        ("photo.jpg", true),
        ("...", true),
        (" ", true),
        ("a\\b", true),
        ("CON", true),
        ("a\0b", false),
        ("/", false),
        (".", false),
        ("..", false),
        ("", false),
    ] {
        assert_eq!(is_one_name(part), one, "{part:?}");
    }
}

#[test]
fn an_answer_with_no_end_is_refused_at_the_limit() {
    let endless = std::io::repeat(b'a').take(LONGEST_LINE as u64 * 4);
    assert_eq!(Helper::answer(endless).unwrap_err().kind(), ErrorKind::InvalidData);
}

#[test]
fn a_drag_with_a_line_past_the_limit_or_too_many_files_is_refused() {
    let mut long = b"files\nfile 1 ".to_vec();
    long.extend(std::iter::repeat_n(b'a', LONGEST_LINE * 2));
    let (helper, _requests, _qemu) = fake_helper(&[&long]);
    assert_eq!(helper.catch(Duration::from_millis(100)).unwrap_err().kind(), ErrorKind::InvalidData);

    let mut many = b"files\n".to_vec();
    for _ in 0..=MOST_ENTRIES {
        many.extend_from_slice(b"file 1 Yi50eHQ=\n");
    }
    let (helper, _requests, _qemu) = fake_helper(&[&many]);
    assert_eq!(helper.catch(Duration::from_millis(100)).unwrap_err().kind(), ErrorKind::InvalidData);
}

#[test]
fn a_length_line_with_no_end_is_refused() {
    let mut long = b"ok ".to_vec();
    long.extend(std::iter::repeat_n(b'1', LONGEST_LINE * 2));
    let (helper, _requests, _qemu) = fake_helper(&[&long]);
    let error = helper.get("a.txt", &mut Vec::new()).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidData);
}
