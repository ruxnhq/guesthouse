//! The repo's own guest folder: everything the app sends is there, and nothing that must not be.

#![cfg(target_os = "linux")]

use std::fs;
use std::path::PathBuf;

const SENT_TO_THE_GUEST: [&str; 32] = [
    "SetupComplete.cmd",
    "agent.ps1",
    "api-check.ps1",
    "autounattend.xml",
    "check.ps1",
    "clipboard-ask.ps1",
    "clipboard-start.ps1",
    "clipboard.ps1",
    "display-remove.ps1",
    "display-name.ps1",
    "display.ps1",
    "files-start.ps1",
    "files.ps1",
    "finish.ps1",
    "helper-stop.ps1",
    "idle-input.ps1",
    "idle.ps1",
    "lean-check.ps1",
    "lean-start.ps1",
    "lean.ps1",
    "nv-files.ps1",
    "nv-media-foundation.ps1",
    "nv-profiles.ps1",
    "nvme-ready.ps1",
    "scan-exclusions.ps1",
    "setmode.ps1",
    "setup.ps1",
    "share-start.ps1",
    "window-size-ask.ps1",
    "window-size-start.ps1",
    "window-size.ps1",
    "window-speed.ps1",
];

fn guest_file(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../guest").join(name)
}

#[test]
fn everything_the_app_sends_is_in_the_repo() {
    for name in SENT_TO_THE_GUEST {
        assert!(guest_file(name).is_file(), "{name} is missing");
    }
}

/// The registry entries are Windows' own to share with other programs, so the script may only ever
/// touch the ones under its own name, and never names a folder the user didn't make or install into.
#[test]
fn the_window_speed_script_removes_only_its_own_entries() {
    let script = fs::read_to_string(guest_file("window-speed.ps1")).unwrap();
    assert!(script.contains("$ours = 'guesthouse-window-speed-'"));
    let removals: Vec<&str> = script.lines().filter(|line| line.contains("Remove-Item")).collect();
    assert_eq!(removals.len(), 2, "{removals:?}");
    assert!(removals[0].trim_start().starts_with("Our-Entries $key |"), "{}", removals[0]);
    assert!(removals[1].contains("SubKeyCount -eq 0") && removals[1].contains("ValueCount -eq 0"));
    assert!(script.contains("$_.FullName -ne $windows"), "Windows' own folder gets no entry");
    assert!(script.contains("WOW6432Node"), "a 32-bit program reads its own copy of the key");
}

/// Which transforms NVIDIA's driver has for a card, what they're called and which files they're in
/// is the driver's to say, and the user brings the driver. The script holds none of that: it reads
/// the INF in the guest, for the card the host names, and writes only through the one function that
/// takes an entry read from it.
#[test]
fn the_transforms_script_registers_only_what_the_drivers_inf_asks_for() {
    let script = fs::read_to_string(guest_file("nv-media-foundation.ps1")).unwrap();
    let code: Vec<&str> = script.lines().filter(|line| !line.trim_start().starts_with('#')).collect();
    let id = regex::Regex::new("[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}").unwrap();
    for line in &code {
        assert!(!id.is_match(line), "a transform's ID is in the script: {line}");
        assert!(!line.to_lowercase().contains(".dll"), "a driver file is named in the script: {line}");
    }
    assert!(script.contains("[IO.File]::ReadAllLines($inf,"));
    assert!(script.trim_end().ends_with("\nRegister-Transforms $store $card"));
    assert!(!script.contains("$card ="), "the card is the host's to name");
    let writes: Vec<&&str> = code.iter().filter(|line| line.contains(".SetValue(")).collect();
    assert_eq!(writes.len(), 2, "{writes:?}");
    assert!(writes.iter().all(|line| line.contains("$key.SetValue($entry.Name, ")), "{writes:?}");
    assert_eq!(script.matches("Write-Entry $entry").count(), 1, "only an entry from the INF is written");
    for removal in ["Remove-Item", "DeleteSubKey", "DeleteValue"] {
        assert!(!script.contains(removal), "nothing in the registry is removed: {removal}");
    }
}

