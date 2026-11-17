# Sent by the host through the agent once the desktop is up, at every start, with $on set above it.
# Windows shows a Direct3D program that draws in a window the old way through a copy of every frame,
# and moves it to the way without the copy only when it counts the program as a game. One entry here
# makes it do that for every program under a folder. The folder has to be named: an entry for a whole
# drive matches nothing. So each folder at the top of a fixed drive gets one, except Windows' own, and
# a folder made later gets its entry at the next start. A program reads this when it starts.
# A 32-bit program reads the second key. Only the entries named as ours are ever written or removed,
# so with the setting off nothing of this is left.
$keys = 'HKLM:\SOFTWARE\Microsoft\Direct3D', 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Direct3D'
$ours = 'guesthouse-window-speed-'
$behaviour = 'WindowedSwapEffectUpgradeOverride=1;'

function Program-Folders {
    $windows = $env:windir.TrimEnd('\')
    [IO.DriveInfo]::GetDrives() | Where-Object { $_.DriveType -eq 'Fixed' -and $_.IsReady } | ForEach-Object {
        Get-ChildItem -LiteralPath $_.RootDirectory.FullName -Directory -ea 0 |
            Where-Object { $_.FullName -ne $windows } | ForEach-Object { $_.FullName + '\' }
    } | Sort-Object
}

function Our-Entries($key) {
    Get-ChildItem -LiteralPath $key -ea 0 | Where-Object { $_.PSChildName.StartsWith($ours) }
}

# What the entries say, in the order they are numbered, to tell whether anything has changed.
function Written($key) {
    Our-Entries $key | Sort-Object { [int]$_.PSChildName.Substring($ours.Length) } | ForEach-Object {
        $entry = Get-ItemProperty -LiteralPath $_.PSPath
        $entry.Name + '|' + $entry.D3DBehaviors
    }
}

# The key itself goes only when nothing is left in it, as another program's entries live there too.
function Remove-Ours($key) {
    Our-Entries $key | ForEach-Object { Remove-Item -LiteralPath $_.PSPath -Recurse -Force -ErrorAction Stop }
    $left = Get-Item -LiteralPath $key -ea 0
    if ($left -and $left.SubKeyCount -eq 0 -and $left.ValueCount -eq 0) { Remove-Item -LiteralPath $key -Force -ErrorAction Stop }
}

try {
    $folders = @(if ($on) { Program-Folders })
    $wanted = ($folders | ForEach-Object { $_ + '|' + $behaviour }) -join "`n"
    $changed = $false
    foreach ($key in $keys) {
        if (((Written $key) -join "`n") -eq $wanted) { continue }
        $changed = $true
        Remove-Ours $key
        $number = 0
        foreach ($folder in $folders) {
            $number++
            $entry = Join-Path $key ($ours + $number)
            New-Item -Path $entry -Force -ErrorAction Stop | Out-Null
            New-ItemProperty -LiteralPath $entry -Name Name -PropertyType String -Value $folder -Force -ErrorAction Stop | Out-Null
            New-ItemProperty -LiteralPath $entry -Name D3DBehaviors -PropertyType String -Value $behaviour -Force -ErrorAction Stop | Out-Null
        }
    }
    if (-not $changed) { "kept $($folders.Count) folders" }
    elseif ($on) { "on for $($folders -join ', ')" }
    else { 'off, entries removed' }
} catch {
    "FAILED window speed: $($_.Exception.Message)"
}
