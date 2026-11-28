# Sent by the host through the agent at every start, and run by setup.ps1 from the disc as Windows
# sets itself up, so a guest made before this has it from its next start. SYSTEM runs what is in
# C:\gpupv, so only SYSTEM and administrators may change anything there. Everyone may read it, but
# the token, which is for SYSTEM and administrators alone: a helper's start lets the account it runs
# as read the token too. A script in the user's session writes only to a file made for it, given to
# that account alone, here or by the host as it runs the script. The host puts the account in, and
# puts logon-secret.ps1 before this, whose step takes the sign-in password out of the registry.
$account = '@USER@'

# SYSTEM is S-1-5-18, the administrators' group S-1-5-32-544 and the users' group S-1-5-32-545, the
# same in every language.
function Get-Sid([string]$Sid) { New-Object System.Security.Principal.SecurityIdentifier $Sid }

function Get-Rule($Who, [string]$Rights, [string]$Inherit = 'None') {
    New-Object System.Security.AccessControl.FileSystemAccessRule $Who, $Rights, $Inherit, 'None', 'Allow'
}

function Test-Link($Item) { [bool]($Item.Attributes -band [IO.FileAttributes]::ReparsePoint) }

# Takes away the link itself, never what it leads to.
function Remove-Link($Item) {
    if ($Item.PSIsContainer) { [IO.Directory]::Delete($Item.FullName) } else { [IO.File]::Delete($Item.FullName) }
}

# The folder's own rules, which everything in it takes: SYSTEM and administrators may do anything,
# users may read and run. Nothing above it counts any more. The owner is the administrators' group in
# a guest, and a test's own account in a test.
function Set-FolderRules([string]$Folder, $Owner) {
    $all = 'ContainerInherit, ObjectInherit'
    $rules = Get-Acl -LiteralPath $Folder
    $rules.SetOwner($Owner)
    $rules.SetAccessRuleProtection($true, $false)
    foreach ($rule in @($rules.GetAccessRules($true, $false, [System.Security.Principal.SecurityIdentifier]))) {
        [void]$rules.RemoveAccessRuleSpecific($rule)
    }
    $rules.AddAccessRule((Get-Rule (Get-Sid 'S-1-5-18') 'FullControl' $all))
    $rules.AddAccessRule((Get-Rule (Get-Sid 'S-1-5-32-544') 'FullControl' $all))
    $rules.AddAccessRule((Get-Rule $Owner 'FullControl' $all))
    $rules.AddAccessRule((Get-Rule (Get-Sid 'S-1-5-32-545') 'ReadAndExecute' $all))
    Set-Acl -LiteralPath $Folder -AclObject $rules
}

# Whether an item inside has only what the folder gives it, and an owner that can't give more.
function Test-Inherits($Rules, $Owner) {
    $ownedBy = $Rules.GetOwner([System.Security.Principal.SecurityIdentifier])
    $own = @($Rules.GetAccessRules($true, $false, [System.Security.Principal.SecurityIdentifier]))
    -not $Rules.AreAccessRulesProtected -and $own.Count -eq 0 -and
        ($ownedBy -eq $Owner -or $ownedBy.Value -eq 'S-1-5-18')
}

# Everything inside, one level at a time and into no link: a link is taken away, as is a second
# name for a file, and anything else gets only what the folder gives it, with the folder's owner.
# With -LinksOnly it only takes links and second names away, which goes first: Windows carries the
# folder's new rules down to what is in it, and so to a file outside through a second name.
function Repair-Inside([string]$Folder, $Owner, [switch]$LinksOnly) {
    foreach ($item in @(Get-ChildItem -LiteralPath $Folder -Force)) {
        if (Test-Link $item) {
            Remove-Link $item
            "took away the link $($item.FullName)"
            continue
        }
        if (-not $item.PSIsContainer -and $item.LinkType -eq 'HardLink') {
            [IO.File]::Delete($item.FullName)
            "took away the second name $($item.FullName)"
            continue
        }
        if ($LinksOnly) {
            if ($item.PSIsContainer) { Repair-Inside $item.FullName $Owner -LinksOnly }
            continue
        }
        try {
            $rules = Get-Acl -LiteralPath $item.FullName
            if (-not (Test-Inherits $rules $Owner)) {
                $rules.SetOwner($Owner)
                foreach ($rule in @($rules.GetAccessRules($true, $false, [System.Security.Principal.SecurityIdentifier]))) {
                    [void]$rules.RemoveAccessRuleSpecific($rule)
                }
                $rules.SetAccessRuleProtection($false, $true)
                Set-Acl -LiteralPath $item.FullName -AclObject $rules
            }
        } catch {
            if ($item.PSIsContainer) {
                "the rules of $($item.FullName) could not be changed: $($_.Exception.Message)"
                continue
            }
            # A file whose rules can't be put right goes: SYSTEM and administrators may delete in here.
            [IO.File]::Delete($item.FullName)
            "took away $($item.FullName), whose rules could not be changed"
            continue
        }
        if ($item.PSIsContainer) { Repair-Inside $item.FullName $Owner }
    }
}

