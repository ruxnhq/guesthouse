// SPDX-License-Identifier: GPL-2.0-or-later
use super::*;
use std::fs;

#[test]
fn the_account_is_the_macs_user_made_fit_for_windows() {
    assert_eq!(account_name("nick"), "nick");
    assert_eq!(account_name("Pat O'Brien"), "PatOBrien");
    assert_eq!(account_name("1abc"), "user1abc");
    assert_eq!(account_name(""), "user");
    assert_eq!(account_name("administrator"), "administrator1");
    assert_eq!(account_name("Guest"), "Guest1");
    // The shared list is the rule now: it has the setup-made names, and not "admin".
    assert_eq!(account_name("defaultuser0"), "defaultuser01");
    assert_eq!(account_name("admin"), "admin");
    assert_eq!(account_name("a-very-long-short-name-indeed").len(), MOST_WINDOWS_NAME_CHARS);
}

#[test]
fn the_answers_name_the_account_with_its_password_and_trust_the_public_certificate() {
    let answers = Answers {
        account: "nick",
        password: "k7mn-p3xq-ab9d-w4ht",
        computer_name: "GUESTHOUSE",
        edition: "Windows 11 Pro",
        ui_language: None,
    };
    let text = autounattend(&answers);
    assert!(text.contains("<Name>nick</Name>"));
    assert!(text.contains("<Username>nick</Username>"));
    // The password, said plainly, twice: the account and the automatic sign-in.
    assert_eq!(
        text.matches("<Password><Value>k7mn-p3xq-ab9d-w4ht</Value><PlainText>true</PlainText></Password>")
            .count(),
        2
    );
    // The public certificate goes in the guest's trusted stores before test signing is turned on,
    // and the driver is installed from its package, which signs nothing here.
    let root = text.find("addstore -f Root").expect("trusted root");
    let publisher = text.find("addstore -f TrustedPublisher").expect("trusted publisher");
    let signing = text.find("bcdedit /set testsigning on").expect("test signing");
    assert!(root < signing && publisher < signing);
    assert!(text.contains("ruxn-driver.cer") && text.contains("install.ps1"));
    assert!(!text.contains("wddm.ps1") && !text.contains("pfx"));
    assert!(text.contains("BypassTPMCheck") && text.contains("BypassSecureBootCheck"));
    assert!(text.contains("<Value>Windows 11 Pro</Value>"));
    assert!(text.contains(r#"processorArchitecture="arm64""#));
    // The driver comes from the stick by its label, and each outcome is said on the serial port.
    assert!(text.contains("Get-Volume -FileSystemLabel RUXN"));
    for line in [marker::SYSTEM_INSTALLED, marker::DRIVER_INSTALLED, marker::DRIVER_FAILED] {
        assert!(text.contains(line), "{line}");
    }
}

#[test]
fn what_a_person_types_cannot_break_the_answers_out_of_their_place() {
    let answers =
        Answers { account: "a<b>&\"c", password: "x", computer_name: "X", edition: "E", ui_language: None };
    let text = autounattend(&answers);
    assert!(text.contains("<Name>a&lt;b&gt;&amp;&quot;c</Name>"));
    assert!(!text.contains("a<b>"));
}

#[test]
fn only_a_big_iso_is_taken_as_the_installer() {
    let dir = std::env::temp_dir().join(format!("gh-win-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    assert!(check_installer(&dir.join("nothing.iso")).unwrap_err().contains("not there"));
    let text = dir.join("notes.txt");
    fs::write(&text, "x").unwrap();
    assert!(check_installer(&text).unwrap_err().contains(".iso"));
    let small = dir.join("small.ISO");
    fs::write(&small, "x").unwrap();
    assert!(check_installer(&small).unwrap_err().contains("too small"));
    let big = dir.join("Win11_arm64.iso");
    let file = fs::File::create(&big).unwrap();
    file.set_len(MIN_INSTALLER_BYTES + 1).unwrap();
    assert_eq!(check_installer(&big).unwrap(), big);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn base64_is_the_standard_one_for_utf16_text() {
    // "Man" as UTF-16 little-endian is 4D 00 61 00 6E 00.
    assert_eq!(base64_utf16("Man"), "TQBhAG4A");
    assert_eq!(base64_utf16("a"), "YQA=");
    assert_eq!(base64_utf16(""), "");
}

#[test]
fn the_second_start_says_so_through_a_run_once_that_needs_no_quoting() {
    let text = autounattend(&Answers {
        account: "nick",
        password: "x",
        computer_name: "X",
        edition: "E",
        ui_language: None,
    });
    assert!(text.contains("RunOnce"));
    assert!(text.contains("-EncodedCommand "));
    assert!(!text.contains("first start ok"), "the words are in the encoded command, not the plain text");
}

#[test]
fn setup_gets_the_generic_key_of_the_edition_so_the_product_key_page_is_not_shown() {
    let for_edition = |edition: &str| {
        autounattend(&Answers { account: "a", password: "x", computer_name: "X", edition, ui_language: None })
    };
    // The shared table's keys, each in the user data beside the licence.
    let pro = for_edition("Windows 11 Pro");
    assert!(pro.contains(
        "<UserData><ProductKey><Key>W269N-WFGWX-YVC9B-4J6C9-T83GX</Key><WillShowUI>OnError</WillShowUI></ProductKey><AcceptEula>true</AcceptEula></UserData>"
    ));
    let iot = for_edition("Windows 11 IoT Enterprise LTSC");
    assert!(iot.contains("<Key>KBN8V-HFGQ4-MGXVD-347P6-PDQGT</Key>"));
    // The image's own yearly name picks the same edition.
    assert!(for_edition("Windows 11 IoT Enterprise LTSC 2024").contains("<Key>KBN8V-"));
    // An edition the table lacks is installed as it is named, with no key made up for it.
    let other = for_edition("Windows 11 Home");
    assert!(!other.contains("<ProductKey>") && other.contains("<UserData><AcceptEula>"));
}

#[test]
fn setup_is_given_its_locale_so_no_keyboard_or_language_page_is_left_for_a_person() {
    let text = autounattend(&Answers {
        account: "a",
        password: "x",
        computer_name: "X",
        edition: "E",
        ui_language: None,
    });
    // The component is in the windowsPE pass, before Setup's own, as the shared answer file has it.
    let international = text.find("Microsoft-Windows-International-Core-WinPE").expect("the component");
    assert!(international < text.find("Microsoft-Windows-Setup").unwrap());
    assert!(text.contains(
        "<InputLocale>en-US</InputLocale><SystemLocale>en-US</SystemLocale><UserLocale>en-US</UserLocale>"
    ));
    // No language is named that the installer might not carry.
    assert!(!text.contains("<UILanguage>") && !text.contains("<SetupUILanguage>"));
    let named = autounattend(&Answers {
        account: "a",
        password: "x",
        computer_name: "X",
        edition: "E",
        ui_language: Some("en-US"),
    });
    assert!(named.contains(
        "<SetupUILanguage><UILanguage>en-US</UILanguage></SetupUILanguage><UILanguage>en-US</UILanguage>"
    ));
}
