# Sent by the host through the agent once the desktop is up, with $card set above it: the PCI device
# number of the card the machine runs on, in hex. Registers NVIDIA's Media Foundation transforms,
# which are how a program that asks Media Foundation for a hardware encoder finds the card's. The
# driver's INF does this on a PC, and a guest never installs it: it only has the driver's files, so
# Media Foundation finds no encoder there. What to write is read from that INF in the driver's
# folder, so a newer driver is followed. The INF gives each card the transforms it has, so only the
# install sections of this card's own lines count, and Windows can't say which card that is, as it
# sees the number Guesthouse gave the device. From the AddReg sections those name come the lines
# under MediaFoundation\Transforms, the class entries of the transforms named there, and the switch
# for hardware transforms. A line this can't read is said and left out. A value the registry already
# has as the INF writes it stays, so a second run writes nothing. One that is there with other data
# is written over with the INF's, so a transform's entries follow a changed driver. The switch is
# the exception. It is Windows' own, and every maker's hardware transforms go by it, so a value of
# it that someone set otherwise is said and left as it is. Nothing is ever removed.
$store = Join-Path $env:windir 'System32\HostDriverStore\FileRepository\nv_dispi.inf_amd64_gpupv'
$infName = 'nv_dispi.inf'
# NVIDIA's PCI vendor number, as the INF's lines for its cards have it.
$nvidia = '10DE'
$deviceNumber = '^[0-9A-Fa-f]{4}$'
# An INF names the folder its package's files are in by this number.
$packageFolder = '13'
# An INF's names for the registry's roots, and where each is under HKEY_LOCAL_MACHINE.
$roots = @{ HKCR = 'SOFTWARE\Classes\'; HKLM = '' }
# The flags of an AddReg line say what kind of value it writes. These are the kinds a transform's
# lines have. A line with any other flags is left out.
$kinds = @{ 0x0 = 'String'; 0x1 = 'Binary'; 0x10001 = 'DWord'; 0x20000 = 'ExpandString' }
$guid = '[0-9A-Fa-f]{8}(?:-[0-9A-Fa-f]{4}){3}-[0-9A-Fa-f]{12}'
$transformKey = '^(Wow6432Node\\)?MediaFoundation\\Transforms(\\|$)'
$transformId = "^(?:Wow6432Node\\)?MediaFoundation\\Transforms\\\{?$guid\}?(\\|`$)"
$switchKey = '^Software\\(Wow6432Node\\)?Microsoft\\Windows Media Foundation\\HardwareMFT$'
$field = [regex]'\G\s*(?:"(?<quoted>(?:[^"]|"")*)"|(?<plain>[^,;"]*))\s*(?<end>,|;.*$|$)'
# A binary value's line is long, and the start of it is enough to find it by.
$shown = 160

# An INF goes on to the next line after a backslash at a line's end.
function Join-Continued($lines) {
    $whole = ''
    foreach ($line in $lines) {
        if ($line -match '^(.*)\\\s*$') { $whole += $Matches[1]; continue }
        $whole + $line
        $whole = ''
    }
    if ($whole) { $whole }
}

# A line's fields: split at the commas outside quotes, the quotes taken off, and a comment at the
# end dropped. Nothing for a line with a quote that doesn't close.
function Split-Fields($line) {
    $fields = @()
    $at = 0
    while ($true) {
        $found = $field.Match($line, $at)
        if (-not $found.Success) { return $null }
        if ($found.Groups['quoted'].Success) { $fields += $found.Groups['quoted'].Value.Replace('""', '"') }
        else { $fields += $found.Groups['plain'].Value.Trim() }
        if ($found.Groups['end'].Value -ne ',') { return , $fields }
        $at = $found.Index + $found.Length
    }
}

# The text with each %name% replaced by what the INF's strings say, and %% by one. Nothing for a
# name the strings don't have.
function Expand-Tokens($text, $strings) {
    $done = ''
    $rest = $text
    while ($rest -match '^(.*?)%([^%]*)%(.*)$') {
        $before, $name, $rest = $Matches[1], $Matches[2], $Matches[3]
        if ($name -eq '') { $done += $before + '%' }
        elseif ($strings.ContainsKey($name)) { $done += $before + $strings[$name] }
        else { return $null }
    }
    $done + $rest
}

# A number as an INF writes it, in hex after 0x or in decimal, as the registry keeps a DWORD: one
# with its top bit set is negative.
function Read-Number($text) {
    if ($text -eq '') { return 0 }
    if ($text -match '^0x') { return [Convert]::ToInt32($text, 16) }
    [BitConverter]::ToInt32([BitConverter]::GetBytes([uint32]$text), 0)
}

# What an AddReg line writes, from its fields after the root and the key: the value's name, its
# kind and its data. Nothing for a line this can't read.
function Read-Value($fields, $strings) {
    $name = Expand-Tokens $fields[2] $strings
    $flags = Expand-Tokens $fields[3] $strings
    if ($null -eq $name -or $null -eq $flags) { return $null }
    $data = @($fields | Select-Object -Skip 4)
    try {
        $kind = $kinds[(Read-Number $flags)]
        if ($kind -eq 'Binary') {
            $value = [byte[]]@($data | Where-Object { $_ -ne '' } | ForEach-Object { [Convert]::ToByte($_, 16) })
        } elseif ($kind -eq 'DWord' -and $data.Count -eq 1) {
            $value = Read-Number $data[0]
        } elseif ($kind -in 'String', 'ExpandString' -and $data.Count -le 1) {
            $value = Expand-Tokens "$data" $strings
        } else {
            return $null
        }
    } catch {
        return $null
    }
    if ($null -eq $value) { return $null }
    [pscustomobject]@{ Name = $name; Kind = $kind; Value = $value }
}

# The registry entries an INF's text asks for the transforms of the card with the device number
# $card, with the package's files in $folder: whether the card is Listed, the Entries, each with its
# Key under HKEY_LOCAL_MACHINE, Name, Kind and Value, the Transforms they are for, and the lines
# Skipped as unreadable. This only reads, so it can be tried on any text.
function Read-Registrations($lines, $folder, $card) {
    $strings = @{ $packageFolder = $folder }
    $sections = @{}
    $registers = @{}
    $own = @()
    $withSubsystem = @()
    $id = "PCI\VEN_$nvidia&DEV_$card"
    $model = '^[^=;]*=(.*' + [regex]::Escape($id) + '.*)$'
    $section = ''
    foreach ($line in Join-Continued $lines) {
        if ($line -match '^\s*\[([^\]]+)\]') {
            $section = $Matches[1].Trim()
            if (-not $sections.ContainsKey($section)) { $sections[$section] = New-Object Collections.Generic.List[string] }
        } elseif ($line -match '^\s*AddReg\s*=(.*)$') {
            $registers[$section] += Split-Fields $Matches[1]
        } elseif ($section -eq 'Strings' -and $line -match '^\s*([^=;\s]+)\s*=(.*)$') {
            $name = $Matches[1]
            $said = Split-Fields $Matches[2]
            if ($null -ne $said) { $strings[$name] = $said[0] }
        } elseif ($line -match $model) {
            # A card's line: a name, then its install section and the IDs it's for.
            $named = Split-Fields $Matches[1]
            $ids = @($named | Select-Object -Skip 1)
            if ($ids -contains $id) { $own += $named[0] }
            elseif ($ids -like "$id&SUBSYS_*") { $withSubsystem += $named[0] }
        } elseif ($section -and $line -match '^\s*(HKCR|HKLM)\s*,.*(Media ?Foundation|CLSID)') {
            $sections[$section].Add($line)
        }
    }
    # A line for the card alone counts before the lines for it in one maker's laptop or board.
    $installs = if ($own) { $own } else { $withSubsystem }
    $found = @{ Listed = [bool]$installs; Entries = [ordered]@{}; Transforms = @(); Skipped = @() }
    if (-not $installs) { return $found }

    # Each line's key first, as the transforms' keys say which classes are theirs.
    $keyed = @()
    $unread = @()
    $asked = $installs | Select-Object -Unique | ForEach-Object { $registers[$_] } | Select-Object -Unique |
        Where-Object { $_ -and $sections.ContainsKey($_) } | ForEach-Object { $sections[$_] } | Select-Object -Unique
    foreach ($line in $asked) {
        $fields = Split-Fields $line
        $key = $null
        if ($null -ne $fields -and $fields.Count -ge 4) { $key = Expand-Tokens $fields[1] $strings }
        if ($null -eq $key) { $unread += $line; continue }
        $keyed += [pscustomobject]@{ Line = $line; Root = $fields[0]; Key = $key; Fields = $fields }
    }
    $transforms = @($keyed | Where-Object { $_.Root -eq 'HKCR' -and $_.Key -match $transformId } |
            ForEach-Object { [regex]::Match($_.Key, $guid).Value.ToUpper() } | Sort-Object -Unique)
    if (-not $transforms) { return $found }
    $found.Transforms = $transforms
    $classKey = "^(Wow6432Node\\)?CLSID\\\{($($transforms -join '|'))\}(\\|`$)"
    $ours = "MediaFoundation\\Transforms|Windows Media Foundation\\HardwareMFT|$($transforms -join '|')"

    foreach ($line in $keyed) {
        $wanted = if ($line.Root -eq 'HKCR') { $line.Key -match $transformKey -or $line.Key -match $classKey }
        else { $line.Key -match $switchKey }
        if (-not $wanted) { continue }
        $value = Read-Value $line.Fields $strings
        if ($null -eq $value) { $unread += $line.Line; continue }
        $key = $roots[$line.Root] + $line.Key
        $value | Add-Member -NotePropertyName Key -NotePropertyValue $key
        $found.Entries["$key`n$($value.Name)"] = $value
    }
    $found.Skipped = @($unread | Where-Object { $_ -match $ours })
    $found
}

# What the registry has under the entry's name: 'same' when it's there as the INF writes it, 'other'
# when it's of another kind or has other data, and 'none'.
function Find-Entry($entry) {
    $key = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey($entry.Key)
    if (-not $key) { return 'none' }
    try {
        $has = $key.GetValue($entry.Name, $null, 'DoNotExpandEnvironmentNames')
        if ($null -eq $has) { return 'none' }
        $same = $key.GetValueKind($entry.Name) -eq $entry.Kind -and ($has -join ',') -eq ($entry.Value -join ',')
        if ($same) { 'same' } else { 'other' }
    } finally {
        $key.Close()
    }
}

function Write-Entry($entry) {
    $key = [Microsoft.Win32.Registry]::LocalMachine.CreateSubKey($entry.Key)
    try {
        # Anything but an array of bytes is refused for a binary value, and PowerShell hands over
        # an array of objects unless told.
        if ($entry.Kind -eq 'Binary') { $key.SetValue($entry.Name, [byte[]]$entry.Value, $entry.Kind) }
        else { $key.SetValue($entry.Name, $entry.Value, $entry.Kind) }
    } finally {
        $key.Close()
    }
}

# Registers what the INF in $folder asks for the card $card, and says what it did. The last line
# starts with 'kept' when everything was there, with 'registered' when something was written, failed
# or left as someone set it, with 'nothing to register' for a driver whose INF has no transforms for
# the card, and with 'no transforms registered' when the card isn't known or the INF doesn't list it.
function Register-Transforms($folder, $card) {
    if ($card -notmatch $deviceNumber) { return "no transforms registered: '$card' isn't a card's device number" }
    $inf = Join-Path $folder $infName
    if (-not (Test-Path -LiteralPath $inf)) { return "nothing to register: no $inf" }
    # An INF with no mark of its encoding is in the system's own, as Windows reads it.
    $found = Read-Registrations ([IO.File]::ReadAllLines($inf, [Text.Encoding]::Default)) $folder $card
    if (-not $found.Listed) { return "no transforms registered: $inf doesn't list the card $card" }
    if (-not $found.Entries.Count -and -not $found.Skipped) {
        return "nothing to register: $inf has no transforms for the card $card"
    }
    foreach ($line in $found.Skipped) {
        "skipped a line that can't be read: $($line.Substring(0, [Math]::Min($line.Length, $shown)))"
    }
    $written = 0
    $failed = 0
    $left = 0
    foreach ($entry in $found.Entries.Values) {
        try {
            $has = Find-Entry $entry
            if ($has -eq 'same') { continue }
            if ($has -eq 'other' -and $entry.Key -match $switchKey) {
                $left++
                "left HKLM\$($entry.Key) $($entry.Name) as it is set, which isn't as the INF writes it"
                continue
            }
            Write-Entry $entry
            $written++
        } catch {
            $failed++
            "FAILED HKLM\$($entry.Key) $($entry.Name): $($_.Exception.Message)"
        }
    }
    $all = "$($found.Entries.Count) values of $($found.Transforms.Count) transforms"
    if (-not ($written -or $failed -or $left)) { return "kept $all" }
    $said = "registered $written of $all, failed $failed"
    if ($left) { $said += ", left $left of the switch's as set" }
    $said
}

Register-Transforms $store $card