# The token: SYSTEM and administrators alone, until a helper's start adds the account it runs as.
function Set-TokenRules([string]$File, $Owner) {
    if (-not (Test-Path -LiteralPath $File -PathType Leaf)) { return }
    $rules = Get-Acl -LiteralPath $File
    $rules.SetOwner($Owner)
    $rules.SetAccessRuleProtection($true, $false)
    foreach ($rule in @($rules.GetAccessRules($true, $false, [System.Security.Principal.SecurityIdentifier]))) {
        [void]$rules.RemoveAccessRuleSpecific($rule)
    }
    $rules.AddAccessRule((Get-Rule (Get-Sid 'S-1-5-18') 'FullControl'))
    $rules.AddAccessRule((Get-Rule (Get-Sid 'S-1-5-32-544') 'FullControl'))
    $rules.AddAccessRule((Get-Rule $Owner 'FullControl'))
    Set-Acl -LiteralPath $File -AclObject $rules
}

# The guest's folder as SYSTEM keeps it.
function Set-GuestFolder([string]$Folder, [string]$Owner = 'S-1-5-32-544') {
    $ownerSid = Get-Sid $Owner
    $found = Get-Item -LiteralPath $Folder -Force -ErrorAction SilentlyContinue
    if ($found -and (Test-Link $found)) {
        Remove-Link $found
        "took away the link $Folder"
    }
    New-Item -ItemType Directory -Force -Path $Folder | Out-Null
    Repair-Inside $Folder $ownerSid -LinksOnly
    Set-FolderRules $Folder $ownerSid
    Repair-Inside $Folder $ownerSid
    Set-TokenRules (Join-Path $Folder 'token.txt') $ownerSid
}

# A file a script in the user's session writes to, which SYSTEM makes, and which only that account
# may also read and write. It can't take the file away or make another in its place.
function Grant-SessionFile([string]$File, [string]$Account) {
    if (-not (Test-Path -LiteralPath $File -PathType Leaf)) { [IO.File]::WriteAllText($File, '') }
    $rules = Get-Acl -LiteralPath $File
    $rules.AddAccessRule((Get-Rule (New-Object System.Security.Principal.NTAccount $Account) 'Read, Write'))
    Set-Acl -LiteralPath $File -AclObject $rules
}

# Windows' copy of the answer file it was set up with, which has the account's password in it. It is
# only needed until setup has finished.
function Remove-AnswerFile([string]$File, [string]$SetupKey) {
    $setup = Get-ItemProperty -LiteralPath $SetupKey -ErrorAction SilentlyContinue
    if (-not $setup -or $setup.SystemSetupInProgress -ne 0 -or $setup.OOBEInProgress -ne 0) { return }
    if (Test-Path -LiteralPath $File -PathType Leaf) {
        Remove-Item -LiteralPath $File -Force
        "took away $File"
    }
}

try { Set-GuestFolder 'C:\gpupv'; 'C:\gpupv: SYSTEM and administrators only' } catch { "C:\gpupv: $($_.Exception.Message)" }
try {
    New-Item -ItemType Directory -Force -Path 'C:\gpupv\cap' | Out-Null
    Grant-SessionFile 'C:\gpupv\cap\setmode.txt' $account
} catch { "setmode.txt: $($_.Exception.Message)" }
try { Remove-AnswerFile 'C:\Windows\Panther\unattend.xml' 'HKLM:\SYSTEM\Setup' } catch { "answer file: $($_.Exception.Message)" }
try { "sign-in password: $(Move-LeftPassword 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon')" } catch { "sign-in password: $($_.Exception.Message)" }
