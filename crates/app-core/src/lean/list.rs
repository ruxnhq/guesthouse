//! What a lean Windows leaves out and switches off, each with the reason, in one place. The words the
//! user reads before saying yes and the data the guest is given are both made from these tables, so
//! they can't say different things.
//!
//! Only two kinds of thing are here, both documented by Microsoft and both undone the documented way:
//! an app package, named by its package name, which is the same in every language and edition, and a
//! policy value under `HKLM\SOFTWARE\Policies`. Nothing that keeps Windows safe or up to date is in
//! either table, and nothing the app or a recipe needs.

use serde_json::json;
use std::fmt::Write;

/// An app Windows puts in for every new account, which a lean Windows takes out before the account's
/// first logon, and from every account after the install. Getting one back is a click in the Store.
pub struct App {
    /// The package's name, not its title: the same on every edition and in every language.
    pub name: &'static str,
    /// What a person knows it as.
    pub title: &'static str,
    /// What leaving it out saves.
    pub saves: &'static str,
}

/// A documented policy, a number under `HKLM\SOFTWARE\Policies`. Deleting the value undoes it.
pub struct Policy {
    /// Below `HKLM\SOFTWARE\Policies\`.
    pub key: &'static str,
    pub value: &'static str,
    pub data: u32,
    /// What it switches off, and what that saves.
    pub what: &'static str,
}

const BACKGROUND: &str = "a program that runs in the background from logon";
const UPDATES: &str = "disk space, and the Store keeping it up to date";

pub const APPS: [App; 22] = [
    App { name: "Microsoft.YourPhone", title: "Phone Link", saves: BACKGROUND },
    App { name: "MSTeams", title: "Teams (free)", saves: BACKGROUND },
    App { name: "MicrosoftTeams", title: "Teams chat, of Windows 11's first releases", saves: BACKGROUND },
    App { name: "Microsoft.549981C3F5F10", title: "Cortana", saves: BACKGROUND },
    App {
        name: "Microsoft.Copilot",
        title: "Copilot",
        saves: "a program that can start at logon, and its updates",
    },
    App { name: "Microsoft.BingNews", title: "News", saves: UPDATES },
    App { name: "Microsoft.BingWeather", title: "Weather", saves: UPDATES },
    App { name: "Microsoft.Getstarted", title: "Tips", saves: "the tips it shows after the first logon" },
    App { name: "Microsoft.MicrosoftOfficeHub", title: "Microsoft 365 (Office)", saves: UPDATES },
    App { name: "Microsoft.OutlookForWindows", title: "Outlook (new)", saves: UPDATES },
    App { name: "microsoft.windowscommunicationsapps", title: "Mail and Calendar", saves: UPDATES },
    App { name: "Microsoft.MicrosoftSolitaireCollection", title: "Solitaire Collection", saves: UPDATES },
    App { name: "Clipchamp.Clipchamp", title: "Clipchamp", saves: UPDATES },
    App { name: "Microsoft.PowerAutomateDesktop", title: "Power Automate", saves: UPDATES },
    App { name: "Microsoft.Todos", title: "Microsoft To Do", saves: UPDATES },
    App { name: "Microsoft.Windows.DevHome", title: "Dev Home", saves: UPDATES },
    App { name: "Microsoft.WindowsFeedbackHub", title: "Feedback Hub", saves: UPDATES },
    App { name: "Microsoft.WindowsMaps", title: "Maps", saves: "disk space, and its map updates" },
    App { name: "Microsoft.ZuneVideo", title: "Movies & TV", saves: UPDATES },
    App { name: "MicrosoftCorporationII.MicrosoftFamily", title: "Family", saves: UPDATES },
    App { name: "Microsoft.People", title: "People, on Windows 10", saves: UPDATES },
    App { name: "Microsoft.SkypeApp", title: "Skype, on Windows 10", saves: UPDATES },
];

