# Copies the driver's files to where a normal install puts them, so programs find them by name. The
# app sends the list. A file already there stays unless this one is newer: the file version decides,
# then the date.
# Usage: nv-files.ps1 LIST [-StoreFolder NAME] [-Check]
# The files are taken from the driver's folder in the host driver store: NVIDIA's unless another is named.
# -StoreRoot is for the tests, which give a folder that stands in for the store.
# With -Check nothing is copied: it says how many files a run would copy or fail on, as "to place N".
param(
    [Parameter(Mandatory)][string]$List,
    [string]$StoreFolder = 'nv_dispi.inf_amd64_gpupv',
    [switch]$Check,
    [string]$StoreRoot = 'C:\Windows\System32\HostDriverStore\FileRepository'
)
$store = Join-Path $StoreRoot $StoreFolder

function Test-Newer($source, $target) {
    if (-not (Test-Path -LiteralPath $target)) { return $true }
    $new = Get-Item -LiteralPath $source
    $old = Get-Item -LiteralPath $target
    if ($target -match '\.(dll|exe)$') {
        $newVersion = $new.VersionInfo.FileVersionRaw
        $oldVersion = $old.VersionInfo.FileVersionRaw
        if ($newVersion -ne $oldVersion) { return $newVersion -gt $oldVersion }
    }
    return $new.LastWriteTimeUtc -gt $old.LastWriteTimeUtc
}

# A file a program has loaded can't be written over, but it can be renamed. So what is there is moved
# aside first, and put back if the copy fails. Copies moved aside earlier go once nothing has them loaded.
function Copy-Over($source, $target) {
    $old = (Split-Path -Leaf $target) + '.*.old'
    Get-ChildItem -LiteralPath (Split-Path $target) -Filter $old -ErrorAction SilentlyContinue |
        Remove-Item -Force -ErrorAction SilentlyContinue
    $aside = $null
    if (Test-Path -LiteralPath $target) {
        $aside = "$target.$([DateTime]::UtcNow.Ticks).old"
        Move-Item -LiteralPath $target -Destination $aside -Force -ErrorAction Stop
    }
    try {
        Copy-Item -LiteralPath $source -Destination $target -Force -ErrorAction Stop
    } catch {
        if ($aside) { Move-Item -LiteralPath $aside -Destination $target -Force -ErrorAction SilentlyContinue }
        throw
    }
}

$copied = 0
$kept = 0
$failed = 0
foreach ($line in Get-Content -LiteralPath $List) {
    $folder, $name, $target = $line -split "`t"
    $source = Join-Path $store $name
    $path = Join-Path (Join-Path $env:windir $folder) $target
    try {
        if ($Check) {
            if (Test-Newer $source $path) { $copied++ }
        } elseif (Test-Newer $source $path) {
            Copy-Over $source $path
            $copied++
            "copied $folder\$target from $name"
        } else {
            $kept++
            "kept $folder\$target"
        }
    } catch {
        if ($Check) {
            $copied++
        } else {
            $failed++
            "FAILED $folder\$target from ${name}: $($_.Exception.Message)"
        }
    }
}
if ($Check) { "to place $copied" } else { "copied $copied, kept $kept, failed $failed" }
