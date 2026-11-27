# Brings the guest's copy of NVIDIA's driver, in the host driver store, to the version on the driver
# disc the app puts in the VM when the host's driver has changed. Programs load the driver from that
# folder, so it is never written over while it is in use: the disc is copied beside it first, checked,
# and then the two folders change places. If something has the driver loaded, the folder can't be
# renamed, and the same change is queued for the next boot, when Windows makes it before anything
# loads. A copy that fails or doesn't check out is removed, and the driver there stays as it was.
#
# Usage: nv-store.ps1 [-Version VERSION] [-Source FOLDER] [-Status FILE]
# Without -Version the disc's own version is the one wanted. Without -Source the disc is looked for
# on the CD drives. The last line said, also added to the status file, is one of these:
#   store current VERSION [restart]    the folder has that version, as the disc has it
#   store replaced VERSION [restart]   the folders changed places now
#   store queued VERSION               the change waits for the next boot
#   store no disc | store no room ... | store failed ...
# "restart" means the old copy could not be removed, so something may still have it loaded.
param(
    [string]$Version = '',
    [string]$Source = '',
    [string]$Status = '',
    [string]$StoreRoot = 'C:\Windows\System32\HostDriverStore\FileRepository',
    [string]$StoreFolder = 'nv_dispi.inf_amd64_gpupv'
)
$DiscFolder = 'nvidia'
$InfName = 'nv_dispi.inf'
# Room left over on the drive once the copy is there, so Windows doesn't run out.
$HeadroomBytes = 512MB
$LockName = 'Global\GuesthouseNvStore'
$LockMinutes = 30
$PendingKey = 'SYSTEM\CurrentControlSet\Control\Session Manager'
$PendingValue = 'PendingFileRenameOperations'

function Say($line) {
    $line
    if ($Status) { Add-Content -LiteralPath $Status -Value $line }
}

# The version in an INF's DriverVer line, as 32.0.16.1742. INFs are Latin-1 or UTF-16.
function Get-DriverVer($folder) {
    $path = Join-Path $folder $InfName
    if (-not (Test-Path -LiteralPath $path)) { return $null }
    $bytes = [IO.File]::ReadAllBytes($path)
    if ($bytes.Length -ge 2 -and $bytes[0] -eq 0xFF -and $bytes[1] -eq 0xFE) {
        $text = [Text.Encoding]::Unicode.GetString($bytes, 2, $bytes.Length - 2)
    } else {
        $text = [Text.Encoding]::GetEncoding(28591).GetString($bytes)
    }
    $found = [regex]::Match($text, '(?m)^\s*DriverVer\s*=\s*[^,\r\n]*,\s*([0-9]+(\.[0-9]+){3})')
    if ($found.Success) { return $found.Groups[1].Value }
    return $null
}

