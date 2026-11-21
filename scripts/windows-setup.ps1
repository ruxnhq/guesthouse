# Builds Guesthouse's Windows installer, the one downloadable file guesthouse-setup-VERSION.exe, on
# Windows, in one command:
#   powershell -ExecutionPolicy Bypass -File scripts\windows-setup.ps1 [-Version 0.7.1] [-Out FOLDER]
#                                                                      [-App FOLDER] [-NoApp] [-Payload FOLDER]
#                                                                      [-RemotemanFrom FOLDER] [-RemotemanUnpinned]
#                                                                      [-NoScreen]
# 1. builds the service, the serial relay and hcs-boot (cargo, release);
# 2. finds the app's files, unless -NoApp says not to: -App FOLDER if given, else what
#    crates\app-windows\stage.ps1 makes if that is there, else guesthouse-app.exe in target\release
#    with the Windows App Runtime beside it. With none of those the installer is built without the
#    app, and this says so;
# 3. gets remoteman, for a Windows guest's screen, unless -NoScreen says not to: the two release
#    files windows\remoteman.json pins, remoteman's host's installer (remoteman-host-setup-V.exe)
#    and its Windows viewer (remoteman-viewer-windows-V.zip), downloaded from the pin's base into
#    target\remoteman-cache\V\ (a cached file is used when its SHA-256 is the pin's, and downloaded
#    again when it isn't), or taken from -RemotemanFrom FOLDER. Each file's SHA-256 has to be the
#    pin's, or the build stops; -RemotemanUnpinned skips that check, for trying a local build of
#    remoteman only, and says so at the start and the end (with it, -RemotemanFrom's files may have
#    another version in their names). With no network and nothing cached the build stops: it never
#    builds without the screen unless -NoScreen asks;
# 4. gathers the files to install (the payload) in the out folder's payload\:
#      guesthouse-service.exe, guesthouse-serial-relay.exe (it has to be beside the service),
#      guesthouse-hcs-boot.exe, setup.ps1 (windows\installer), LICENSE, THIRD-PARTY.md (with the
#      remoteman files this installer carries added at its end), GPL-3.0.txt (the GPL 3 that
#      FFmpeg's LGPL 3 builds on), app\ (the app and the Windows App Runtime beside it),
#      screen\remoteman-host-setup.exe (as it is: the service runs it in each Windows guest), and
#      viewer\ (the viewer's zip unpacked, remoteman.exe in it: the app starts that);
# 5. builds the installer (crates\installer-windows) and packs the Windows App Runtime its pages
#    need and the payload into it (guesthouse-setup --pack): the runtime is the same as the
#    app's and is carried once. It's the out folder's guesthouse-setup-VERSION.exe (VERSION is
#    the workspace's, from Cargo.toml, unless -Version says otherwise), with
#    guesthouse-setup-VERSION.exe.sha256 beside it ("<hex>  <file>"): the two files to publish.
# -Payload FOLDER packs that folder as the payload instead of steps 1 to 4 (it must have setup.ps1).
# docs/windows-host.md says how to try the result.
param([string]$Version = '', [string]$Out = '', [string]$App = '', [string]$Payload = '', [switch]$NoApp,
      [string]$RemotemanFrom = '', [switch]$RemotemanUnpinned, [switch]$NoScreen)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
if (-not $Out) { $Out = Join-Path $root 'target\installer' }
New-Item -ItemType Directory -Force $Out | Out-Null
if (-not $Version) {
    $line = Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
    $Version = $line.Matches[0].Groups[1].Value
}
$unpinnedWarning = "-RemotemanUnpinned: remoteman's files are NOT checked against windows\remoteman.json. Only for trying a local build of remoteman; never hand this installer to anyone."
if ($RemotemanUnpinned -and -not $NoScreen) { Write-Warning $unpinnedWarning }

# Runs a native program and stops if it fails.
function Invoke-Native([string]$What, [scriptblock]$Command) {
    & $Command
    if ($LASTEXITCODE -ne 0) { throw "$What failed ($LASTEXITCODE)" }
}

