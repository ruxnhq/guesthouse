# The guest's screen: remoteman's host, which shows its desktop to the remoteman viewer on the PC
# that runs it, encoded on the guest's card. report.ps1 runs this at every start, as SYSTEM, before
# its last line, so Send-Line is there. The host puts what it needs in C:\Guesthouse\screen before
# each boot: remoteman's host's installer, from remoteman's release, the devices to trust, and which
# encoder suits the card. remoteman's installer does all of remoteman's own work, run unattended:
# where its files go, its service and how it's started, its trust list. This script only signs the
# guest in, lets the port in and says what happened.
$screen = 'C:\Guesthouse\screen'
$setup = Join-Path $screen 'remoteman-host-setup.exe'
$folder = 'C:\Program Files\remoteman host'
$hostExe = Join-Path $folder 'remoteman-host.exe'
$state = 'C:\ProgramData\remoteman\host'
$service = 'remoteman host'

# Signing in by itself at every start, so the screen opens on the desktop and not on a sign-in
# screen that would ask the viewer for the password. The host writes the account here once, and it
# goes as soon as Windows has it. It takes effect from the next start.
$logon = Join-Path $screen 'logon.txt'
if (Test-Path -LiteralPath $logon) {
    $lines = Get-Content -LiteralPath $logon
    $user = (($lines | Where-Object { $_ -like 'user: *' }) -replace '^user: ', '').Trim()
    $password = (($lines | Where-Object { $_ -like 'password: *' }) -replace '^password: ', '').Trim()
    if ($user -and $password) {
        $winlogon = 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon'
        Set-ItemProperty -LiteralPath $winlogon -Name AutoAdminLogon -Value '1'
        Set-ItemProperty -LiteralPath $winlogon -Name DefaultUserName -Value $user
        Set-ItemProperty -LiteralPath $winlogon -Name DefaultPassword -Value $password
        Set-ItemProperty -LiteralPath $winlogon -Name DefaultDomainName -Value $env:COMPUTERNAME
        Remove-ItemProperty -LiteralPath $winlogon -Name AutoLogonCount -ErrorAction SilentlyContinue
        # Windows 11 hides signing in with a password until this is 0.
        $passwordless = 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\PasswordLess\Device'
        New-Item -Path $passwordless -Force | Out-Null
        Set-ItemProperty -LiteralPath $passwordless -Name DevicePasswordLessBuildVersion -Value 0 -Type DWord
        # No lock screen, and no locking or sleeping when nobody types, as a locked guest shows the
        # viewer a sign-in screen.
        $personalization = 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\Personalization'
        New-Item -Path $personalization -Force | Out-Null
        Set-ItemProperty -LiteralPath $personalization -Name NoLockScreen -Value 1 -Type DWord
        $system = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System'
        Set-ItemProperty -LiteralPath $system -Name InactivityTimeoutSecs -Value 0 -Type DWord
        powercfg.exe /change monitor-timeout-ac 0 | Out-Null
        powercfg.exe /change standby-timeout-ac 0 | Out-Null
        Send-Line "screen: $user signs in by itself from the next start"
    }
    Remove-Item -LiteralPath $logon -Force
}

if (-not (Test-Path -LiteralPath $setup)) {
    Send-Line "screen: remoteman's host wasn't given, so the guest has no screen"
    return
}

# Writes what Windows holds in memory of its registry and of drive C to the disk. A machine stopped
# at once, as a guest with a card is, loses what wasn't written: a whole install's service and
# firewall rule were lost so, while its files stayed.
function Save-ToDisk {
    if (-not ('Guesthouse.Registry' -as [type])) {
        Add-Type -Namespace Guesthouse -Name Registry -MemberDefinition '[DllImport("advapi32.dll")] public static extern int RegFlushKey(IntPtr key);'
    }
    # HKEY_LOCAL_MACHINE's predefined handle, which flushes every hive under it.
    [Guesthouse.Registry]::RegFlushKey([IntPtr]0x80000002) | Out-Null
    Write-VolumeCache -DriveLetter C -ErrorAction SilentlyContinue
}