/// The switch for hardware transforms is Windows' own, and every maker's transforms go by it, so one
/// that someone set otherwise has to last through every start. A transform's own entries follow the
/// INF, as that is how a changed driver's are taken.
#[test]
fn the_transforms_script_leaves_the_switch_as_someone_set_it() {
    let script = fs::read_to_string(guest_file("nv-media-foundation.ps1")).unwrap();
    let left = "if ($has -eq 'other' -and $entry.Key -match $switchKey) {";
    let (before, after) = script.split_once(left).expect("a switch value with other data is left");
    assert!(!before.contains("Write-Entry $entry"), "it is left before anything is written");
    assert!(after.trim_start().starts_with("$left++"), "and counted, so the start says so");
    assert!(script.contains("if (-not ($written -or $failed -or $left)) { return \"kept $all in "));
}

/// The helper for dragged files listens on the guest's network, so both the firewall rule and the
/// helper itself have to keep it to this PC's end of QEMU's private link.
#[test]
fn the_files_helper_only_answers_the_host() {
    let start = fs::read_to_string(guest_file("files-start.ps1")).unwrap();
    let rules: Vec<&str> = start.lines().filter(|line| line.contains("New-NetFirewallRule")).collect();
    assert_eq!(rules.len(), 1, "{rules:?}");
    assert!(rules[0].contains("-LocalPort 8767 -RemoteAddress 10.0.2.2"), "{}", rules[0]);
    let helper = fs::read_to_string(guest_file("files.ps1")).unwrap();
    assert!(helper.contains("const string HostAddress = \"10.0.2.2\";"));
    assert!(helper.contains("RemoteEndPoint).Address.ToString() != HostAddress) return;"));
    assert!(helper.contains("words[0] != token) return;"));
}

/// A helper's task runs conhost, and stopping the task leaves the PowerShell under it holding the
/// helper's port. The next helper then can't listen, and a loop that went on would keep a core busy.
#[test]
fn a_stopped_helper_leaves_no_powershell_and_one_that_cannot_listen_leaves() {
    let stop = fs::read_to_string(guest_file("helper-stop.ps1")).unwrap();
    assert!(stop.contains("Stop-ScheduledTask -TaskName \"gpupv-$helperName\""), "{stop}");
    assert!(stop.contains("C:\\gpupv\\$helperName.ps1") && stop.contains("Stop-Process"), "{stop}");
    for helper in ["clipboard.ps1", "window-size.ps1"] {
        let text = fs::read_to_string(guest_file(helper)).unwrap();
        assert!(text.contains("try { $listener.Start() } catch { exit 1 }"), "{helper}");
        assert!(text.contains("try { $client = $listener.AcceptTcpClient() } catch { exit 1 }"), "{helper}");
    }
    for name in ["clipboard", "files", "window-size"] {
        let start = fs::read_to_string(guest_file(&format!("{name}-start.ps1"))).unwrap();
        assert!(start.contains(&format!("-File C:\\gpupv\\{name}.ps1")), "the stop finds it by this: {name}");
    }
}

#[test]
fn the_answer_file_holds_no_password() {
    let answers = fs::read_to_string(guest_file("autounattend.xml")).unwrap();
    assert_eq!(answers.matches("<Value>@PASSWORD@</Value>").count(), 2);
    assert_eq!(answers.matches("<PlainText>true</PlainText>").count(), 2);
}

