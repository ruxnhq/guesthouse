//! guest/windows/guest-rights.ps1, which keeps C:\gpupv for SYSTEM and administrators. Its folder
//! functions run on a folder that stands in for C:\gpupv as a guest made before it has it: a folder
//! that takes "Authenticated Users may change" from above, as one made at the top of drive C does, with
//! a file someone else gave rights on, a link out of it and a second name for a file outside it. The
//! folder's owner in the test is the account the test runs as, as the test can't give it to the
//! administrators' group, and so that account may change it as the administrators may in a guest. Answer-file removal runs anywhere, with the setup values made up.

use crate::{Said, Scratch, functions_from, guest_script, quote, run, shell};

fn rights_script() -> std::path::PathBuf {
    guest_script("guest-rights.ps1")
}

fn run_with(shell: &str, functions: &[&str], command: &str) -> Said {
    run(
        shell,
        &format!("$ErrorActionPreference = 'Stop'\n{}{command}", functions_from(&rights_script(), functions)),
    )
}

#[cfg(windows)]
const FOLDER_FUNCTIONS: [&str; 9] = [
    "Get-Sid",
    "Get-Rule",
    "Test-Link",
    "Remove-Link",
    "Set-FolderRules",
    "Test-Inherits",
    "Repair-Inside",
    "Set-TokenRules",
    "Set-GuestFolder",
];

/// What the folder has, after: each item a SID other than SYSTEM's or the administrators' may change,
/// each it may read the token as, whether the folder still takes rules from above, and what became of
/// the link and the file outside.
#[cfg(windows)]
const LOOK: &str = r#"
$changes = [System.Security.AccessControl.FileSystemRights]'WriteData, AppendData, WriteExtendedAttributes, WriteAttributes, Delete, DeleteSubdirectoriesAndFiles, ChangePermissions, TakeOwnership'
$ours = 'S-1-5-18', 'S-1-5-32-544', $me
foreach ($item in @(Get-Item -LiteralPath $root -Force) + @(Get-ChildItem -LiteralPath $root -Recurse -Force)) {
    foreach ($rule in (Get-Acl -LiteralPath $item.FullName).GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier])) {
        if ($ours -contains $rule.IdentityReference.Value -or $rule.AccessControlType -ne 'Allow') { continue }
        if ($rule.FileSystemRights -band $changes) { "changes $($item.Name) $($rule.IdentityReference.Value)" }
        if ($item.Name -eq 'token.txt') { "reads token.txt $($rule.IdentityReference.Value)" }
    }
}
"from above $(-not (Get-Acl -LiteralPath $root).AreAccessRulesProtected)"
"link there $(Test-Path -LiteralPath (Join-Path $root 'lean'))"
"outside kept $(Test-Path -LiteralPath (Join-Path $outside 'keep.txt'))"
"second name there $(Test-Path -LiteralPath (Join-Path $root 'second.txt'))"
"outside rules kept $((Get-Acl -LiteralPath (Join-Path $outside 'linked.txt')).Sddl -eq $linkedBefore)"
"#;

/// Gives the test's account its folder back, so the scratch folder can go.
#[cfg(windows)]
const GIVE_BACK: &str = r#"
$me = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
& icacls.exe $root /grant "*${me}:(OI)(CI)F" /T /C /Q | Out-Null
"#;

/// A folder as a guest made before has it: it takes "Authenticated Users may change" from above.
#[cfg(windows)]
const MAKE: &str = r#"
$me = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
New-Item -ItemType Directory -Force $root, (Join-Path $root 'cap'), $outside | Out-Null
$above = Get-Acl -LiteralPath $root
$above.AddAccessRule((New-Object System.Security.AccessControl.FileSystemAccessRule (New-Object System.Security.Principal.SecurityIdentifier 'S-1-5-11'), 'Modify', 'ContainerInherit, ObjectInherit', 'None', 'Allow'))
Set-Acl -LiteralPath $root -AclObject $above
'agent' | Set-Content -LiteralPath (Join-Path $root 'agent.ps1')
'token' | Set-Content -LiteralPath (Join-Path $root 'token.txt')
'mode' | Set-Content -LiteralPath (Join-Path $root 'cap\setmode.txt')
$given = Join-Path $root 'given.ps1'
'given' | Set-Content -LiteralPath $given
$rules = Get-Acl -LiteralPath $given
$rules.AddAccessRule((New-Object System.Security.AccessControl.FileSystemAccessRule (New-Object System.Security.Principal.SecurityIdentifier 'S-1-1-0'), 'FullControl', 'Allow'))
Set-Acl -LiteralPath $given -AclObject $rules
'keep' | Set-Content -LiteralPath (Join-Path $outside 'keep.txt')
'linked' | Set-Content -LiteralPath (Join-Path $outside 'linked.txt')
New-Item -ItemType Junction -Path (Join-Path $root 'lean') -Target $outside | Out-Null
New-Item -ItemType HardLink -Path (Join-Path $root 'second.txt') -Target (Join-Path $outside 'linked.txt') | Out-Null
$linkedBefore = (Get-Acl -LiteralPath (Join-Path $outside 'linked.txt')).Sddl
"#;

