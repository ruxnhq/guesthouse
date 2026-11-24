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

function Send-Line([string]$Text) {
    Add-Content -LiteralPath $report -Value $Text -Encoding utf8
    if ($port) {
        try { $port.WriteLine("guesthouse: $Text") } catch { $port = $null }
    }
}

Send-Line "started $(Get-Date -Format o)"

# The host asks the guest to shut down on this port, as HCS's own request needs Hyper-V's
# integration services, which a machine HCS makes doesn't have. The port is listened to on a thread
# of its own from now on, while the report goes on, and for as long as the guest runs. Asked, the
# guest shuts down as Windows does from Start, with programs closed if they don't close.
$listener = $null
if ($port) {
    $listener = [powershell]::Create()
    [void]$listener.AddScript({
        param($port)
        while ($true) {
            try { $line = $port.ReadLine() } catch [TimeoutException] { continue } catch { return }
            if ($line.Trim() -eq 'guesthouse: shutdown') {
                try { $port.WriteLine('guesthouse: shutting down, as the host asked') } catch { }
                & shutdown.exe /s /f /t 0 /d p:0:0 /c 'Guesthouse: the host asked this machine to shut down'
                return
            }
        }
    }).AddArgument($port)
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
# stops a copy running as it was. Only then, unless -NoStart, starts it if it isn't running.
function Set-ShareTask($Slot, [string]$Arguments, [switch]$NoStart) {
    $task = Get-ScheduledTask -TaskName $Slot.Task -ErrorAction SilentlyContinue
    $wrong = -not $task -or
        $task.Actions[0].Execute -ne $shareProgram -or $task.Actions[0].Arguments -ne $Arguments -or
        -not ($task.Triggers | Where-Object { $_.CimClass.CimClassName -eq 'MSFT_TaskBootTrigger' }) -or
        $task.Principal.UserId -ne 'SYSTEM' -or
        $task.Settings.DisallowStartIfOnBatteries -or $task.Settings.StopIfGoingOnBatteries -or
        $task.Settings.ExecutionTimeLimit -ne 'PT0S' -or $task.Settings.RestartCount -ne 999
    if ($wrong) {
        if ($task) { Stop-ScheduledTask -TaskName $Slot.Task -ErrorAction SilentlyContinue }
        $action = New-ScheduledTaskAction -Execute $shareProgram -Argument $Arguments
        $trigger = New-ScheduledTaskTrigger -AtStartup
        $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew `
            -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1)
        $principal = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
        Register-ScheduledTask -TaskName $Slot.Task -Action $action -Trigger $trigger -Settings $settings -Principal $principal -Force | Out-Null
    }
    if (-not $NoStart -and (Get-ScheduledTask -TaskName $Slot.Task).State -ne 'Running') {
        Start-ScheduledTask -TaskName $Slot.Task
    }
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
