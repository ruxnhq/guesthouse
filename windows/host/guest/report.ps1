# Says what the guest sees of its GPU and network, so the host can read it with no screen. Each line
# goes to a file on the guest's own disk, which the host reads once the machine has stopped, and to
# the first serial port too if there is one, which the host prints while the guest runs. It runs as
# SYSTEM at every start, from a task of its own, which SetupComplete.cmd has it register with
# -Register.
param([switch]$Register)
$ErrorActionPreference = 'Continue'

# The task starts it at every start, whatever the guest's clock says and with no rule about
# batteries. schtasks /sc onstart gives a task the time it was made as its start, and a guest's
# clock can be hours behind when it starts, which kept the report from running at all; and a
# guest can see the host's battery. A guest made before this has such a task, so the report puts
# it right whenever it runs.
$taskName = 'Guesthouse report'
$task = Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
# It has no time limit either: the report puts the clock right, a jump of hours, and it ends by
# itself within minutes.
$wrong = -not $task -or ($task.Triggers | Where-Object { $_.StartBoundary }) -or
    $task.Settings.DisallowStartIfOnBatteries -or $task.Settings.StopIfGoingOnBatteries -or
    $task.Settings.ExecutionTimeLimit -ne 'PT0S'
if ($wrong) {
    $action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument '-NoProfile -ExecutionPolicy Bypass -File C:\Guesthouse\report.ps1'
    $trigger = New-ScheduledTaskTrigger -AtStartup
    $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew -ExecutionTimeLimit ([TimeSpan]::Zero)
    $principal = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
    Register-ScheduledTask -TaskName $taskName -Action $action -Trigger $trigger -Settings $settings -Principal $principal -Force | Out-Null
}
if ($Register) {
    Start-ScheduledTask -TaskName $taskName
    exit 0
}

$report = 'C:\Guesthouse\report.txt'
Set-Content -LiteralPath $report -Value "started $(Get-Date -Format o)" -Encoding utf8

$port = $null
try {
    $port = New-Object -TypeName System.IO.Ports.SerialPort -ArgumentList 'COM1', 115200
    $port.Open()
} catch {
    Add-Content -LiteralPath $report -Value "no serial port: $($_.Exception.Message)" -Encoding utf8
    $port = $null
}

# Two threads write to the port, this one and the listener's below, and a line must never be cut
# into by another, so each holds this while it writes. screen.ps1 writes through Send-Line too.
$portLock = New-Object -TypeName System.Object

function Send-Line([string]$Text) {
    Add-Content -LiteralPath $report -Value $Text -Encoding utf8
    if ($port) {
        [System.Threading.Monitor]::Enter($portLock)
        try { $port.WriteLine("guesthouse: $Text") } catch { $port = $null } finally { [System.Threading.Monitor]::Exit($portLock) }
    }
}

# ctl channel: begin
# The host asks remoteman's host in the guest what it would ask with `remoteman-host.exe ctl`, on
# this port, as it has no other way in: "guesthouse: ctl ask ID WORD...". The guest runs it and says
# the answer back in base64, in numbered lines of its own, "guesthouse: ctl answer ID CODE N/TOTAL
# CHUNK", or "guesthouse: ctl failed ID REASON" when it ran nothing. The answers go to the port only,
# never to the report's file, as they can hold the names of a person's devices and pairing codes.
# Only the words the host may send are run, by the grammar below, which the host checks too: every
# word is lower-case letters, digits and '-', a device is named by its fingerprint, nothing is ever
# quoted, and never `sharing off` or `drop`, which would cut Guesthouse's own viewer. The tests in
# guest-scripts run this block on its own.
function Test-CtlWords {
    param([string[]]$Words)
    if (-not $Words -or $Words.Count -eq 0) { return $false }
    foreach ($word in $Words) {
        if ($word -cnotmatch '\A[0-9a-z-]+\z') { return $false }
    }
    $fp = '[0-9a-f]{64}'
    $grammar = "\A(status|pair (on|off)|pair (accept|decline) $fp|revoke $fp|device $fp (allow (clipboard|files|sound|mic|camera) (on|off)|view-only (on|off))|allow (clipboard|files) (on|off)|set updates off|sharing on)\z"
    return ($Words -join ' ') -cmatch $grammar
}