# Each file under a folder by its path inside it, in lower case as Windows ignores case, with its size.
function Get-Tree($root) {
    $tree = @{}
    if (-not (Test-Path -LiteralPath $root)) { return $tree }
    $full = (Get-Item -LiteralPath $root).FullName.TrimEnd('\', '/')
    foreach ($file in Get-ChildItem -LiteralPath $root -Recurse -File -Force) {
        $tree[$file.FullName.Substring($full.Length + 1).ToLowerInvariant()] = $file.Length
    }
    return $tree
}

# Each file's path inside a folder, as it is written.
function Get-Inside($root) {
    if (-not (Test-Path -LiteralPath $root)) { return @() }
    $full = (Get-Item -LiteralPath $root).FullName.TrimEnd('\', '/')
    foreach ($file in Get-ChildItem -LiteralPath $root -Recurse -File -Force) { $file.FullName.Substring($full.Length + 1) }
}

function Test-SameTree($a, $b) {
    if ($a.Count -ne $b.Count) { return $false }
    foreach ($name in $a.Keys) {
        if (-not $b.ContainsKey($name) -or $b[$name] -ne $a[$name]) { return $false }
    }
    return $true
}

# The driver's folder on a CD drive, as the app's driver disc has it.
function Find-Disc {
    foreach ($drive in [IO.DriveInfo]::GetDrives()) {
        if ($drive.DriveType -ne 'CDRom' -or -not $drive.IsReady) { continue }
        $folder = Join-Path $drive.RootDirectory.FullName $DiscFolder
        if (Test-Path -LiteralPath (Join-Path $folder $InfName)) { return $folder }
    }
    return $null
}

function Remove-Folder($path) {
    if (-not (Test-Path -LiteralPath $path)) { return $true }
    Remove-Item -LiteralPath $path -Recurse -Force -ErrorAction SilentlyContinue
    return -not (Test-Path -LiteralPath $path)
}

# The renames that make the change at boot, as pairs of paths: every file of the folder in use goes to
# the old one, then every file of the copy comes in. Files are moved, not folders, as that is what the
# queue is documented to do.
function Get-RenamePairs($store, $new, $old) {
    $pairs = @()
    foreach ($name in Get-Inside $store | Sort-Object) {
        $pairs += , @((Join-Path $store $name), (Join-Path $old $name))
    }
    foreach ($name in Get-Inside $new | Sort-Object) {
        $pairs += , @((Join-Path $new $name), (Join-Path $store $name))
    }
    return $pairs
}

# Without a registry, as where the app's tests run it, nothing is queued.
function Get-Pending {
    $machine = $null
    try { $machine = [Microsoft.Win32.Registry]::LocalMachine } catch { }
    if (-not $machine) { return @() }
    $key = $machine.OpenSubKey($PendingKey)
    if (-not $key) { return @() }
    $value = $key.GetValue($PendingValue)
    $key.Close()
    if ($value) { return @($value) }
    return @()
}

function Test-Queued($new) {
    $mark = "\??\$new\"
    foreach ($entry in Get-Pending) {
        if ($entry.StartsWith($mark, [StringComparison]::OrdinalIgnoreCase)) { return $true }
    }
    return $false
}

# Adds the renames to the ones Windows makes at its next boot, in one write, so either all of them
# are queued or none is. The folders they go to are made now, as a rename doesn't make them. Another
# program's entries may hold empty strings, which reading the value can lose, so this only writes the
# value when there is none: rewriting it could turn someone's deletion into a rename. A value that
# doesn't read back as written goes.
function Add-Pending($store, $new, $old) {
    $key = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey($PendingKey, $true)
    if ($key.GetValueNames() -contains $PendingValue) {
        $key.Close()
        throw 'Windows has other changes waiting for its next restart'
    }
    foreach ($folder in Get-ChildItem -LiteralPath $new -Recurse -Directory -Force) {
        $inside = $folder.FullName.Substring($new.Length + 1)
        New-Item -ItemType Directory -Force (Join-Path $store $inside) | Out-Null
    }
    New-Item -ItemType Directory -Force $old | Out-Null
    foreach ($folder in Get-ChildItem -LiteralPath $store -Recurse -Directory -Force) {
        $inside = $folder.FullName.Substring($store.Length + 1)
        New-Item -ItemType Directory -Force (Join-Path $old $inside) | Out-Null
    }
    $entries = New-Object System.Collections.Generic.List[string]
    foreach ($pair in Get-RenamePairs $store $new $old) {
        $entries.Add("\??\$($pair[0])")
        $entries.Add("\??\$($pair[1])")
    }
    $kind = [Microsoft.Win32.RegistryValueKind]::MultiString
    $key.SetValue($PendingValue, $entries.ToArray(), $kind)
    $after = @(Get-Pending)
    if (($after -join "`n") -ne ($entries -join "`n") -or $after.Count -ne $entries.Count) {
        $key.DeleteValue($PendingValue, $false)
        $key.Close()
        throw 'the queued renames did not read back as written'
    }
    $key.Close()
}

function Update-Store($Version, $Source, $StoreRoot, $StoreFolder) {
    $store = Join-Path $StoreRoot $StoreFolder
    $new = "$store.new"
    $old = "$store.old"
    # A change cut short between its two renames left the store missing and the old copy beside it.
    if (-not (Test-Path -LiteralPath $store) -and (Test-Path -LiteralPath $old)) {
        Move-Item -LiteralPath $old -Destination $store -ErrorAction Stop
    }
    $from = $Source
    if (-not $from) { $from = Find-Disc }
    $wanted = $Version
    if ($from -and -not $wanted) { $wanted = Get-DriverVer $from }
    if (Test-Queued $new) { return "store queued $wanted" }
    $disc = $null
    if ($from) { $disc = Get-Tree $from }
    if ($wanted -and (Get-DriverVer $store) -eq $wanted -and (-not $disc -or (Test-SameTree $disc (Get-Tree $store)))) {
        # Done, now or by an earlier run: what is left of the change goes.
        $gone = (Remove-Folder $old) -and (Remove-Folder $new)
        if ($gone) { return "store current $wanted" }
        return "store current $wanted restart"
    }
    if (-not $from) { return 'store no disc' }
    $onDisc = Get-DriverVer $from
    if ($onDisc -ne $wanted) { return "store failed: the disc has driver $onDisc, not $wanted" }
    # A copy that a run before this one finished is used as it is.
    if (-not (Test-SameTree $disc (Get-Tree $new))) {
        if (-not (Remove-Folder $new)) { return "store failed: an earlier copy at $new could not be removed" }
        $size = 0
        foreach ($length in $disc.Values) { $size += $length }
        $drive = [IO.DriveInfo]::new([IO.Path]::GetPathRoot($StoreRoot))
        $free = $drive.AvailableFreeSpace
        if ($free -lt $size + $HeadroomBytes) {
            $needMb = [math]::Ceiling(($size + $HeadroomBytes) / 1MB)
            $haveMb = [math]::Floor($free / 1MB)
            return "store no room: the driver needs $needMb MB on $($drive.Name), and it has $haveMb MB free"
        }
        Say 'store copying'
        try {
            Copy-Item -LiteralPath $from -Destination $new -Recurse -Force -ErrorAction Stop
            # A disc's files are read-only, and the old copy has to be removable once it is replaced.
            Get-ChildItem -LiteralPath $new -Recurse -File -Force | ForEach-Object { $_.IsReadOnly = $false }
        } catch {
            Remove-Folder $new | Out-Null
            return "store failed: the copy stopped: $($_.Exception.Message)"
        }
        if (-not (Test-SameTree $disc (Get-Tree $new)) -or (Get-DriverVer $new) -ne $wanted) {
            Remove-Folder $new | Out-Null
            return 'store failed: the copy does not match the disc'
        }
    }
    if (-not (Remove-Folder $old)) { return "store failed: an older copy at $old could not be removed" }
    if (Test-Path -LiteralPath $store) {
        try {
            Move-Item -LiteralPath $store -Destination $old -ErrorAction Stop
        } catch {
            # Something has the driver loaded.
            try {
                Add-Pending $store $new $old
            } catch {
                return "store failed: the change could not be queued for the next boot: $($_.Exception.Message)"
            }
            return "store queued $wanted"
        }
    }
    try {
        Move-Item -LiteralPath $new -Destination $store -ErrorAction Stop
    } catch {
        if (Test-Path -LiteralPath $old) { Move-Item -LiteralPath $old -Destination $store -ErrorAction SilentlyContinue }
        return "store failed: the copy could not take the store's place: $($_.Exception.Message)"
    }
    if (Remove-Folder $old) { return "store replaced $wanted" }
    return "store replaced $wanted restart"
}

# Dot-sourced, as the app's tests do, it only defines the functions.
if ($MyInvocation.InvocationName -ne '.') {
    # The status file has this run's lines only.
    if ($Status) { Remove-Item -LiteralPath $Status -Force -ErrorAction SilentlyContinue }
    $lock = New-Object System.Threading.Mutex($false, $LockName)
    $held = $false
    try {
        try { $held = $lock.WaitOne([TimeSpan]::FromMinutes($LockMinutes)) } catch [System.Threading.AbandonedMutexException] { $held = $true }
        if (-not $held) {
            Say 'store failed: another run kept the store busy'
        } else {
            try { Say (Update-Store -Version $Version -Source $Source -StoreRoot $StoreRoot -StoreFolder $StoreFolder) } catch { Say "store failed: $($_.Exception.Message)" }
        }
    } finally {
        if ($held) { $lock.ReleaseMutex() }
        $lock.Dispose()
    }
}
