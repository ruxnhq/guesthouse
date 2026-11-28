use super::*;
use crate::testing::{Scratch, scratch_context};

#[test]
fn a_folder_of_ones_own_can_be_left_out() {
    for fine in [
        r"D:\Games",
        r"C:\Program Files (x86)\Steam\steamapps",
        r"C:\Users\gpupv\source\build-cache",
        r"C:\SteamLibrary",
    ] {
        assert_eq!(folder_problem(fine), None, "{fine}");
    }
}

#[test]
fn what_would_leave_most_of_windows_unchecked_is_refused() {
    for never in [
        r"C:\Windows",
        r"C:\windows\System32",
        r"C:\Users",
        r"C:\Users\gpupv",
        r"C:\Users\gpupv\Downloads",
        r"D:\Games\Temp",
        r"C:\Program Files",
        r"C:\Program Files (x86)",
        r"C:\ProgramData",
        r"C:\Users\gpupv\AppData\Local\Steam",
    ] {
        assert!(folder_problem(never).unwrap().contains("too much"), "{never}");
    }
}

#[test]
fn only_a_plain_full_path_is_a_folder() {
    for odd in [
        "",
        r"C:\",
        "Games",
        r"\\server\share\games",
        r"C:Games",
        r"C:\Games\*",
        r"C:\Games\..\Windows",
        r"C:\Windows.",
        r"C:\Users.\gpupv",
        r"C:\Windows \System32",
        r"C:\.hidden",
        r"C:\Games\",
        r"C:\%TEMP%",
        "C:\\Games\\a\u{202e}b",
        r"C:\Games;D:\",
        r"C:\Games' ; Remove-Item C:\ #",
        "C:\\Games\nD:\\",
    ] {
        assert!(folder_problem(odd).unwrap().contains("not a folder"), "{odd:?}");
    }
    let many: Vec<String> = (0..=MOST).map(|n| format!(r"D:\Games{n}")).collect();
    assert!(list_problem(&many).unwrap().contains("At most"));
    assert_eq!(list_problem(&many[..MOST]), None);
}

#[test]
fn the_machine_keeps_its_folders_and_none_is_no_file() {
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let vm = Machine::new("box", "Box", "/d", "");
    fs::create_dir_all(vm.folder(&context)).unwrap();
    assert!(read_exclusions(&context, &vm).is_empty());
    let folders = vec![r"D:\Games".to_owned()];
    write_exclusions(&context, &vm, &folders).unwrap();
    assert_eq!(read_exclusions(&context, &vm), folders);
    assert_eq!(
        state_text(&context, &vm).unwrap(),
        "Defender was to leave out D:\\Games, but didn't take them, so every folder is scanned."
    );
    mark_applied(&context, &vm).unwrap();
    assert_eq!(read_exclusions(&context, &vm), folders);
    assert_eq!(
        state_text(&context, &vm).unwrap(),
        "Defender doesn't scan D:\\Games. Everything else is scanned."
    );
    write_exclusions(&context, &vm, &[]).unwrap();
    assert!(!vm.folder(&context).join(EXCLUSIONS_FILE).exists());
    assert_eq!(state_text(&context, &vm), None);
    assert!(mark_applied(&context, &vm).is_err());
}

#[test]
fn the_words_say_what_it_means_and_name_every_folder() {
    let text = what_it_does(&[r"D:\Games".to_owned(), r"E:\Builds".to_owned()]);
    assert!(text.starts_with(WHAT_IT_MEANS));
    assert!(text.contains("- D:\\Games\n- E:\\Builds"));
    assert!(WHAT_IT_MEANS.contains("not one for files or programs you don't know"));
}

#[test]
fn what_the_guest_says_is_kept_as_plain_text_and_ends_the_look() {
    assert_eq!(parse_found("folder C:\\Games\n"), None, "cut short");
    assert_eq!(parse_found("listed\n"), Some(vec![]));
    let found = parse_found("folder C:\\Games\nfolder D:\\a\u{1b}[2Jb\nfolder  \nlisted\n").unwrap();
    assert_eq!(found, ["C:\\Games", "D:\\a[2Jb"]);
    let scratch = Scratch::new();
    let context = scratch_context(&scratch);
    let vm = Machine { throwaway: true, ..Machine::new("box", "Box", "/d", "") };
    fs::create_dir_all(crate::throwaway::clean_folder(&context, &vm)).unwrap();
    mark_for_look(&vm.folder(&context)).unwrap();
    assert!(needs_look(&context, &vm));
    record_found(&context, &vm, &found).unwrap();
    assert!(!needs_look(&context, &vm));
    let text = state_text(&context, &vm).unwrap();
    assert_eq!(
        text,
        "Defender in this guest doesn't scan C:\\Games, D:\\a[2Jb, as the guest said at a start."
    );
    let clean = crate::throwaway::clean_folder(&context, &vm).join(EXCLUSIONS_FILE);
    assert!(clean.is_file(), "a throwaway's clean state is the disk that was asked");
    record_found(&context, &vm, &[]).unwrap();
    assert_eq!(state_text(&context, &vm), None);
    assert!(!clean.exists());
}