# The request on a line, if it is one with a number to answer: its number, its words and whether
# they may be run. Nothing for any other line.
function Read-CtlRequest {
    param([string]$Line)
    $ask = 'guesthouse: ctl ask '
    $Line = $Line.TrimEnd([char]13, [char]10)
    if (-not $Line.StartsWith($ask, [StringComparison]::Ordinal)) { return $null }
    $parts = $Line.Substring($ask.Length).Split([char]' ')
    if ($parts[0] -cnotmatch '\A[1-9][0-9]{0,9}\z' -or [uint64]$parts[0] -gt [uint32]::MaxValue) { return $null }
    $words = [string[]]@($parts | Select-Object -Skip 1)
    return [pscustomobject]@{ Id = [uint32]$parts[0]; Words = $words; Valid = (Test-CtlWords -Words $words) }
}

# The lines that answer request $Id: what ctl said, in base64, 1024 characters to a line, so each
# line is far inside what the host takes, and up to 64 KB of it.
function Get-CtlAnswerLines {
    param([uint32]$Id, [int]$Code, [byte[]]$Bytes)
    if ($null -eq $Bytes) { $Bytes = [byte[]]@() }
    if ($Bytes.Length -gt 65536) { return , "guesthouse: ctl failed $Id toolong" }
    $text = [Convert]::ToBase64String($Bytes)
    $chunks = New-Object -TypeName System.Collections.Generic.List[string]
    for ($at = 0; $at -lt $text.Length; $at += 1024) {
        $chunks.Add($text.Substring($at, [Math]::Min(1024, $text.Length - $at)))
    }
    if ($chunks.Count -eq 0) { $chunks.Add('-') }
    $total = $chunks.Count
    for ($n = 0; $n -lt $total; $n += 1) { "guesthouse: ctl answer $Id $Code $($n + 1)/$total $($chunks[$n])" }
}

# Runs $Program with $Arguments for up to $Milliseconds: its exit code and everything it said, as
# UTF-8, or why it didn't run. The arguments are joined into its command line, with no shell to read
# it, and each is first checked to be letters, digits and . / - only, so each is one argument as the
# program reads its command line, with nothing to quote. Windows PowerShell's .NET has no list of
# arguments to start a process with.
function Invoke-Ctl {
    param([string]$Program, [string[]]$Arguments, [int]$Milliseconds)
    foreach ($argument in $Arguments) {
        if ($argument -cnotmatch '\A[0-9A-Za-z./-]+\z') { return [pscustomobject]@{ Failure = 'refused' } }
    }
    if (-not (Test-Path -LiteralPath $Program -PathType Leaf)) { return [pscustomobject]@{ Failure = 'missing' } }
    $start = New-Object -TypeName System.Diagnostics.ProcessStartInfo
    $start.FileName = $Program
    $start.Arguments = $Arguments -join ' '
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.StandardOutputEncoding = [System.Text.Encoding]::UTF8
    $start.StandardErrorEncoding = [System.Text.Encoding]::UTF8
    try { $process = [System.Diagnostics.Process]::Start($start) } catch { return [pscustomobject]@{ Failure = 'error' } }
    # Read as it comes, so a long answer can't fill the pipe and hold the program up.
    $out = $process.StandardOutput.ReadToEndAsync()
    $err = $process.StandardError.ReadToEndAsync()
    if (-not $process.WaitForExit($Milliseconds)) {
        try { $process.Kill() } catch { }
        return [pscustomobject]@{ Failure = 'timeout' }
    }
    $process.WaitForExit()
    $said = $out.Result + $err.Result
    return [pscustomobject]@{ Failure = $null; Code = $process.ExitCode; Bytes = [System.Text.Encoding]::UTF8.GetBytes($said) }
}

# Writes an answer's lines together, holding the port's lock.
function Write-CtlLines {
    param($Port, $Lock, [string[]]$Lines)
    [System.Threading.Monitor]::Enter($Lock)
    try {
        foreach ($line in $Lines) { $Port.WriteLine($line) }
    } finally {
        [System.Threading.Monitor]::Exit($Lock)
    }
}

# remoteman's host answers whoever waits on a plain `pair accept` or `pair decline`, so the device
# named is read from the host's status straight before, and only when it is the one waiting is the
# answer sent. Otherwise nothing is run and the request is answered "another device is waiting now".
function Invoke-CtlAnswer {
    param([string]$Program, [string]$Answer, [string]$Fingerprint, [int]$Milliseconds)
    $status = Invoke-Ctl -Program $Program -Arguments @('ctl', '--port', '7791', 'status') -Milliseconds $Milliseconds
    if ($status.Failure) { return $status }
    $waiting = $null
    $said = @([System.Text.Encoding]::UTF8.GetString($status.Bytes) -split "`n" | ForEach-Object { $_.Trim() } | Where-Object { $_.StartsWith('{') })
    if ($said.Count -gt 0) {
        try { $pending = ($said[0] | ConvertFrom-Json).pending; if ($pending) { $waiting = [string]$pending.fingerprint } } catch { }
    }
    if (-not $waiting -or $waiting.ToLowerInvariant() -cne $Fingerprint) {
        return [pscustomobject]@{ Failure = $null; Code = 1; Bytes = [System.Text.Encoding]::UTF8.GetBytes('another device is waiting now') }
    }
    return Invoke-Ctl -Program $Program -Arguments @('ctl', '--port', '7791', 'pair', $Answer) -Milliseconds $Milliseconds
}