function Get-Sha256([string]$Path) { (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant() }

# windows\remoteman.json: the version of remoteman this installer carries, where its release files
# are, and each file's name and SHA-256.
function Read-Pin {
    $file = Join-Path $root 'windows\remoteman.json'
    $pin = Get-Content -Raw $file | ConvertFrom-Json
    foreach ($key in 'host_setup', 'viewer_zip') {
        $entry = $pin.$key
        if (-not $entry -or -not $entry.file) { throw "$file has no $key.file" }
        if (-not $RemotemanUnpinned -and $entry.sha256 -notmatch '^[0-9a-fA-F]{64}$') {
            throw "$file pins no SHA-256 for $($entry.file) (it says '$($entry.sha256)'). Fill in remoteman's release there, or give -NoScreen."
        }
    }
    if (-not $pin.version) { throw "$file has no version" }
    if (-not $pin.base) { throw "$file has no base" }
    $pin
}

# One of remoteman's release files, checked against the pin: from -RemotemanFrom, or from the cache,
# or downloaded into it. Gives its path.
function Get-RemotemanFile($Pin, $Entry, [string]$Pattern) {
    $want = $Entry.sha256.ToLowerInvariant()
    if ($RemotemanFrom) {
        $path = Join-Path $RemotemanFrom $Entry.file
        if (-not (Test-Path -LiteralPath $path) -and $RemotemanUnpinned) {
            # A local build may carry another version in its name.
            $found = @(Get-ChildItem -LiteralPath $RemotemanFrom -Filter $Pattern | Sort-Object LastWriteTime -Descending)
            if ($found.Count -gt 0) { $path = $found[0].FullName }
        }
        if (-not (Test-Path -LiteralPath $path)) { throw "$RemotemanFrom has no $($Entry.file)" }
        if ($RemotemanUnpinned) {
            Write-Warning "$path is used unchecked (SHA-256 $(Get-Sha256 $path))."
        } elseif ((Get-Sha256 $path) -ne $want) {
            throw "$path has SHA-256 $(Get-Sha256 $path), and windows\remoteman.json pins $want. Refused."
        }
        return $path
    }
    $cache = Join-Path $root "target\remoteman-cache\$($Pin.version)"
    New-Item -ItemType Directory -Force $cache | Out-Null
    $path = Join-Path $cache $Entry.file
    if (Test-Path -LiteralPath $path) {
        if ($RemotemanUnpinned -or (Get-Sha256 $path) -eq $want) {
            Write-Host "remoteman: $($Entry.file) from the cache, $cache."
            return $path
        }
        Write-Warning "The cached $path isn't what windows\remoteman.json pins; downloading it again."
        Remove-Item -LiteralPath $path -Force
    }
    $url = $Pin.base.TrimEnd('/') + '/' + $Entry.file
    $part = "$path.part"
    Write-Host "remoteman: downloading $url"
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
        $ProgressPreference = 'SilentlyContinue'
        Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $part
    } catch {
        Remove-Item -LiteralPath $part -Force -ErrorAction SilentlyContinue
        throw ("remoteman's $($Entry.file) couldn't be downloaded from $url, and isn't in $cache " +
            "($($_.Exception.Message)). Connect to the network and run this again, or give -RemotemanFrom FOLDER " +
            'with the two files, or give -NoScreen to build an installer without a Windows guest''s screen.')
    }
    $got = Get-Sha256 $part
    if (-not $RemotemanUnpinned -and $got -ne $want) {
        Remove-Item -LiteralPath $part -Force
        throw "$url has SHA-256 $got, and windows\remoteman.json pins $want. Refused: nothing of it is kept."
    }
    Move-Item -LiteralPath $part $path -Force
    $path
}

# remoteman's two files into the payload: the host's installer as it is, the viewer's zip unpacked.
function Add-Remoteman($Pin, [string]$To) {
    $setupExe = Get-RemotemanFile $Pin $Pin.host_setup 'remoteman-host-setup-*.exe'
    $zip = Get-RemotemanFile $Pin $Pin.viewer_zip 'remoteman-viewer-windows-*.zip'
    $screen = Join-Path $To 'screen'
    New-Item -ItemType Directory -Force $screen | Out-Null
    Copy-Item -LiteralPath $setupExe (Join-Path $screen 'remoteman-host-setup.exe')

    $viewer = Join-Path $To 'viewer'
    $unpacked = Join-Path $Out 'viewer-unpacked'
    if (Test-Path $unpacked) { Remove-Item -Recurse -Force $unpacked }
    Expand-Archive -LiteralPath $zip -DestinationPath $unpacked
    # The zip's files, or the one folder it holds them in.
    $top = $unpacked
    $inside = @(Get-ChildItem -LiteralPath $unpacked)
    if ($inside.Count -eq 1 -and $inside[0].PSIsContainer) { $top = $inside[0].FullName }
    if (-not (Test-Path (Join-Path $top 'remoteman.exe'))) { throw "$zip has no remoteman.exe" }
    New-Item -ItemType Directory -Force $viewer | Out-Null
    Copy-Item -Recurse (Join-Path $top '*') $viewer
    # Its licences go with it: remoteman's, and FFmpeg's.
    $notices = @(Get-ChildItem -LiteralPath $viewer -File | Where-Object { $_.Name -match 'licen[cs]e|copying|notice|lgpl' })
    if ($notices.Count -eq 0) { throw "$zip has no licence files beside remoteman.exe (remoteman's LICENSE and FFmpeg's LGPL); it can't be shipped like that." }
    Write-Output "remoteman's viewer carries: $(($notices | ForEach-Object Name) -join ', ')."

    # The installer's THIRD-PARTY.md says which remoteman it carries.
    $which = if ($RemotemanUnpinned) { 'remoteman (a trial build: NOT checked against windows\remoteman.json)' } else { "remoteman $($Pin.version)" }
    $lines = @(
        '',
        '## remoteman in this installer',
        '',
        "This installer carries $which, under the MIT licence: ``screen\remoteman-host-setup.exe`` (from $(Split-Path -Leaf $setupExe)) and ``viewer\`` (from $(Split-Path -Leaf $zip)). remoteman's licence and FFmpeg's, with where FFmpeg's source is, are in ``viewer\`` beside ``remoteman.exe``: $(($notices | ForEach-Object Name) -join ', ')."
    )
    Add-Content -Path (Join-Path $To 'THIRD-PARTY.md') -Value $lines -Encoding UTF8
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
        # The pin is read first: a pin with no hashes stops the build before anything is built.
        $pin = $null
        if ($NoScreen) { Write-Output 'No screen: -NoScreen was given.' } else { $pin = Read-Pin }

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
        if ($pin) {
            Copy-Item (Join-Path $root 'windows\installer\GPL-3.0.txt') $Payload
            Add-Remoteman $pin $Payload
        }
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
    # Its SHA-256 beside it, as sha256sum writes one ("<hex>  <file>"), for whoever downloads it.
    $sum = "$setup.sha256"
    [IO.File]::WriteAllText($sum, "$(Get-Sha256 $setup)  $(Split-Path -Leaf $setup)`n", (New-Object Text.ASCIIEncoding))
    Write-Output "Built $setup"
    Write-Output "  and $sum"
    if ($RemotemanUnpinned -and -not $NoScreen) { Write-Warning $unpinnedWarning }
} finally { Pop-Location }
