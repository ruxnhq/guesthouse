# Runs a recipe's steps in the guest, as the account that is logged in, with the rights of its
# administrator. It says how each went in status.txt, a line at a time with a word first, for the host
# to read: begin, waiting winget, start N, ok N, fail N and why, and end. Everything the recipe gave is
# in data.json and is only read as data here: a package id is one argument to winget, and a script is
# a file of its own that PowerShell runs.
$folder = 'C:\gpupv\recipe'
$status = Join-Path $folder 'status.txt'
$wordsLongest = 300
$idShape = '\A[A-Za-z0-9][A-Za-z0-9._+-]{1,127}\z'
# winget is registered for a user when App Installer first runs for them, which a new Windows 11 can
# be slow to do.
$appInstaller = 'Microsoft.DesktopAppInstaller_8wekyb3d8bbwe'

function Say([string]$line) { Add-Content -LiteralPath $status -Value $line -Encoding ASCII }

# The end of what a program said, on one line and in plain characters, as that is what the host shows.
function Tail([string]$text) {
    $one = ($text -replace '[^\x20-\x7e]+', ' ' -replace '\s+', ' ').Trim()
    if ($one.Length -gt $wordsLongest) { $one = $one.Substring($one.Length - $wordsLongest) }
    $one
}

function Wait-Winget([int]$seconds) {
    $end = (Get-Date).AddSeconds($seconds)
    $registered = $false
    while ($true) {
        $found = Get-Command winget.exe -ErrorAction SilentlyContinue
        if ($found) { return $found.Source }
        if ((Get-Date) -ge $end) { return $null }
        if (-not $registered) {
            $registered = $true
            try { Add-AppxPackage -RegisterByFamilyName -MainPackage $appInstaller -ErrorAction Stop } catch { }
        }
        Start-Sleep -Seconds 10
    }
}

# Says why a package did not install, or nothing when it is there.
function Install-Package([string]$winget, [string]$id) {
    if ($id -notmatch $idShape) { return 'That is not a package id.' }
    & $winget list --id $id --exact --accept-source-agreements --disable-interactivity *> $null
    if ($LASTEXITCODE -eq 0) { return $null }
    $said = & $winget install --id $id --exact --silent --accept-package-agreements --accept-source-agreements --disable-interactivity 2>&1 | Out-String
    if ($LASTEXITCODE -eq 0) { return $null }
    "winget stopped with $LASTEXITCODE. " + (Tail $said)
}

function Invoke-Script([int]$number, [string]$text) {
    $file = Join-Path $folder "step-$number.ps1"
    # A byte order mark, as Windows PowerShell reads a file without one in the system's code page.
    [IO.File]::WriteAllText($file, $text, [Text.UTF8Encoding]::new($true))
    $said = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $file 2>&1 | Out-String
    if ($LASTEXITCODE -eq 0) { return $null }
    "The script stopped with $LASTEXITCODE. " + (Tail $said)
}

# Which process this run is, so that running the steps again ends this run and what it started.
Set-Content -LiteralPath (Join-Path $folder 'run.pid') -Value $PID -Encoding ASCII

$current = 0
try {
    $job = Get-Content -LiteralPath (Join-Path $folder 'data.json') -Raw -Encoding UTF8 | ConvertFrom-Json
    Say 'begin'
    $winget = $null
    $needsWinget = @($job.steps | Where-Object { $_.kind -eq 'package' }).Count -gt 0
    if ($needsWinget) {
        Say 'waiting winget'
        $winget = Wait-Winget ([int]$job.winget_seconds)
    }
    foreach ($step in $job.steps) {
        $current = [int]$step.number
        Say "start $current"
        if ($step.kind -eq 'package') {
            if (-not $winget) {
                $why = "winget is not in this Windows yet. App Installer has not registered itself, so there is nothing to install with. Run the recipe again in a few minutes."
            } else {
                $why = Install-Package $winget ([string]$step.value)
            }
        } else {
            $why = Invoke-Script $current ([string]$step.value)
        }
        if ($why) { Say "fail $current $why"; break }
        Say "ok $current"
    }
} catch {
    Say ("fail $current " + (Tail "$_"))
}
Say 'end'
