# Lean Windows, from the host's list: the apps Windows adds for every account are taken out by their
# package names, and a few documented policies are set. Setup runs it from the answer disc in its
# specialize pass, before any account exists, and the host runs it again through the agent once the
# install is up, for what reached an account anyway. Both times it runs as SYSTEM. The list is data,
# and each item is checked again here. It says how it went in the status file, a line at a time: begin,
# then a line for each thing done or kept, then end and how many things it could not do.
param([string]$Data, [string]$Status = 'C:\gpupv\lean-setup.log')
$ErrorActionPreference = 'Continue'
$nameShape = '\A[A-Za-z0-9][A-Za-z0-9.]{1,127}\z'
$keyShape = '\AMicrosoft(\\[A-Za-z0-9]+){1,4}\z'
$valueShape = '\A[A-Za-z0-9]{1,64}\z'
$policyRoot = 'HKLM:\SOFTWARE\Policies'

function Say([string]$line) { Add-Content -LiteralPath $Status -Value $line -Encoding ASCII }

New-Item -ItemType Directory -Force (Split-Path -Parent $Status) | Out-Null
$failed = 0
try {
    $list = Get-Content -LiteralPath $Data -Raw -Encoding UTF8 | ConvertFrom-Json
    Say 'begin'
    $names = @($list.apps | ForEach-Object { "$_" } | Where-Object { $_ -match $nameShape })
    # What Windows installs for each new account. A provisioned package's DisplayName is its package
    # name, such as Microsoft.BingNews, and not words in the system's language. Names are compared
    # whole, never as a pattern, so nothing off the list can match.
    foreach ($package in @(Get-AppxProvisionedPackage -Online)) {
        if ($names -notcontains $package.DisplayName) { continue }
        try {
            Remove-AppxProvisionedPackage -Online -PackageName $package.PackageName -ErrorAction Stop | Out-Null
            Say "deprovisioned $($package.DisplayName)"
        } catch {
            $failed++
            Say "kept $($package.DisplayName) $($_.Exception.Message)"
        }
    }
    # What an account already has. -Name would take a pattern, so every package is read and compared.
    # Before the first logon there are none.
    $installed = @()
    try { $installed = @(Get-AppxPackage -AllUsers -ErrorAction Stop) } catch { Say "no accounts yet $($_.Exception.Message)" }
    foreach ($package in $installed) {
        if ($names -notcontains $package.Name) { continue }
        try {
            Remove-AppxPackage -Package $package.PackageFullName -AllUsers -ErrorAction Stop
            Say "removed $($package.Name)"
        } catch {
            $failed++
            Say "kept $($package.Name) $($_.Exception.Message)"
        }
    }
    foreach ($policy in @($list.policies)) {
        $key = "$($policy.key)"
        $value = "$($policy.value)"
        if ($key -notmatch $keyShape -or $value -notmatch $valueShape) {
            $failed++
            Say 'kept a policy that is not one'
            continue
        }
        $path = Join-Path $policyRoot $key
        # New-Item -Force on a key that is there makes it again, empty, so it is only made when missing.
        if (-not (Test-Path -LiteralPath $path)) { New-Item -Path $path -Force | Out-Null }
        New-ItemProperty -LiteralPath $path -Name $value -PropertyType DWord -Value ([int]$policy.data) -Force | Out-Null
        Say "set $key\$value"
    }
} catch {
    $failed++
    Say "kept the rest $($_.Exception.Message)"
}
Say "end $failed"