#[test]
fn the_account_is_named_by_the_host_everywhere_the_scripts_need_it() {
    let answers = fs::read_to_string(guest_file("autounattend.xml")).unwrap();
    assert_eq!(answers.matches("@USER@").count(), 4, "full name, name, display name and the sign-in");
    assert!(!answers.contains(">gpupv<"), "the default name belongs to the app, not the template");
    for name in ["finish.ps1", "check.ps1"] {
        let script = fs::read_to_string(guest_file(name)).unwrap();
        assert!(script.contains("$user = '@USER@'"), "{name} takes the name from the host");
        assert!(!script.contains("gpupv$"), "{name} still matches the default name by hand");
    }
}

/// Windows names its groups in its own language, so a script that asks for one by its English
/// name works on an English Windows only.
#[test]
fn no_script_finds_a_group_by_its_english_name() {
    for name in SENT_TO_THE_GUEST.iter().filter(|name| name.ends_with(".ps1")) {
        let script = fs::read_to_string(guest_file(name)).unwrap();
        for line in script.lines().filter(|line| !line.trim_start().starts_with('#')) {
            assert!(!line.contains("Administrators"), "{name} names the Administrators group: {line}");
        }
    }
    let check = fs::read_to_string(guest_file("check.ps1")).unwrap();
    assert!(check.contains("-SID $administrators"), "the check finds the group by its number");
    assert!(check.contains("$administrators = 'S-1-5-32-544'"));
}

/// Windows is installed without a licence, and the only keys the answer file can carry are Microsoft's
/// KMS client keys, which pick an edition and activate nothing. The template itself carries none: the
/// edition and its key are filled in from the disc the machine installs from.
#[test]
fn the_only_keys_are_the_published_ones_that_activate_nothing() {
    let answers = fs::read_to_string(guest_file("autounattend.xml")).unwrap();
    assert!(!answers.contains("<ProductKey>"), "the template picks its key when it is filled");
    assert!(answers.contains("<Value>@EDITION@</Value>"));
    assert!(answers.contains("@PRODUCT_KEY@"));
    let keys: Vec<&str> = guesthouse_app_core::iso::WINDOWS_EDITIONS.iter().map(|(_, key)| *key).collect();
    assert_eq!(
        keys,
        ["W269N-WFGWX-YVC9B-4J6C9-T83GX", "KBN8V-HFGQ4-MGXVD-347P6-PDQGT"],
        "adding another key is a decision, and one that would activate Windows is not one this makes"
    );
}

#[test]
fn only_the_host_reaches_the_agent() {
    let setup = fs::read_to_string(guest_file("setup.ps1")).unwrap();
    assert!(setup.contains("-RemoteAddress 10.0.2.2"));
    let agent = fs::read_to_string(guest_file("agent.ps1")).unwrap();
    assert!(agent.contains("$hostAddress = '10.0.2.2'"));
    assert!(agent.contains("RemoteEndPoint"));
    assert!(agent.contains("ReadTimeout"));
}

#[test]
fn no_token_is_kept_in_the_folder() {
    assert!(!guest_file(".token").exists());
    assert!(!guest_file("token.txt").exists());
}

/// A task in the user's session that starts PowerShell by itself gets a window from the default
/// terminal, which is Windows Terminal on Windows 11, and Windows Terminal doesn't hide it for
/// -WindowStyle Hidden: an empty PowerShell window stays open as long as the helper runs. Each one
/// starts PowerShell in a console with no window instead. The agent runs as SYSTEM, with no desktop.
#[test]
fn no_task_in_the_user_s_session_opens_a_window() {
    for name in SENT_TO_THE_GUEST.iter().filter(|name| name.ends_with(".ps1")) {
        let script = fs::read_to_string(guest_file(name)).unwrap();
        for line in script.lines().filter(|line| line.contains("New-ScheduledTaskAction")) {
            if *name == "setup.ps1" {
                assert!(script.contains("-User 'SYSTEM'"), "{line}");
                continue;
            }
            assert!(
                line.contains("-Execute conhost.exe -Argument ")
                    && line.contains("--headless powershell.exe"),
                "{name}: {line}"
            );
            assert!(!line.contains("WindowStyle"), "{name}: {line}");
        }
    }
}
