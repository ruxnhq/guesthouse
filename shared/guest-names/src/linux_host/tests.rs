use super::file_names::{for_windows, is_one_name, not_among};
use super::scan_exclusions::{MOST, folder_problem, guest_data, list_problem, parse_found};

#[test]
fn each_name_maps_as_on_the_linux_host() {
    for (name, windows) in [
        ("plain.txt", "plain.txt"),
        ("a<b>c:d\"e/f\\g|h?i*j", "a_b_c_d_e_f_g_h_i_j"),
        ("tab\there\u{7f}", "tab_here_"),
        ("trailing space ", "trailing space_"),
        ("dots and spaces. .", "dots and spaces. _"),
        ("..", "._"),
        ("", "_"),
        ("con.tar.gz", "_con.tar.gz"),
        ("LPT0", "LPT0"),
        ("CONSOLE", "CONSOLE"),
        ("nul.", "nul_"),
    ] {
        assert_eq!(for_windows(name), windows, "{name:?}");
    }
    let taken: Vec<String> = ["Photo.JPG", "photo (2).jpg", "archive.tar.gz"].map(str::to_owned).into();
    assert_eq!(not_among("photo.jpg".to_owned(), &taken), "photo (3).jpg");
    assert_eq!(not_among("archive.tar.gz".to_owned(), &taken), "archive.tar (2).gz");
    assert_eq!(not_among(".env".to_owned(), &[".ENV".to_owned()]), ".env (2)");
    assert_eq!(not_among("new.txt".to_owned(), &taken), "new.txt");
    for (part, one) in
        [("...", true), ("a\\b", true), ("a\0b", false), ("/", false), ("..", false), ("", false)]
    {
        assert_eq!(is_one_name(part), one, "{part:?}");
    }
}

#[test]
fn each_folder_is_taken_or_refused_as_on_the_linux_host() {
    assert_eq!(folder_problem(r"D:\Games"), None);
    assert_eq!(folder_problem(r"D:\Program Files\Steam"), None);
    assert_eq!(
        folder_problem("C:/Games").unwrap(),
        "\"C:/Games\" is not a folder to leave out of scanning. It is a full path such as D:\\Games, of letters, \
         digits, spaces and . _ - ( ) only."
    );
    assert_eq!(
        folder_problem(r"C:\Users\someone").unwrap(),
        "C:\\Users\\someone holds too much of Windows, or what comes from the internet, to leave out of scanning."
    );
    let nine: Vec<String> = (1..=9).map(|n| format!(r"D:\Games{n}")).collect();
    assert_eq!(MOST, 8);
    assert_eq!(list_problem(&nine).unwrap(), "At most 8 folders can be left out of scanning.");
    assert_eq!(guest_data(&[r"D:\Games".to_owned()]), r#"{"folders":["D:\\Games"]}"#);
    assert_eq!(parse_found("folder D:\\a\tb\nfolder  \nlisted\n"), Some(vec!["D:\\ab".to_owned()]));
    assert_eq!(parse_found("folder D:\\Games\n"), None);
}
