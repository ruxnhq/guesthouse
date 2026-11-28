//! The rules for folders a Windows guest's Defender may leave out of its scanning, held as they are
//! today: which paths are refused and in which sentence, the list's limit, the words for what it
//! means, the folders as the guest's script is given them, and what the guest says back.

#![cfg(target_os = "linux")]

use guesthouse_app_core::scan_exclusions::{
    MOST, WHAT_IT_MEANS, folder_problem, guest_data, list_problem, parse_found, what_it_does,
};

const SCRIPT: &str = include_str!("../../../guest/windows/scan-exclusions.ps1");

fn not_a_folder(path: &str) -> String {
    format!(
        "\"{path}\" is not a folder to leave out of scanning. It is a full path such as D:\\Games, of letters, digits, spaces and . _ - ( ) only."
    )
}

fn too_much(path: &str) -> String {
    format!("{path} holds too much of Windows, or what comes from the internet, to leave out of scanning.")
}

#[test]
fn each_path_is_taken_or_refused_in_the_sentence_it_is_today() {
    let longest = format!("D:\\{}", "a".repeat(197));
    let too_long = format!("D:\\{}", "a".repeat(198));
    for fine in [
        r"D:\Games",
        r"c:\games",
        r"D:\Program Files\Steam",
        r"C:\Users\someone\Games",
        r"C:\temporary",
        r"E:\a.b\c_d\e-f\(g)",
        longest.as_str(),
    ] {
        assert_eq!(folder_problem(fine), None, "{fine}");
    }
    for odd in [
        "",
        r"C:\",
        r"1:\Games",
        "C:/Games",
        r"C:\Games\",
        r"C:\Games\.git",
        r"C:\G~1",
        r"C:\Gämes",
        too_long.as_str(),
    ] {
        assert_eq!(folder_problem(odd), Some(not_a_folder(odd)), "{odd:?}");
    }
    for never in [
        r"D:\Windows",
        r"C:\WINDOWS\Temp\x",
        r"C:\Users",
        r"C:\Users\someone",
        r"D:\Program Files",
        r"D:\ProgramData",
        r"D:\Games\Downloads",
        r"D:\Games\Startup\x",
        r"D:\tmp",
    ] {
        assert_eq!(folder_problem(never), Some(too_much(never)), "{never}");
    }
}

#[test]
fn a_list_is_eight_folders_at_most_and_its_first_problem_is_said() {
    assert_eq!(MOST, 8);
    let eight: Vec<String> = (1..=8).map(|n| format!(r"D:\Games{n}")).collect();
    assert_eq!(list_problem(&eight), None);
    let nine: Vec<String> = (1..=9).map(|n| format!(r"D:\Games{n}")).collect();
    assert_eq!(list_problem(&nine), Some("At most 8 folders can be left out of scanning.".to_owned()));
    let mixed = [r"D:\Games".to_owned(), "Games".to_owned(), r"C:\Windows".to_owned()];
    assert_eq!(list_problem(&mixed), Some(not_a_folder("Games")));
    assert_eq!(list_problem(&[]), None);
}

#[test]
fn the_words_for_what_it_means_are_as_today() {
    assert_eq!(
        WHAT_IT_MEANS,
        "Defender, Windows' own virus protection, doesn't check files in these folders as they are written or run. \
         Defender stays on for everything else. It suits a machine for games or builds you trust, where it makes \
         loading and installing faster, and not one for files or programs you don't know."
    );
    assert_eq!(
        what_it_does(&[r"D:\Games".to_owned(), r"E:\Builds".to_owned()]),
        format!(
            "{WHAT_IT_MEANS}\n- D:\\Games\n- E:\\Builds\nThe machine's settings can have Defender scan them again."
        )
    );
    assert_eq!(
        what_it_does(&[]),
        format!("{WHAT_IT_MEANS}\n\nThe machine's settings can have Defender scan them again.")
    );
}

#[test]
fn the_guest_is_given_the_folders_as_json_and_its_answer_is_read_as_today() {
    assert_eq!(
        guest_data(&[r"D:\Games".to_owned(), "E:\\\"x\"".to_owned()]),
        r#"{"folders":["D:\\Games","E:\\\"x\""]}"#
    );
    assert_eq!(guest_data(&[]), r#"{"folders":[]}"#);
    assert!(SCRIPT.contains("ConvertFrom-Json).folders)"));
    assert_eq!(parse_found(""), None);
    assert_eq!(parse_found("folder D:\\Games\nLISTED\n"), None);
    assert_eq!(parse_found("  listed  \r\n"), Some(Vec::new()));
    assert_eq!(
        parse_found(
            "  folder D:\\Games  \r\nfolder E:\\a\tb\u{7}c\nfolder    \nfolder\nFolder X:\\no\nlisted\n"
        ),
        Some(vec!["D:\\Games".to_owned(), "E:\\abc".to_owned()])
    );
    let long = format!("folder {}\nlisted\n", "x".repeat(300));
    assert_eq!(parse_found(&long), Some(vec!["x".repeat(260)]));
    let many: String = (0..40).map(|n| format!("folder D:\\{n}\n")).chain(["listed\n".to_owned()]).collect();
    let found = parse_found(&many).unwrap();
    assert_eq!(found.len(), 32);
    assert_eq!(found[31], "D:\\31");
}