#[cfg(windows)]
#[test]
fn only_system_and_administrators_change_the_guest_folder() {
    let Some(shell) = shell() else { return };
    let scratch = Scratch::new();
    let (root, outside) = (scratch.path("gpupv"), scratch.path("outside"));
    let said = run_with(
        shell,
        &FOLDER_FUNCTIONS,
        &format!(
            "$root = {}\n$outside = {}\n{MAKE}\ntry {{ Set-GuestFolder $root $me | Out-Null\n{LOOK} }} finally {{ {GIVE_BACK} }}",
            quote(&root),
            quote(&outside)
        ),
    );
    assert_eq!(said.code, Some(0), "{said}");
    assert_eq!(
        said.lines(),
        [
            "from above False",
            "link there False",
            "outside kept True",
            "second name there False",
            "outside rules kept True",
        ],
        "{said}"
    );
}

/// The token is rewritten in place when the host moves the guest to a new one, as `Set-Content` does,
/// and it keeps its rules through that.
#[cfg(windows)]
#[test]
fn the_token_keeps_its_rules_when_it_changes() {
    let Some(shell) = shell() else { return };
    let scratch = Scratch::new();
    let (root, outside) = (scratch.path("gpupv"), scratch.path("outside"));
    let said = run_with(
        shell,
        &FOLDER_FUNCTIONS,
        &format!(
            "$root = {}\n$outside = {}\n{MAKE}\ntry {{ Set-GuestFolder $root $me | Out-Null
            Set-Content -LiteralPath (Join-Path $root 'token.txt') -Value 'new token' -ErrorAction Stop
            $rules = Get-Acl -LiteralPath (Join-Path $root 'token.txt')
            \"protected $($rules.AreAccessRulesProtected)\"
            ($rules.GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier]) | ForEach-Object {{ $_.IdentityReference.Value }} | Where-Object {{ $_ -ne $me }} | Sort-Object) -join ' '
            }} finally {{ {GIVE_BACK} }}",
            quote(&root),
            quote(&outside)
        ),
    );
    assert_eq!(said.code, Some(0), "{said}");
    assert_eq!(said.lines(), ["protected True", "S-1-5-18 S-1-5-32-544"], "{said}");
}

/// A session script's file: the account may read and write it, and not take it away.
#[cfg(windows)]
#[test]
fn a_session_file_is_the_accounts_to_write_and_not_to_take_away() {
    let Some(shell) = shell() else { return };
    let scratch = Scratch::new();
    let file = scratch.path("setmode.txt");
    let said = run_with(
        shell,
        &["Get-Rule", "Grant-SessionFile"],
        &format!(
            "$file = {}
            $me = [System.Security.Principal.WindowsIdentity]::GetCurrent()
            Grant-SessionFile $file $me.Name
            \"empty $((Get-Item -LiteralPath $file).Length -eq 0)\"
            foreach ($rule in (Get-Acl -LiteralPath $file).GetAccessRules($true, $false, [System.Security.Principal.SecurityIdentifier])) {{
                if ($rule.IdentityReference.Value -ne $me.User.Value) {{ continue }}
                \"write $([bool]($rule.FileSystemRights -band [System.Security.AccessControl.FileSystemRights]::WriteData))\"
                \"delete $([bool]($rule.FileSystemRights -band [System.Security.AccessControl.FileSystemRights]::Delete))\"
            }}",
            quote(&file)
        ),
    );
    assert_eq!(said.code, Some(0), "{said}");
    assert_eq!(said.lines(), ["empty True", "write True", "delete False"], "{said}");
}