# remoteman's host keeps files and the clipboard off unless its owner says otherwise. Here the owner
# is Guesthouse, and the only viewer that may connect is the one this guest trusts, so both are on,
# at every start. remoteman's host reads its switches from allowed.json in its state folder when it
# starts, so they are merged into that file first (every other switch it saved stays as it is), and
# once the host is up `ctl allow ... on` turns them on in the running host, which keeps them too.
function Set-SharingFile {
    $file = Join-Path $state 'allowed.json'
    try {
        $saved = $null
        if (Test-Path -LiteralPath $file) {
            $saved = Get-Content -LiteralPath $file -Raw | ConvertFrom-Json -ErrorAction Stop
        }
        if (-not $saved) { $saved = [pscustomobject]@{} }
        foreach ($key in 'files', 'clipboard') {
            $saved | Add-Member -NotePropertyName $key -NotePropertyValue $true -Force
        }
        New-Item -ItemType Directory -Path $state -Force | Out-Null
        # No byte order mark: remoteman's host doesn't read one.
        [System.IO.File]::WriteAllText($file, ($saved | ConvertTo-Json -Compress), (New-Object System.Text.UTF8Encoding($false)))
        Send-Line "screen: files and clipboard are on in $file"
    } catch {
        Send-Line "screen: couldn't write ${file}: $($_.Exception.Message)"
    }
}

function Enable-Sharing {
    foreach ($what in 'clipboard', 'files') {
        $said = ''
        for ($try = 0; $try -lt 20; $try += 1) {
            $said = ((& $hostExe ctl --port 7791 allow $what on 2>&1) | Out-String).Trim()
            if ($said -eq 'ok') { break }
            Start-Sleep -Seconds 3
        }
        Send-Line "screen: allow $what on: $said"
    }
}

# remoteman's host looks for a newer remoteman by itself from 0.1.9. Guesthouse pins the release it
# gives the guest and updates it itself, so the guest shouldn't reach a download site on its own:
# `set updates off` at every start, which the host keeps in updates.json in its state folder. When it
# doesn't take, the guest says why and its screen goes on as it is.
function Set-UpdatesOff {
    $said = ''
    for ($try = 0; $try -lt 10; $try += 1) {
        $said = ((& $hostExe ctl --port 7791 set updates off 2>&1) | Out-String).Trim()
        if ($said -eq 'ok') { break }
        Start-Sleep -Seconds 3
    }
    Send-Line "screen: set updates off: $said"
}

# remoteman's installer is run again whenever the host brings a different one, a different viewer
# or a different encoder, and whenever its service isn't there, whatever the guest's note of the
# last install says. Run again, it keeps the host's identity, so its fingerprint stays. It replaces
# the host's whole trust list with the one it's given, so it's given trusted: the viewer and every
# device paired in the guest, which the host reads off the guest's disk before each boot. Its note
# is of viewer.txt, the viewer's line alone, so a device paired since doesn't run it again and
# blink the picture. viewer.txt holds what trusted held when the viewer was all it had, so a guest
# installed then keeps its note and isn't installed again.
function Get-InstallNote([string]$Folder) {
    $setupHash = (Get-FileHash -LiteralPath (Join-Path $Folder 'remoteman-host-setup.exe') -Algorithm SHA256).Hash
    $viewerHash = (Get-FileHash -LiteralPath (Join-Path $Folder 'viewer.txt') -Algorithm SHA256).Hash
    $encoder = (Get-Content -LiteralPath (Join-Path $Folder 'encoder.txt') -Raw).Trim()
    ($setupHash, $viewerHash, $encoder) -join ' '
}

# Whether the note of the last install in $Folder isn't $Note.
function Test-NoteChanged([string]$Folder, [string]$Note) {
    $file = Join-Path $Folder 'installed.txt'
    $installed = if (Test-Path -LiteralPath $file) { (Get-Content -LiteralPath $file -Raw).Trim() } else { '' }
    $installed -ne $Note
}

function Get-SetupArguments([string]$Folder, [string]$Encoder, [string]$Log) {
    # Start-Process joins these unquoted, so the paths are quoted here.
    '--silent', '--trust', "`"$(Join-Path $Folder 'trusted')`"", '--encoder', $Encoder, '--no-pair', '--log', "`"$Log`""
}

# Who may reach remoteman's port: this PC alone. hosts.txt, which the host writes at each boot, says
# the network the machine is on and this PC's addresses, a line each. On the shared network this PC
# is the guest's default gateway too, which has its address only once the machine has started. With
# no file, from a host before this, or nothing in it, the rules are left as they are.
function Get-ScreenHosts([string]$Folder, [string[]]$Gateways) {
    $file = Join-Path $Folder 'hosts.txt'
    if (-not (Test-Path -LiteralPath $file)) { return @() }
    $lines = @(Get-Content -LiteralPath $file | ForEach-Object { $_.Trim() } | Where-Object { $_ })
    if ($lines.Count -eq 0) { return @() }
    $address = '^\d{1,3}(\.\d{1,3}){3}$'
    $allowed = @($lines | Select-Object -Skip 1 | Where-Object { $_ -match $address })
    switch ($lines[0]) {
        'shared' { $allowed += @($Gateways | Where-Object { $_ -match $address -and $_ -ne '0.0.0.0' }) }
        'bridged' { }
        default { return @() }
    }
    return @($allowed | Select-Object -Unique)
}

