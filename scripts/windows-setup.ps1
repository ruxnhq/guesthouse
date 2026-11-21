# Builds Guesthouse's Windows installer, the one downloadable file guesthouse-setup-VERSION.exe, on
# Windows, in one command:
#   powershell -ExecutionPolicy Bypass -File scripts\windows-setup.ps1 [-Version 0.7.1] [-Out FOLDER]
#                                                                      [-App FOLDER] [-NoApp] [-Payload FOLDER]
# 1. builds the service, the serial relay and hcs-boot (cargo, release);
# 2. finds the app's files, unless -NoApp says not to: -App FOLDER if given, else what
#    crates\app-windows\stage.ps1 makes if that is there, else guesthouse-app.exe in target\release
#    with the Windows App Runtime beside it. With none of those the installer is built without the
#    app, and this says so;
# 3. gathers the files to install (the payload) in the out folder's payload\:
#      guesthouse-service.exe, guesthouse-serial-relay.exe (it has to be beside the service),
#      guesthouse-hcs-boot.exe, setup.ps1 (windows\installer), LICENSE, THIRD-PARTY.md,
#      and app\ (the app and the Windows App Runtime beside it);
# 4. builds the installer (crates\installer-windows) and packs the Windows App Runtime its pages
#    need and the payload into it (guesthouse-setup --pack): the runtime is the same as the
#    app's and is carried once.
# -Payload FOLDER packs that folder as the payload instead of steps 1 to 3 (it must have setup.ps1).
# docs/windows-host.md says how to try the result.
param([string]$Version = '', [string]$Out = '', [string]$App = '', [string]$Payload = '', [switch]$NoApp)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
if (-not $Out) { $Out = Join-Path $root 'target\installer' }
New-Item -ItemType Directory -Force $Out | Out-Null
if (-not $Version) {
    $line = Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
    $Version = $line.Matches[0].Groups[1].Value
}

# Runs a native program and stops if it fails.
function Invoke-Native([string]$What, [scriptblock]$Command) {
    & $Command
    if ($LASTEXITCODE -ne 0) { throw "$What failed ($LASTEXITCODE)" }
}

Push-Location $root
try {
    # The Windows App Runtime the pages (and the app) need: what windows-reactor-setup staged beside
    # the installer in target\release, the files its runtime.txt lists (not the WebView2 DLL it also
    # stages, which only Reactor's WebView2 control loads). Known once the installer is built, so
    # its build comes first: it also stages the runtime the app is given below.
    Invoke-Native 'the installer build' { cargo build --release --locked -p guesthouse-installer-windows }
    $meta = & cargo metadata --format-version 1 --locked | ConvertFrom-Json
    $setupCrate = $meta.packages | Where-Object { $_.name -eq 'windows-reactor-setup' } | Select-Object -First 1
    $list = Get-Content (Join-Path (Split-Path $setupCrate.manifest_path) 'assets\runtime.txt') | Where-Object { $_.Trim() }
    $release = Join-Path $meta.target_directory 'release'
    $runtime = Join-Path $Out 'runtime'
    if (Test-Path $runtime) { Remove-Item -Recurse -Force $runtime }
    New-Item -ItemType Directory -Force $runtime | Out-Null
    $staged = Get-ChildItem $release | Where-Object { $list -contains $_.Name.ToLowerInvariant() }
    if ($staged.Count -ne $list.Count) { throw "runtime.txt lists $($list.Count) entries, target\release has $($staged.Count) of them" }
    foreach ($entry in $staged) { Copy-Item -Recurse $entry.FullName $runtime }

    if (-not $Payload) {
        Invoke-Native 'the build' {
            cargo build --release --locked -p guesthouse-service -p guesthouse-host-windows -p guesthouse-hcs-boot
        }
        $appFolder = ''
        if ($NoApp) {
            Write-Output 'No app: -NoApp was given.'
        } elseif ($App) {
            if (-not (Test-Path (Join-Path $App 'guesthouse-app.exe'))) { throw "$App has no guesthouse-app.exe" }
            $appFolder = $App
        } elseif (Test-Path (Join-Path $root 'crates\app-windows\stage.ps1')) {
            $appFolder = Join-Path $Out 'app-folder'
            Invoke-Native "the app's stage.ps1" {
                powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root 'crates\app-windows\stage.ps1') -Out $appFolder
            }
        } elseif (Test-Path (Join-Path $release 'guesthouse-app.exe')) {
            $appFolder = Join-Path $Out 'app-folder'
            if (Test-Path $appFolder) { Remove-Item -Recurse -Force $appFolder }
            New-Item -ItemType Directory -Force $appFolder | Out-Null
            Copy-Item (Join-Path $release 'guesthouse-app.exe') $appFolder
            Copy-Item -Recurse (Join-Path $runtime '*') $appFolder
        }
        if ($appFolder) {
            Write-Output "The app comes from $appFolder."
        } else {
            Write-Warning ('No app files were found, so this installer has the service and the tools only. ' +
                'Build the app (crates\app-windows), then run this again, or give -App FOLDER, a folder with ' +
                'guesthouse-app.exe and the Windows App Runtime beside it.')
        }
        $Payload = Join-Path $Out 'payload'
        if (Test-Path $Payload) { Remove-Item -Recurse -Force $Payload }
        New-Item -ItemType Directory -Force $Payload | Out-Null
        foreach ($exe in 'guesthouse-service.exe', 'guesthouse-serial-relay.exe', 'guesthouse-hcs-boot.exe') {
            Copy-Item (Join-Path $release $exe) $Payload
        }
        Copy-Item (Join-Path $root 'windows\installer\setup.ps1'), (Join-Path $root 'LICENSE'), (Join-Path $root 'THIRD-PARTY.md') $Payload
        if ($appFolder) { Copy-Item -Recurse $appFolder (Join-Path $Payload 'app') }
    }
    if (-not (Test-Path (Join-Path $Payload 'setup.ps1'))) { throw "$Payload has no setup.ps1" }

    $setup = Join-Path $Out "guesthouse-setup-$Version.exe"
    $exe = Join-Path $release 'guesthouse-setup.exe'
    $report = Join-Path $Out 'pack.txt'
    # The installer is a windowed program: started this way so that it is waited for.
    $p = Start-Process -FilePath $exe -ArgumentList @('--pack', '--runtime', "`"$runtime`"", '--payload', "`"$Payload`"", '--version', $Version, '--out', "`"$setup`"") `
        -NoNewWindow -Wait -PassThru -RedirectStandardOutput $report
    Get-Content $report
    if ($p.ExitCode -ne 0) { throw "packing failed ($($p.ExitCode))" }
    Write-Output "Built $setup"
} finally { Pop-Location }