pub const POLICIES: [Policy; 5] = [
    Policy {
        key: r"Microsoft\Windows\CloudContent",
        value: "DisableWindowsConsumerFeatures",
        data: 1,
        what: "Windows puts no suggested apps from other makers in the new account. Enterprise, Education \
               and IoT Enterprise honour this. Home and Pro ignore it.",
    },
    Policy {
        key: r"Microsoft\Dsh",
        value: "AllowNewsAndInterests",
        data: 0,
        what: "Widgets are off, so their programs don't run from logon.",
    },
    Policy {
        key: r"Microsoft\Windows\OneDrive",
        value: "DisableFileSyncNGSC",
        data: 1,
        what: "OneDrive doesn't run or sync, so it isn't in the background from logon.",
    },
    Policy {
        key: r"Microsoft\Edge",
        value: "StartupBoostEnabled",
        data: 0,
        what: "Edge doesn't start at logon to open faster later.",
    },
    Policy {
        key: r"Microsoft\Edge",
        value: "BackgroundModeEnabled",
        data: 0,
        what: "Edge doesn't keep running after its last window closes.",
    },
];

/// Packages that lean Windows never touches, but that Windows then doesn't add after the first logon,
/// as what brings them is gone or switched off. A plain and a lean install of Windows 11 build 26300
/// differed by these two besides the list. Which of the list keeps each away is read from what each
/// is for, and not from Microsoft.
pub const NOT_ADDED: [App; 2] = [
    App {
        name: "Microsoft.StartExperiencesApp",
        title: "Start's companion content, such as the Phone Link panel",
        saves: "kept away with Phone Link, it seems",
    },
    App {
        name: "Microsoft.WidgetsPlatformRuntime",
        title: "the runtime Widgets run on",
        saves: "kept away with Widgets switched off, it seems",
    },
];

/// What stays, and why, for the person deciding.
pub const LEFT_ALONE: [&str; 6] = [
    "Defender, the firewall, SmartScreen, Windows Update, UAC, Secure Boot, the TPM and BitLocker, as \
     Windows sets them.",
    "The Store, App Installer and winget, which the recipes install with.",
    "Every Xbox part, Game Bar and the Xbox identity provider, which games and Game Pass need.",
    "Edge, and the WebView2 runtime many programs need.",
    "Get Help, which is Windows 11's troubleshooters, and Quick Assist.",
    "The tools: Photos, Media Player, Notepad, Paint, Calculator, Terminal, Snipping Tool, Camera, Clock, \
     Sound Recorder and Sticky Notes, as well as the runtimes, languages, input methods and \
     accessibility tools.",
];

/// The whole of it, as the user reads it before the machine is made.
pub fn what_it_does() -> String {
    let mut text = String::from(
        "Lean Windows takes out these apps before the account first logs on, and again after the install \
         from every account:\n",
    );
    for app in &APPS {
        let _ = writeln!(text, "- {} ({}): {}", app.title, app.name, app.saves);
    }
    text.push_str("It sets these documented policies:\n");
    for policy in &POLICIES {
        let _ = writeln!(
            text,
            "- {} = {} in HKLM\\SOFTWARE\\Policies\\{}: {}",
            policy.value, policy.data, policy.key, policy.what
        );
    }
    text.push_str("Windows then doesn't add these, which it otherwise adds after the first logon:\n");
    for app in &NOT_ADDED {
        let _ = writeln!(text, "- {} ({}): {}", app.title, app.name, app.saves);
    }
    text.push_str("It leaves alone:\n");
    for item in &LEFT_ALONE {
        let _ = writeln!(text, "- {item}");
    }
    text.push_str(
        "Settings and Edge then say some settings are managed by your organization, as they do for any \
         policy. An app comes back from the Store, and a policy goes by deleting its value.",
    );
    text
}

/// The list as the guest's script is given it, as data. The script checks each item again.
pub fn guest_data() -> String {
    let apps: Vec<&str> = APPS.iter().map(|app| app.name).collect();
    let policies: Vec<_> = POLICIES
        .iter()
        .map(|policy| json!({ "key": policy.key, "value": policy.value, "data": policy.data }))
        .collect();
    json!({ "apps": apps, "policies": policies }).to_string()
}

#[cfg(test)]
#[path = "list_tests.rs"]
mod tests;