# Lets only $Allowed reach the port by each rule named, Guesthouse's own and the one remoteman's
# installer makes, which it makes open again each time it runs.
function Limit-ScreenRules([string[]]$Allowed, [string[]]$Names) {
    if (@($Allowed).Count -eq 0) { return }
    foreach ($name in $Names) {
        if (Get-NetFirewallRule -DisplayName $name -ErrorAction SilentlyContinue) {
            Set-NetFirewallRule -DisplayName $name -RemoteAddress $Allowed -ErrorAction SilentlyContinue
        }
    }
}

$encoder = (Get-Content -LiteralPath (Join-Path $screen 'encoder.txt') -Raw).Trim()
$wanted = Get-InstallNote $screen
$installedFile = Join-Path $screen 'installed.txt'
$missing = -not (Get-Service -Name $service -ErrorAction SilentlyContinue)
if ($missing -or -not (Test-Path -LiteralPath $hostExe) -or (Test-NoteChanged $screen $wanted)) {
    Send-Line "screen: installing remoteman host, encoder $encoder"
    # Its own log, where it says each step, and FAILED and why as its last line when it fails.
    $setupLog = Join-Path $screen 'remoteman-setup.log'
    $arguments = Get-SetupArguments $screen $encoder $setupLog
    $run = Start-Process -FilePath $setup -ArgumentList $arguments -Wait -PassThru
    if ($run.ExitCode -ne 0) {
        Send-Line "screen: remoteman's installer failed, with $($run.ExitCode)"
        if (Test-Path -LiteralPath $setupLog) {
            foreach ($line in (Get-Content -LiteralPath $setupLog -Tail 12)) { Send-Line "screen: setup: $line" }
        }
        return
    }
    Set-Content -LiteralPath $installedFile -Value $wanted -Encoding ascii
    Save-ToDisk
    Send-Line 'screen: remoteman host installed'
}

Set-SharingFile
$status = Get-Service -Name $service -ErrorAction SilentlyContinue
if ($status -and $status.Status -ne 'Running') {
    try { Start-Service -Name $service -ErrorAction Stop } catch { Send-Line "screen: remoteman's service didn't start: $($_.Exception.Message)" }
    Start-Sleep -Seconds 3
    $status = Get-Service -Name $service -ErrorAction SilentlyContinue
}
Send-Line "screen: remoteman's service is $($status.Status), command $((Get-ItemProperty -LiteralPath "HKLM:\SYSTEM\CurrentControlSet\Services\$service" -ErrorAction SilentlyContinue).ImagePath)"
# Why it stopped, if it did, as Windows saw it: the service manager's word, and any crash of the
# host's program since this start.
$since = (Get-CimInstance Win32_OperatingSystem).LastBootUpTime
foreach ($entry in Get-WinEvent -FilterHashtable @{ LogName = 'System'; ProviderName = 'Service Control Manager'; StartTime = $since } -ErrorAction SilentlyContinue |
    Where-Object { $_.Message -like '*remoteman*' } | Select-Object -First 5) {
    Send-Line "screen: service manager: $(($entry.Message -replace '\s+', ' '))"
}
foreach ($entry in Get-WinEvent -FilterHashtable @{ LogName = 'Application'; Id = 1000, 1001, 1026; StartTime = $since } -ErrorAction SilentlyContinue |
    Where-Object { $_.Message -like '*remoteman*' } | Select-Object -First 3) {
    $text = $entry.Message -replace '\s+', ' '
    Send-Line "screen: crash: $($text.Substring(0, [Math]::Min(400, $text.Length)))"
}