# Answers $Line if it is a request: true when it was one. remoteman's host is given 10 s in all, and the
# host waits longer than that for the answer.
function Receive-CtlLine {
    param([string]$Line, $Port, $Lock, [string]$Program)
    $request = Read-CtlRequest -Line $Line
    if (-not $request) { return $false }
    if (-not $request.Valid) {
        $lines = @("guesthouse: ctl failed $($request.Id) refused")
    } else {
        if ($request.Words[0] -ceq 'pair' -and $request.Words.Count -eq 3) {
            $ran = Invoke-CtlAnswer -Program $Program -Answer $request.Words[1] -Fingerprint $request.Words[2] -Milliseconds 5000
        } else {
            $ran = Invoke-Ctl -Program $Program -Arguments (@('ctl', '--port', '7791') + $request.Words) -Milliseconds 10000
        }
        if ($ran.Failure) {
            $lines = @("guesthouse: ctl failed $($request.Id) $($ran.Failure)")
        } else {
            $lines = @(Get-CtlAnswerLines -Id $request.Id -Code $ran.Code -Bytes $ran.Bytes)
        }
    }
    Write-CtlLines -Port $Port -Lock $Lock -Lines $lines
    return $true
}

# remoteman's host's program, where its installer puts it (screen.ps1 has the same).
$ctlHost = 'C:\Program Files\remoteman host\remoteman-host.exe'
# The channel's functions as text, for the listener's thread, which starts with none of this script's.
$ctlCode = (('Test-CtlWords', 'Read-CtlRequest', 'Get-CtlAnswerLines', 'Invoke-Ctl', 'Invoke-CtlAnswer', 'Write-CtlLines', 'Receive-CtlLine') |
    ForEach-Object { "function $_ {$((Get-Item -LiteralPath "function:$_").Definition)}" }) -join "`n"
# ctl channel: end

Send-Line "started $(Get-Date -Format o)"

# The host asks the guest to shut down on this port, as HCS's own request needs Hyper-V's
# integration services, which a machine HCS makes doesn't have. The port is listened to on a thread
# of its own from now on, while the report goes on, and for as long as the guest runs. Asked, the
# guest shuts down as Windows does from Start, with programs closed if they don't close. The host's
# remoteman requests are answered on the same thread, one at a time.
$listener = $null
if ($port) {
    $listener = [powershell]::Create()
    [void]$listener.AddScript({
        param($port, $portLock, $ctlCode, $ctlHost)
        . ([scriptblock]::Create($ctlCode))
        while ($true) {
            try { $line = $port.ReadLine() } catch [TimeoutException] { continue } catch { return }
            if ($line.Trim() -eq 'guesthouse: shutdown') {
                [System.Threading.Monitor]::Enter($portLock)
                try { $port.WriteLine('guesthouse: shutting down, as the host asked') } catch { } finally { [System.Threading.Monitor]::Exit($portLock) }
                & shutdown.exe /s /f /t 0 /d p:0:0 /c 'Guesthouse: the host asked this machine to shut down'
                return
            }
            try { [void](Receive-CtlLine -Line $line -Port $port -Lock $portLock -Program $ctlHost) } catch { }
        }
    }).AddArgument($port).AddArgument($portLock).AddArgument($ctlCode).AddArgument($ctlHost)
    [void]$listener.BeginInvoke()
}

# The card's driver can take a while to start after Windows does.
$waitedSeconds = 0
while ($waitedSeconds -lt 120) {
    $cards = @(Get-CimInstance -ClassName Win32_VideoController | Where-Object { $_.Name -notlike 'Microsoft*' })
    if ($cards.Count -gt 0) { break }
    Start-Sleep -Seconds 5
    $waitedSeconds += 5
}

foreach ($card in Get-CimInstance -ClassName Win32_VideoController) {
    Send-Line "card: $($card.Name), driver $($card.DriverVersion), status $($card.Status)"
}

$store = 'C:\Windows\System32\HostDriverStore\FileRepository'
foreach ($folder in Get-ChildItem -LiteralPath $store -Directory -ErrorAction SilentlyContinue) {
    Send-Line "host driver: $($folder.Name)"
}

