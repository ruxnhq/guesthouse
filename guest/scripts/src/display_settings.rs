//! The display driver's settings folder, as `Set-SettingsFolder` in the driver's install script keeps it. The
//! folder here stands in for C:\VirtualDisplayDriver, and the test account stands in for the account that
//! owns it, as the test can't give it to the administrators' group: only SYSTEM, the administrators, the
//! driver's own account and the readers named below have rights to it afterwards.
#![cfg(windows)]

use crate::{Scratch, functions_from, quote, run, shell};
use std::path::Path;

fn script() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../drivers/windows-display/setup/sign-and-install.ps1")
}

/// Each SID that may change something in the folder or in what is in it, other than the ones in `ours`.
const LOOK: &str = r#"
$changes = [System.Security.AccessControl.FileSystemRights]'WriteData, AppendData, WriteExtendedAttributes, WriteAttributes, Delete, DeleteSubdirectoriesAndFiles, ChangePermissions, TakeOwnership'
$ours = 'S-1-5-18', 'S-1-5-32-544', 'S-1-5-19', $me
foreach ($item in @(Get-Item -LiteralPath $root -Force) + @(Get-ChildItem -LiteralPath $root -Recurse -Force)) {
    foreach ($rule in (Get-Acl -LiteralPath $item.FullName).GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier])) {
        if ($ours -contains $rule.IdentityReference.Value -or $rule.AccessControlType -ne 'Allow') { continue }
        if ($rule.FileSystemRights -band $changes) { "changes $($item.Name) $($rule.IdentityReference.Value)" }
    }
}
"settings kept $(Test-Path -LiteralPath (Join-Path $root 'vdd_settings.xml'))"
"extra kept $(Test-Path -LiteralPath (Join-Path $root 'planted.txt'))"
"link there $(Test-Path -LiteralPath (Join-Path $root 'sub'))"
"outside kept $(Test-Path -LiteralPath (Join-Path $outside 'keep.txt'))"
"#;

const GIVE_BACK: &str = r#"
& icacls.exe $root /grant "*${me}:(OI)(CI)F" /T /C /Q | Out-Null
"#;

/// A folder someone else made at the name, under rules from above that let every account change it, with
/// a file of theirs in it.
const MAKE: &str = r#"
$me = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
New-Item -ItemType Directory -Force $root, $outside | Out-Null
$above = Get-Acl -LiteralPath $root
$above.AddAccessRule((New-Object System.Security.AccessControl.FileSystemAccessRule (New-Object System.Security.Principal.SecurityIdentifier 'S-1-5-11'), 'Modify', 'ContainerInherit, ObjectInherit', 'None', 'Allow'))
Set-Acl -LiteralPath $root -AclObject $above
'x' | Set-Content -LiteralPath (Join-Path $root 'vdd_settings.xml')
'x' | Set-Content -LiteralPath (Join-Path $root 'planted.txt')
'keep' | Set-Content -LiteralPath (Join-Path $outside 'keep.txt')
New-Item -ItemType Junction -Path (Join-Path $root 'sub') -Target $outside | Out-Null
"#;

fn look(owners: &str) -> Vec<String> {
    let Some(shell) = shell() else { return Vec::new() };
    let scratch = Scratch::new();
    let (root, outside) = (scratch.path("VirtualDisplayDriver"), scratch.path("outside"));
    let said = run(
        shell,
        &format!(
            "$ErrorActionPreference = 'Stop'\n{}$root = {}\n$outside = {}\n{MAKE}\ntry {{ Set-SettingsFolder $root {owners}\n{LOOK} }} finally {{ {GIVE_BACK} }}",
            functions_from(&script(), &["Remove-InnerLinks", "Set-SettingsFolder"]),
            quote(&root),
            quote(&outside)
        ),
    );
    assert_eq!(said.code, Some(0), "{said}");
    said.lines().into_iter().map(str::to_owned).collect()
}

/// A folder another account owns is made anew: its files and the link in it go, the link's target stays,
/// and no other account may change what is left.
#[test]
fn a_folder_someone_else_made_is_made_anew_for_the_system() {
    if shell().is_none() {
        return;
    }
    let found = look("");
    assert_eq!(found, ["settings kept False", "extra kept False", "link there False", "outside kept True"]);
}

/// A folder its owner may keep keeps its files, which take its rules, and a link in it goes.
#[test]
fn a_folder_the_system_made_keeps_its_files_under_its_rules() {
    if shell().is_none() {
        return;
    }
    let found = look("$me");
    assert_eq!(found, ["settings kept True", "extra kept True", "link there False", "outside kept True"]);
}
