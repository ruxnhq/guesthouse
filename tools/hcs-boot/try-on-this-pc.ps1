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
    Write-Output $line
    Add-Content -LiteralPath $log -Value $line -Encoding utf8
}

# Runs a program, shows and logs each line it prints, and logs how it ended.
function Invoke-Step([string]$Name, [string]$File, [string[]]$Arguments) {
    Write-Log "=== $Name"
    & $File @Arguments 2>&1 | ForEach-Object {
        $text = "$_"
        Write-Output $text
        Add-Content -LiteralPath $log -Value $text -Encoding utf8
    }
    Write-Log "=== $Name ended with exit code $LASTEXITCODE"
}

Write-Log "Guesthouse on this PC, logging to $log"
Write-Log "Windows $([Environment]::OSVersion.Version)"
foreach ($feature in 'VirtualMachinePlatform', 'Microsoft-Hyper-V-All') {
    $state = (Get-WindowsOptionalFeature -Online -FeatureName $feature -ErrorAction SilentlyContinue).State
    Write-Log "Feature ${feature}: $state"
}
foreach ($card in Get-CimInstance -ClassName Win32_VideoController) {
    Write-Log "Card: $($card.Name), driver $($card.DriverVersion)"
}

Push-Location $repo
Invoke-Step 'Build' 'cargo' @('build', '--release', '-p', 'guesthouse-hcs-boot')
Pop-Location
if (-not (Test-Path -LiteralPath $program)) {
    Write-Log "There's no $program, so nothing more can run."
    exit 1
}
Invoke-Step 'Cards' $program @('gpus')

if ($WindowsIso) {
    $disk = Join-Path $Folder 'windows.vhdx'
    if (-not (Test-Path -LiteralPath $disk)) {
        Invoke-Step 'Windows: prepare' $program @('prepare', '--image', $WindowsIso, '--disk', $disk)
    }
    if (Test-Path -LiteralPath $disk) {
        Invoke-Step 'Windows: boot' $program @('boot', '--disk', $disk, '--name', 'guesthouse-windows', '--for-minutes', "$Minutes")
    }
}

if ($UbuntuIso) {
    $disk = Join-Path $Folder 'ubuntu.vhdx'
    if (-not (Test-Path -LiteralPath $disk)) {
        Invoke-Step 'Ubuntu: prepare' $program @('prepare', '--image', $UbuntuIso, '--disk', $disk)
    }
    $record = Join-Path $Folder 'ubuntu.guesthouse.json'
    if ((Test-Path -LiteralPath $record) -and -not (Get-Content -LiteralPath $record -Raw | ConvertFrom-Json).installed) {
        Invoke-Step 'Ubuntu: install' $program @('boot', '--disk', $disk, '--name', 'guesthouse-ubuntu', '--for-minutes', "$InstallMinutes")
    }
    if ((Test-Path -LiteralPath $record) -and (Get-Content -LiteralPath $record -Raw | ConvertFrom-Json).installed) {
        Invoke-Step 'Ubuntu: boot' $program @('boot', '--disk', $disk, '--name', 'guesthouse-ubuntu', '--for-minutes', "$Minutes")
    }
}

Write-Log "Done. Everything above is in $log"