# remoteman's installer lets its host's program in on its port. The viewer's packets never reached
# it in a guest, so the port is let in as well, by a rule of Guesthouse's own, and the guest says
# what its firewall and the port look like.
$rule = 'Guesthouse screen'
if (-not (Get-NetFirewallRule -DisplayName $rule -ErrorAction SilentlyContinue)) {
    New-NetFirewallRule -DisplayName $rule -Direction Inbound -Action Allow -Protocol UDP -LocalPort 7791 -Profile Any | Out-Null
}
$gateways = @(Get-NetRoute -DestinationPrefix '0.0.0.0/0' -ErrorAction SilentlyContinue | ForEach-Object { $_.NextHop })
Limit-ScreenRules (Get-ScreenHosts $screen $gateways) @('remoteman host', $rule)
foreach ($network in Get-NetConnectionProfile -ErrorAction SilentlyContinue) {
    Send-Line "screen: network $($network.InterfaceAlias) is $($network.NetworkCategory)"
}
foreach ($found in Get-NetFirewallRule -DisplayName 'remoteman host', $rule -ErrorAction SilentlyContinue) {
    $ports = $found | Get-NetFirewallPortFilter
    $program = ($found | Get-NetFirewallApplicationFilter).Program
    $from = @(($found | Get-NetFirewallAddressFilter).RemoteAddress) -join ' '
    Send-Line "screen: firewall rule $($found.DisplayName): enabled $($found.Enabled), $($found.Action), $($ports.Protocol) $($ports.LocalPort), from $from, program $program, profiles $($found.Profile)"
}
# The host's command line, the trust list, the rule and the sign-in, all kept now.
Save-ToDisk

# The host's fingerprint, which the viewer checks it against, as remoteman's host says it.
$fingerprint = ''
$waited = 0
while ($waited -lt 30) {
    $said = (& $hostExe fingerprint --state $state 2>$null | Select-Object -First 1)
    if ($said -match '^[0-9a-fA-F]{64}$') { $fingerprint = $said.ToLowerInvariant(); break }
    Start-Sleep -Seconds 1
    $waited += 1
}
if ($fingerprint) {
    Start-Sleep -Seconds 5
    $listening = @(Get-NetUDPEndpoint -LocalPort 7791 -ErrorAction SilentlyContinue | ForEach-Object { $_.LocalAddress }) -join ', '
    Send-Line "screen: listening on UDP 7791 at: $listening"
    Send-Line "screen: fingerprint $fingerprint"
} else {
    Send-Line "screen: remoteman's host didn't say its fingerprint"
}

# The host is up: its own update check off, kept on the disk at once, then files and the clipboard
# on in it as it runs (its file is read only at its start).
if ($fingerprint) {
    Set-UpdatesOff
    Save-ToDisk
    Enable-Sharing
}

# For a while after it starts, remoteman's own logs go to the serial port as they're written, with
# whether anything listens on its port, so the host sees how it starts while it does: what it wrote
# to the guest's disk is lost when the machine is stopped at once.
$watchSeconds = 180
# Lines of remoteman's host that name a person's device or a file it sent are left out: they would
# reach the machine's log and its bug reports. Each is known by its message, which tracing writes
# before the fields that name the device.
function Test-NamesDevice([string]$Line) {
    $messages = @(
        ': client name=',
        'this computer is locked: the device is asked for the password',
        'this computer was locked: unlocked for a paired device',
        'a wrong password for unlocking this computer',
        'a device came while sharing is off: turned away',
        "a device's permission changed by the owner",
        'a file from the device',
        'a new device: asking on the desktop',
        'the owner answered'
    )
    foreach ($message in $messages) { if ($Line.Contains($message)) { return $true } }
    return $false
}
$logs = @{ (Join-Path $state 'host.log') = 0; (Join-Path $state 'service.log') = 0 }
foreach ($log in @($logs.Keys)) {
    if (Test-Path -LiteralPath $log) { $logs[$log] = @(Get-Content -LiteralPath $log).Count }
}
for ($second = 0; $second -lt $watchSeconds; $second += 5) {
    foreach ($log in @($logs.Keys)) {
        if (-not (Test-Path -LiteralPath $log)) { continue }
        $lines = @(Get-Content -LiteralPath $log)
        $name = Split-Path -Leaf $log
        foreach ($line in $lines | Select-Object -Skip $logs[$log]) {
            if (-not (Test-NamesDevice $line)) { Send-Line "screen: $name $line" }
        }
        $logs[$log] = $lines.Count
    }
    if ($second % 30 -eq 0) {
        $listening = @(Get-NetUDPEndpoint -LocalPort 7791 -ErrorAction SilentlyContinue | ForEach-Object { "$($_.LocalAddress) pid $($_.OwningProcess)" }) -join ', '
        $hosts = @(Get-Process -Name 'remoteman-host' -ErrorAction SilentlyContinue | ForEach-Object { "pid $($_.Id) session $($_.SessionId)" }) -join ', '
        Send-Line "screen: at $second s, UDP 7791: [$listening], remoteman-host: [$hosts]"
    }
    Start-Sleep -Seconds 5
}
