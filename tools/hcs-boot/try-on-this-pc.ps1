# Runs every step of Guesthouse on a Windows host once, in order, and writes all of it to one log:
# the checks, a Windows guest made and booted, and an Ubuntu guest made, installed and booted. Each
# boot has a time limit, and a step that fails doesn't stop the next. It needs an elevated prompt.
#
# Usage, from the repo, in PowerShell run as administrator:
#   .\tools\hcs-boot\try-on-this-pc.ps1 -Folder D:\vms -WindowsIso D:\iso\Win11.iso -UbuntuIso D:\iso\ubuntu.iso
# Either ISO can be left out to try only the other guest. Disks already in the folder are booted
# again rather than made again.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Folder,
    [string]$WindowsIso,
    [string]$UbuntuIso,
    # How long each guest runs before it's asked to shut down.
    [int]$Minutes = 20,
    # How long Ubuntu's installer may take.
    [int]$InstallMinutes = 90
)

$ErrorActionPreference = 'Continue'
$principal = New-Object -TypeName Security.Principal.WindowsPrincipal -ArgumentList ([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Error 'This needs an elevated prompt: open PowerShell with Run as administrator.'
    exit 1
}

New-Item -ItemType Directory -Force -Path $Folder | Out-Null
$log = Join-Path $Folder 'guesthouse-try.log'
$repo = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$program = Join-Path $repo 'target\release\guesthouse-hcs-boot.exe'

function Write-Log([string]$Text) {
    $line = "[$(Get-Date -Format 'HH:mm:ss')] $Text"
    # To the screen, not the output, which is how Invoke-Step gives back its exit code.
    $line | Out-Host
    Add-Content -LiteralPath $log -Value $line -Encoding utf8
}

# Runs a program, shows and logs each line it prints, logs how it ended, and gives its exit code.
function Invoke-Step([string]$Name, [string]$File, [string[]]$Arguments) {
    Write-Log "=== $Name"
    & $File @Arguments 2>&1 | ForEach-Object {
        $text = "$_"
        Write-Output $text
        Add-Content -LiteralPath $log -Value $text -Encoding utf8
    } | Out-Host
    $code = $LASTEXITCODE
    Write-Log "=== $Name ended with exit code $code"
    return $code
}

# Boots a guest. A boot that fails at once, before its machine could run, is tried again with less:
# no network, then no TPM as well, as either is a part that has never run on this PC before.
function Invoke-Boot([string]$Name, [string[]]$Arguments) {
    foreach ($less in @(@(), @('--no-network'), @('--no-network', '--no-tpm'))) {
        $started = Get-Date
        $code = Invoke-Step "$Name $($less -join ' ')" $program ($Arguments + $jail + $less)
        if ($code -eq 0 -or ((Get-Date) - $started).TotalSeconds -gt 120) { return }
        Write-Log "$Name failed at once, so it's tried again with less."
    }
}

Write-Log "Guesthouse on this PC, logging to $log"
Write-Log "Windows $([Environment]::OSVersion.Version)"
# DISM itself, as Get-WindowsOptionalFeature takes minutes and says nothing under PowerShell 7.
foreach ($feature in 'VirtualMachinePlatform', 'Microsoft-Hyper-V-All') {
    $state = & dism.exe /English /Online /Get-FeatureInfo "/FeatureName:$feature" 2>&1 |
        Select-String -Pattern '^State : (.+)$' | ForEach-Object { $_.Matches[0].Groups[1].Value }
    Write-Log "Feature ${feature}: $(if ($state) { $state } else { 'not known' })"
}
foreach ($card in Get-CimInstance -ClassName Win32_VideoController) {
    Write-Log "Card: $($card.Name), driver $($card.DriverVersion)"
}

Push-Location $repo
$null = Invoke-Step 'Build' 'cargo' @('build', '--release', '-p', 'guesthouse-hcs-boot')
# The jail's tests have passed unelevated. boot runs elevated, so they run that way too, and if they
# fail the serial port is read without the jail, rather than left unread.
$jailTest = Invoke-Step 'Jail, elevated' 'cargo' @('test', '--release', '-p', 'guesthouse-jail', '--test', 'kernel')
$relayTest = Invoke-Step 'Serial relay in its jail, elevated' 'cargo' @('test', '--release', '-p', 'guesthouse-hcs-boot', '--test', 'relay')
Pop-Location
$jail = @()
if ($jailTest -ne 0 -or $relayTest -ne 0) {
    Write-Log '!!! The jail failed elevated, so the boots below read the serial port with --no-jail.'
    $jail = @('--no-jail')
}
if (-not (Test-Path -LiteralPath $program)) {
    Write-Log "There's no $program, so nothing more can run."
    exit 1
}
$null = Invoke-Step 'Cards' $program @('gpus')

if ($WindowsIso) {
    $disk = Join-Path $Folder 'windows.vhdx'
    if (-not (Test-Path -LiteralPath $disk)) {
        $null = Invoke-Step 'Windows: prepare' $program @('prepare', '--image', $WindowsIso, '--disk', $disk)
    }
    if (Test-Path -LiteralPath $disk) {
        Invoke-Boot 'Windows: boot' @('boot', '--disk', $disk, '--name', 'guesthouse-windows', '--for-minutes', "$Minutes")
    }
}

if ($UbuntuIso) {
    $disk = Join-Path $Folder 'ubuntu.vhdx'
    if (-not (Test-Path -LiteralPath $disk)) {
        $null = Invoke-Step 'Ubuntu: prepare' $program @('prepare', '--image', $UbuntuIso, '--disk', $disk)
    }
    # The installer needs the network for its updates, so its boot is never tried without one.
    $record = Join-Path $Folder 'ubuntu.guesthouse.json'
    if ((Test-Path -LiteralPath $record) -and -not (Get-Content -LiteralPath $record -Raw | ConvertFrom-Json).installed) {
        $null = Invoke-Step 'Ubuntu: install' $program (@('boot', '--disk', $disk, '--name', 'guesthouse-ubuntu', '--for-minutes', "$InstallMinutes") + $jail)
    }
    if ((Test-Path -LiteralPath $record) -and (Get-Content -LiteralPath $record -Raw | ConvertFrom-Json).installed) {
        $null = Invoke-Step 'Ubuntu: boot' $program (@('boot', '--disk', $disk, '--name', 'guesthouse-ubuntu', '--for-minutes', "$Minutes") + $jail)
    }
}

Write-Log "Done. Everything above is in $log"