$smi = Get-ChildItem -LiteralPath $store -Recurse -Filter 'nvidia-smi.exe' -ErrorAction SilentlyContinue |
    Select-Object -First 1
if ($smi) {
    foreach ($line in (& $smi.FullName -L 2>&1)) { Send-Line "nvidia-smi: $line" }
}

foreach ($address in Get-NetIPAddress -AddressFamily IPv4 -ErrorAction SilentlyContinue) {
    if ($address.IPAddress -ne '127.0.0.1') { Send-Line "address: $($address.IPAddress)" }
}

# Who may reach the ports Guesthouse lets in here, Remote Desktop's and the screen's: this PC alone.
# The host writes hosts.txt before each start: the network the machine is on, shared or bridged,
# then this PC's addresses, a line each. On the shared network this PC is the guest's default gateway
# too, and that is the address its port forwards reach the guest from; the host may not know it, as
# Windows makes that network's adapter as the machine starts. With no file, a network it doesn't
# know or no address, nobody is let in.
$hostsFile = 'C:\Guesthouse\hosts.txt'

function Get-HostAddresses([string]$File, [string[]]$Gateways) {
    if (-not (Test-Path -LiteralPath $File -PathType Leaf)) { return @() }
    $lines = @(Get-Content -LiteralPath $File | ForEach-Object { $_.Trim() } | Where-Object { $_ })
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

# Who may reach Remote Desktop: $Allowed, and on a bridged network the rest of the network the guest
# is on too, as there other devices reach it directly rather than through a forward.
function Get-DesktopAddresses([string]$File, [string[]]$Allowed) {
    $Allowed = @($Allowed | Where-Object { $_ })
    if (-not (Test-Path -LiteralPath $File -PathType Leaf)) { return $Allowed }
    $first = @(Get-Content -LiteralPath $File | ForEach-Object { $_.Trim() } | Where-Object { $_ } | Select-Object -First 1)
    if ($first.Count -and $first[0] -eq 'bridged') { return @($Allowed + 'LocalSubnet') }
    return $Allowed
}

# The guest's default gateways. When it has a network adapter and no gateway yet, as its address
# can come late, it waits up to $Seconds for one.
function Get-Gateways([int]$Seconds) {
    for ($waited = 0; ; $waited += 2) {
        $found = @(Get-NetRoute -DestinationPrefix '0.0.0.0/0' -ErrorAction SilentlyContinue |
            ForEach-Object { $_.NextHop } | Where-Object { $_ -and $_ -ne '0.0.0.0' })
        if ($found.Count -or $waited -ge $Seconds -or -not @(Get-NetAdapter -ErrorAction SilentlyContinue).Count) {
            return $found
        }
        Start-Sleep -Seconds 2
    }
}

# Makes the inbound rule $Name, or puts it right, so it lets $Protocol port $Port in from $Allowed
# alone, on every profile. It is made with its addresses, never open first. With no address the rule
# is off, and a rule that can't be set is turned off. Says what it did.
function Set-HostRule([string]$Name, [string]$Protocol, [int]$Port, [string[]]$Allowed) {
    $Allowed = @($Allowed | Where-Object { $_ })
    try {
        $there = @(Get-NetFirewallRule -DisplayName $Name -ErrorAction SilentlyContinue).Count -gt 0
        if ($Allowed.Count -eq 0) {
            if ($there) { Disable-NetFirewallRule -DisplayName $Name -ErrorAction Stop }
            return "$Name is off, as no address of this PC's is known"
        }
        if ($there) {
            Set-NetFirewallRule -DisplayName $Name -Direction Inbound -Action Allow -Protocol $Protocol -LocalPort $Port `
                -RemoteAddress $Allowed -Profile Any -Enabled True -ErrorAction Stop
        } else {
            New-NetFirewallRule -DisplayName $Name -Direction Inbound -Action Allow -Protocol $Protocol -LocalPort $Port `
                -RemoteAddress $Allowed -Profile Any -Enabled True -ErrorAction Stop | Out-Null
        }
        "$Name lets $Protocol $Port in from $($Allowed -join ', ')"
    } catch {
        $why = $_.Exception.Message
        try {
            if (@(Get-NetFirewallRule -DisplayName $Name -ErrorAction SilentlyContinue).Count) {
                Disable-NetFirewallRule -DisplayName $Name -ErrorAction Stop
            }
            "$Name is off, as it couldn't be set: $why"
        } catch {
            "$Name couldn't be set or turned off: $why; $($_.Exception.Message)"
        }
    }
}

# Lets only $Allowed in by the rules called $Name that another program makes, as remoteman's
# installer does. With no address, or when they can't be set, they are turned off.
function Limit-HostRule([string]$Name, [string[]]$Allowed) {
    $Allowed = @($Allowed | Where-Object { $_ })
    if (-not @(Get-NetFirewallRule -DisplayName $Name -ErrorAction SilentlyContinue).Count) { return "$Name isn't there" }
    try {
        if ($Allowed.Count -eq 0) { throw "no address of this PC's is known" }
        Set-NetFirewallRule -DisplayName $Name -RemoteAddress $Allowed -ErrorAction Stop
        "$Name lets in only $($Allowed -join ', ')"
    } catch {
        $why = $_.Exception.Message
        try {
            Disable-NetFirewallRule -DisplayName $Name -ErrorAction Stop
            "$Name is off: $why"
        } catch {
            "$Name couldn't be set or turned off: $why; $($_.Exception.Message)"
        }
    }
}

# Remote Desktop, which SetupComplete.cmd turns on, by Guesthouse's own rule for TCP 3389 from the
# addresses Get-DesktopAddresses gives. Windows' own rules for it, which let every address in, are turned off once, found by
# their group's resource name, which is the same in every language. $Note says that was done, so one
# turned on again by hand later is left as it is. A group Windows doesn't have counts as done; any
# other failure to read it is tried again at the next start.
function Set-RemoteDesktop([string[]]$Allowed, [string]$Rule, [string]$Note) {
    if (-not (Test-Path -LiteralPath $Note)) {
        $group = '@FirewallAPI.dll,-28752'
        try {
            $read = @(Get-NetFirewallRule -Group $group -ErrorAction SilentlyContinue -ErrorVariable missed)
            $failed = @($missed | Where-Object { $_.CategoryInfo.Category -ne 'ObjectNotFound' })
            if ($failed.Count) { throw $failed[0] }
            $on = @($read | Where-Object { $_.Enabled -eq 'True' })
            if ($on.Count) { Disable-NetFirewallRule -Group $group -ErrorAction Stop }
            Set-Content -LiteralPath $Note -Value "Windows' own rules turned off" -Encoding ascii
            "Windows' own rules are off ($($on.Count) were on)"
        } catch {
            "Windows' own rules couldn't be turned off: $($_.Exception.Message)"
        }
    }
    Set-HostRule $Rule 'TCP' 3389 $Allowed
}

$hostAddresses = @(Get-HostAddresses $hostsFile (Get-Gateways 30))
$desktopAddresses = @(Get-DesktopAddresses $hostsFile $hostAddresses)
foreach ($said in @(Set-RemoteDesktop $desktopAddresses 'Guesthouse Remote Desktop' 'C:\Guesthouse\remote-desktop.txt')) {
    Send-Line "remote desktop: $said"
}

# The guest's clock starts out twelve hours behind the host's: its hours are read as if the clock
# kept the time on a twelve-hour dial. Windows' time service puts it right from the internet's time,
# once the guest is on the network, and is let make a correction that large, which a workgroup
# machine otherwise only makes up to fifteen hours. A guest with no network keeps the wrong time.
$timeSync = (Get-Service -Name vmictimesync -ErrorAction SilentlyContinue).Status
Send-Line "clock: $([DateTime]::UtcNow.ToString('o')) UTC by the guest, Hyper-V time sync $timeSync"
$config = 'HKLM:\SYSTEM\CurrentControlSet\Services\W32Time\Config'
Set-ItemProperty -LiteralPath $config -Name MaxPosPhaseCorrection -Value 0xFFFFFFFF -Type DWord -ErrorAction SilentlyContinue
Set-ItemProperty -LiteralPath $config -Name MaxNegPhaseCorrection -Value 0xFFFFFFFF -Type DWord -ErrorAction SilentlyContinue
Set-Service -Name w32time -StartupType Automatic -ErrorAction SilentlyContinue
Start-Service -Name w32time -ErrorAction SilentlyContinue
& w32tm.exe /config /manualpeerlist:'time.windows.com,0x9' /syncfromflags:manual /update 2>&1 | Out-Null
$synced = (& w32tm.exe /resync /force 2>&1 | Out-String).Trim() -replace '\s+', ' '
Send-Line "clock: $([DateTime]::UtcNow.ToString('o')) UTC after asking the time service: $synced"

# The disk can be made larger while the machine is stopped, so Windows' partition, the last on it,
# takes whatever room it has at every start.
try {
    $most = (Get-PartitionSupportedSize -DriveLetter C -ErrorAction Stop).SizeMax
    $now = (Get-Partition -DriveLetter C -ErrorAction Stop).Size
    if ($most - $now -gt 64MB) {
        Resize-Partition -DriveLetter C -Size $most -ErrorAction Stop
        Send-Line "disk: C: grown to $([math]::Round($most / 1GB)) GB"
    }
} catch {
    Send-Line "disk: C: couldn't grow: $($_.Exception.Message)"
}

# The host's shared folders, when it gives any, are drives of their own. The host serves them over
# Plan 9 on a Hyper-V socket, each by its own name on the one port, and Guesthouse's own program
# (guesthouse-guest-share.exe, which the host puts in C:\Guesthouse\share with WinFsp's installer)
# mounts each with WinFsp: one copy a folder, each run from a task of its own, as SYSTEM from the
# start, and started again if it ends. The host writes C:\Guesthouse\share.txt at each boot when the
# machine has shares, a line each, "NAME PORT rw" or "NAME PORT ro" (shared 50001 rw). NAME is
# "shared" for the first folder of the machine's settings and "sharedN" for the Nth after it, and
# that number names the folder's task, log and desktop link here, the first's with none, as when
# there could be only one. A folder the file doesn't name loses its task, and so its drive, and the
# link on everyone's desktop. Only a link is ever deleted, never what it points to.
$shareFile = 'C:\Guesthouse\share.txt'
$shareFolder = 'C:\Guesthouse\share'
$shareProgram = Join-Path $shareFolder 'guesthouse-guest-share.exe'
$shareInstaller = Join-Path $shareFolder 'winfsp.msi'
$shareTaskPrefix = 'Guesthouse shared folder'
$desktop = [Environment]::GetFolderPath('CommonDesktopDirectory')
# The first version of this linked the share's folder itself under this name, with a symbolic link.
$oldShareLink = Join-Path $desktop 'Shared folder'
$booted = (Get-CimInstance Win32_OperatingSystem).LastBootUpTime

function Remove-OldShareLink {
    $old = Get-Item -LiteralPath $oldShareLink -Force -ErrorAction SilentlyContinue
    if ($old -and $old.LinkType -eq 'SymbolicLink') {
        # Not recursive, so only the link goes.
        [System.IO.Directory]::Delete($oldShareLink, $false)
    }
}

# What the folder with this number has here: the first's names have no number.
function Get-ShareSlot([int]$Number) {
    $suffix = if ($Number -eq 1) { '' } else { " $Number" }
    $logName = if ($Number -eq 1) { 'share.log' } else { "share$Number.log" }
    [pscustomobject]@{
        Number = $Number
        Task   = "$shareTaskPrefix$suffix"
        Link   = Join-Path $desktop "Shared folder$suffix.lnk"
        Log    = Join-Path $shareFolder $logName
    }
}

# WinFsp is installed when its registry key says where, as its own installer writes it. Only its
# core feature is wanted, with no reboot, which a guest nobody is watching can't be asked for.
function Test-WinFsp {
    [bool](Get-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\WOW6432Node\WinFsp' -ErrorAction SilentlyContinue).InstallDir
}

# The drive a folder's program has mounted it at, "S:", if it has this boot: its log was written
# since the guest started and its last line says "mounted at S:", which the program writes last
# once the drive is there. A log from an earlier boot could name a drive another folder has now.
function Get-ShareDrive($Slot) {
    $log = Get-Item -LiteralPath $Slot.Log -ErrorAction SilentlyContinue
    if (-not $log -or $log.LastWriteTime -lt $booted) { return $null }
    $last = Get-Content -LiteralPath $Slot.Log -Tail 1 -ErrorAction SilentlyContinue
    if ($last -match '^mounted at ([A-Za-z]:)$') {
        $drive = $Matches[1].ToUpperInvariant()
        if (Test-Path -LiteralPath "$drive\") { return $drive }
    }
    return $null
}

# What a folder's program is run with. Each prefers its own letter, the first S: and the Nth the Nth
# from it, so they keep their drives whichever of their tasks Windows starts first at boot.
function Get-ShareArguments($Share) {
    $prefer = [char]([int][char]'S' + $Share.Slot.Number - 1)
    $arguments = "--port $($Share.Port) --share $($Share.Name) --prefer ${prefer}:"
    if ($Share.ReadOnly) { $arguments += ' --read-only' }
    "$arguments --log $($Share.Slot.Log)"
}

# Puts the folder's task right whenever it isn't what's wanted, as the report's own task is, which
# stops a copy running as it was. Only then, unless -NoStart, starts it if it isn't running. Before
# a copy is started, a line in its log says so, so a "mounted at" an earlier copy wrote this boot,
# with a drive another folder may have now, isn't taken for this one's.
function Set-ShareTask($Slot, [string]$Arguments, [switch]$NoStart) {
    $task = Get-ScheduledTask -TaskName $Slot.Task -ErrorAction SilentlyContinue
    $wrong = -not $task -or
        $task.Actions[0].Execute -ne $shareProgram -or $task.Actions[0].Arguments -ne $Arguments -or
        -not ($task.Triggers | Where-Object { $_.CimClass.CimClassName -eq 'MSFT_TaskBootTrigger' -and $_.Repetition.Interval -eq 'PT1M' }) -or
        $task.Principal.UserId -ne 'SYSTEM' -or
        $task.Settings.DisallowStartIfOnBatteries -or $task.Settings.StopIfGoingOnBatteries -or
        $task.Settings.ExecutionTimeLimit -ne 'PT0S' -or $task.Settings.RestartCount -ne 999
    if ($wrong) {
        if ($task) {
            Stop-ScheduledTask -TaskName $Slot.Task -ErrorAction SilentlyContinue
            Add-ShareLogLine $Slot 'stopped by the report to set its task right'
        }
        $action = New-ScheduledTaskAction -Execute $shareProgram -Argument $Arguments
        # Run again every minute from boot, which starts the program only when no copy runs, as a
        # new run is ignored while one does: the task's restart on failure doesn't restart a
        # program that crashed, and its drive would be gone until the next boot.
        $trigger = New-ScheduledTaskTrigger -AtStartup
        $trigger.Repetition = (New-ScheduledTaskTrigger -Once -At (Get-Date) -RepetitionInterval (New-TimeSpan -Minutes 1)).Repetition
        $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew `
            -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1)
        $principal = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
        Register-ScheduledTask -TaskName $Slot.Task -Action $action -Trigger $trigger -Settings $settings -Principal $principal -Force | Out-Null
    }
    if (-not $NoStart -and (Get-ScheduledTask -TaskName $Slot.Task).State -ne 'Running') {
        Add-ShareLogLine $Slot 'started by the report'
        Start-ScheduledTask -TaskName $Slot.Task
    }
}

function Add-ShareLogLine($Slot, [string]$Text) {
    try {
        New-Item -ItemType Directory -Force -Path $shareFolder | Out-Null
        Add-Content -LiteralPath $Slot.Log -Value "report: $Text" -Encoding ascii
    } catch {}
}

# The folders the host gives, each with its number. A line that isn't one is said and passed over.
$shares = @()
if (Test-Path -LiteralPath $shareFile) {
    foreach ($line in @(Get-Content -LiteralPath $shareFile)) {
        if (-not $line.Trim()) { continue }
        $fields = @($line.Trim() -split '\s+')
        if ($fields.Count -ne 3 -or $fields[0] -notmatch '^shared(\d*)$' -or $fields[1] -notmatch '^\d+$' -or $fields[2] -notin 'rw', 'ro') {
            Send-Line "share: $shareFile has '$line', which isn't 'NAME PORT rw' or 'NAME PORT ro'"
            continue
        }
        $null = $fields[0] -match '^shared(\d*)$'
        $number = if ($Matches[1]) { [int]$Matches[1] } else { 1 }
        if ($number -lt 1) { continue }
        $shares += [pscustomobject]@{ Name = $fields[0]; Port = $fields[1]; ReadOnly = $fields[2] -eq 'ro'; Slot = Get-ShareSlot $number }
    }
}

# What's here of a folder no longer given goes: its task, which unmounts its drive, and its link.
$kept = @($shares | ForEach-Object { $_.Slot.Task })
foreach ($task in @(Get-ScheduledTask -TaskName "$shareTaskPrefix*" -ErrorAction SilentlyContinue)) {
    if ($task.TaskName -notmatch "^$([regex]::Escape($shareTaskPrefix))( \d+)?$" -or $task.TaskName -in $kept) { continue }
    Stop-ScheduledTask -TaskName $task.TaskName -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName $task.TaskName -Confirm:$false -ErrorAction SilentlyContinue
}
$keptLinks = @($shares | ForEach-Object { $_.Slot.Link })
foreach ($link in @(Get-ChildItem -LiteralPath $desktop -Filter 'Shared folder*.lnk' -Force -ErrorAction SilentlyContinue)) {
    if ($link.Name -notmatch '^Shared folder( \d+)?\.lnk$' -or $link.FullName -in $keptLinks) { continue }
    Remove-Item -LiteralPath $link.FullName -Force -ErrorAction SilentlyContinue
}

if ($shares.Count) {
    $ready = $true
    try {
        if (-not (Test-Path -LiteralPath $shareProgram)) {
            throw "$shareProgram isn't there, so the host had none to give"
        }
        if (-not (Test-WinFsp)) {
            if (-not (Test-Path -LiteralPath $shareInstaller)) {
                throw "WinFsp isn't installed, and $shareInstaller isn't there, so the host had none to give"
            }
            $install = Start-Process -FilePath 'msiexec.exe' -Wait -PassThru -ArgumentList @(
                '/i', $shareInstaller, '/qn', '/norestart', 'ADDLOCAL=F.Main,F.User')
            # 3010 is success, with a restart WinFsp doesn't need for what's done here.
            Send-Line "share: WinFsp's installer ended with $($install.ExitCode)"
            if (-not (Test-WinFsp)) { throw "WinFsp isn't installed (its installer ended with $($install.ExitCode))" }
        }
    } catch {
        $ready = $false
        foreach ($share in $shares) {
            Send-Line "share: $($share.Name) not mounted: $($_.Exception.Message)"
            Remove-Item -LiteralPath $share.Slot.Link -Force -ErrorAction SilentlyContinue
        }
    }

    # One after another, in the settings' order, so the first folder takes the first free drive
    # from S: and the next the one after. A minute is waited for them all, not for each, so the
    # report isn't held long when the host isn't serving them; a folder started after the minute
    # still mounts, and its link is made at the next start.
    # Every task is put right before any is started, so one still running as it was before can't
    # hold the drive another prefers now.
    foreach ($share in $(if ($ready) { $shares } else { @() })) {
        try { Set-ShareTask $share.Slot (Get-ShareArguments $share) -NoStart } catch {}
    }
    $waited = [Diagnostics.Stopwatch]::StartNew()
    foreach ($share in $(if ($ready) { $shares } else { @() })) {
        try {
            Set-ShareTask $share.Slot (Get-ShareArguments $share)

            # The program says "mounted at S:" in its log once the drive is there.
            $drive = Get-ShareDrive $share.Slot
            while (-not $drive -and $waited.Elapsed.TotalSeconds -lt 60) {
                Start-Sleep -Seconds 1
                $drive = Get-ShareDrive $share.Slot
            }
            if (-not $drive) {
                $tail = @(Get-Content -LiteralPath $share.Slot.Log -Tail 5 -ErrorAction SilentlyContinue) -join ' | '
                throw "no drive within a minute; the program's log ends with: $tail"
            }
            $link = (New-Object -ComObject WScript.Shell).CreateShortcut($share.Slot.Link)
            $link.TargetPath = "$drive\"
            $link.Save()
            Send-Line "share: $($share.Name) at $drive"
        } catch {
            Send-Line "share: $($share.Name) not mounted: $($_.Exception.Message)"
            # A shortcut from an earlier start would lead nowhere.
            Remove-Item -LiteralPath $share.Slot.Link -Force -ErrorAction SilentlyContinue
        }
    }
}
Remove-OldShareLink

# Windows' copy of the answer file the host set it up with, which has the account's password in it.
# It is only needed until setup has finished. guest/windows/guest-rights.ps1 has the same.
function Remove-AnswerFile([string]$File, [string]$SetupKey) {
    $setup = Get-ItemProperty -LiteralPath $SetupKey -ErrorAction SilentlyContinue
    if (-not $setup -or $setup.SystemSetupInProgress -ne 0 -or $setup.OOBEInProgress -ne 0) { return }
    if (Test-Path -LiteralPath $File -PathType Leaf) {
        Remove-Item -LiteralPath $File -Force
        "took away $File"
    }
}
try {
    $said = Remove-AnswerFile 'C:\Windows\Panther\unattend.xml' 'HKLM:\SYSTEM\Setup'
    if ($said) { Send-Line "setup: $said" }
} catch { Send-Line "setup: the answer file stays: $($_.Exception.Message)" }

# The guest's screen, when the host gave it one, before the last line, as the host takes that line
# to mean the guest is set up.
$screenScript = 'C:\Guesthouse\screen\screen.ps1'
if (Test-Path -LiteralPath $screenScript) {
    try { . $screenScript } catch { Send-Line "screen: failed: $($_.Exception.Message)" }
}

Send-Line 'report done'
# The port stays open for the listener, so the host can still ask the guest to shut down. The task
# has no time limit, and this ends with the guest.
while ($listener) { Start-Sleep -Seconds 3600 }
if ($port) { $port.Close() }
