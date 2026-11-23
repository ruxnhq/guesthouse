use super::*;

/// What must never be taken out, by any name that could stand for it.
const KEPT: [&str; 16] = [
    "Microsoft.WindowsStore",
    "Microsoft.DesktopAppInstaller",
    "Microsoft.StorePurchaseApp",
    "Microsoft.GamingApp",
    "Microsoft.XboxGamingOverlay",
    "Microsoft.XboxIdentityProvider",
    "Microsoft.Xbox.TCUI",
    "Microsoft.XboxSpeechToTextOverlay",
    "Microsoft.GamingServices",
    "Microsoft.SecHealthUI",
    "Microsoft.Windows.ShellExperienceHost",
    "Microsoft.MicrosoftEdge.Stable",
    "Microsoft.WebView2",
    "Microsoft.GetHelp",
    "MicrosoftCorporationII.QuickAssist",
    "Microsoft.LanguageExperiencePack",
];

#[test]
fn an_app_is_named_by_a_plain_package_name_and_nothing_wider() {
    for app in &APPS {
        let name = app.name;
        assert!((2..=128).contains(&name.len()), "{name}");
        assert!(
            name.chars().all(|c| c.is_ascii_alphanumeric() || c == '.'),
            "{name} has a wildcard or a space"
        );
    }
    let mut names: Vec<_> = APPS.iter().map(|app| app.name.to_lowercase()).collect();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), APPS.len(), "a name is there twice");
}

#[test]
fn nothing_windows_or_a_recipe_needs_is_on_the_list() {
    for app in &APPS {
        for kept in KEPT {
            assert!(!app.name.eq_ignore_ascii_case(kept), "{kept}");
        }
        let lower = app.name.to_lowercase();
        for part in [
            "xbox",
            "gaming",
            "store",
            "appinstaller",
            "edge",
            "webview",
            "vclibs",
            "net.native",
            "ui.xaml",
            "language",
            "sechealth",
            "defender",
            "accessib",
        ] {
            assert!(!lower.contains(part), "{} looks like something to keep", app.name);
        }
    }
}

#[test]
fn every_policy_is_a_policy_and_none_is_about_security_or_updates() {
    for policy in &POLICIES {
        assert!(policy.key.starts_with(r"Microsoft\"), "{}", policy.key);
        assert!(policy.key.chars().all(|c| c.is_ascii_alphanumeric() || c == '\\'), "{}", policy.key);
        assert!(policy.value.chars().all(|c| c.is_ascii_alphanumeric()), "{}", policy.value);
        let lower = format!("{}\\{}", policy.key, policy.value).to_lowercase();
        for part in [
            "defender",
            "firewall",
            "smartscreen",
            "windowsupdate",
            "update",
            "system\\",
            "deviceguard",
            "bitlocker",
            "fve",
        ] {
            assert!(!lower.contains(part), "{lower}");
        }
    }
}

#[test]
fn the_user_reads_every_item_the_guest_is_given() {
    let text = what_it_does();
    for app in &APPS {
        assert!(text.contains(&format!("({})", app.name)), "{}", app.name);
    }
    for policy in &POLICIES {
        assert!(text.contains(&format!(
            "{} = {} in HKLM\\SOFTWARE\\Policies\\{}",
            policy.value, policy.data, policy.key
        )));
    }
    assert!(text.contains("Defender, the firewall, SmartScreen, Windows Update"));
    let data: serde_json::Value = serde_json::from_str(&guest_data()).unwrap();
    assert_eq!(data["apps"].as_array().unwrap().len(), APPS.len());
    assert_eq!(data["policies"].as_array().unwrap().len(), POLICIES.len());
    assert_eq!(data["policies"][1]["key"], r"Microsoft\Dsh");
}