/// Setup's values as Windows keeps them under HKLM\SYSTEM\Setup, made up, so nothing reads the
/// registry.
fn answer_file_after(setup: &str) -> (Said, bool) {
    let Some(shell) = shell() else { return (run_nothing(), true) };
    let scratch = Scratch::new();
    let file = scratch.path("unattend.xml");
    std::fs::write(&file, "<Password>").unwrap();
    let said = run_with(
        shell,
        &["Remove-AnswerFile"],
        &format!(
            "function Get-ItemProperty {{ param($LiteralPath, $ErrorAction) if ($LiteralPath -eq 'setup') {{ {setup} }} }}
            Remove-AnswerFile {} 'setup'",
            quote(&file)
        ),
    );
    let kept = file.exists();
    (said, kept)
}

fn run_nothing() -> Said {
    Said { stdout: String::new(), stderr: String::new(), code: Some(0), command: String::new() }
}

#[test]
fn the_answer_file_goes_once_setup_has_finished() {
    if shell().is_none() {
        return;
    }
    let (said, kept) =
        answer_file_after("[pscustomobject]@{ SystemSetupInProgress = 0; OOBEInProgress = 0 }");
    assert_eq!(said.code, Some(0), "{said}");
    assert!(!kept, "{said}");
}

#[test]
fn the_answer_file_stays_while_setup_runs() {
    if shell().is_none() {
        return;
    }
    for setup in [
        "[pscustomobject]@{ SystemSetupInProgress = 1; OOBEInProgress = 0 }",
        "[pscustomobject]@{ SystemSetupInProgress = 0; OOBEInProgress = 1 }",
        "$null",
    ] {
        let (said, kept) = answer_file_after(setup);
        assert_eq!(said.code, Some(0), "{said}");
        assert!(kept, "{setup}{said}");
    }
}

/// The Windows host's report has its own copy of the function, run at each of its guests' starts.
#[test]
fn the_windows_hosts_report_removes_the_answer_file_the_same_way() {
    let function = |path: &std::path::Path| {
        let text = std::fs::read_to_string(path).unwrap();
        let start = text.find("function Remove-AnswerFile").unwrap_or_else(|| panic!("{}", path.display()));
        let end = start + text[start..].find("\n}\n").unwrap();
        text[start..end].to_owned()
    };
    let report = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../windows/host/guest/report.ps1");
    assert_eq!(function(&report), function(&rights_script()));
    let text = std::fs::read_to_string(&report).unwrap();
    assert!(text.contains("Remove-AnswerFile 'C:\\Windows\\Panther\\unattend.xml' 'HKLM:\\SYSTEM\\Setup'"));
}

/// A recipe's runner writes in the run's folder it is given, in the account's profile in a guest,
/// and in nothing of SYSTEM's: here it runs a script step from a recipe in a stand-in folder.
#[cfg(windows)]
#[test]
fn a_recipe_writes_only_in_its_runs_own_folder() {
    let Some(shell) = shell() else { return };
    let scratch = Scratch::new();
    let runs = scratch.path("runs");
    std::fs::create_dir_all(runs.join("earlier")).unwrap();
    std::fs::write(runs.join("earlier/status.txt"), "end\n").unwrap();
    let data = scratch.path("data.json");
    std::fs::write(
        &data,
        r#"{"winget_seconds": 1, "steps": [{"number": 1, "kind": "script", "value": "exit 0"}]}"#,
    )
    .unwrap();
    let work = runs.join("this");
    let said = run(
        shell,
        &format!(
            "& {} -Work {} -Data {}",
            quote(&guest_script("recipe-run.ps1")),
            quote(&work),
            quote(&data)
        ),
    );
    let status = std::fs::read_to_string(work.join("status.txt")).unwrap_or_default();
    assert_eq!(status.lines().collect::<Vec<_>>(), ["begin", "start 1", "ok 1", "end"], "{said}");
    assert!(work.join("run.pid").is_file() && work.join("step-1.ps1").is_file(), "{said}");
    assert!(!runs.join("earlier").exists(), "an earlier run's folder goes{said}");
    assert_eq!(
        crate::names(scratch.path("").as_path()),
        ["data.json", "runs/this/run.pid", "runs/this/status.txt", "runs/this/step-1.ps1"]
    );
}
