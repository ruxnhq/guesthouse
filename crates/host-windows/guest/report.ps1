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

# The host's shared folder, when it gives one, is linked on everyone's desktop. The host writes the
# file naming it at each boot, and takes the file away when the machine has no share, and then the
# link goes too. Only the link is ever deleted, never what it points to.
$shareFile = 'C:\Guesthouse\share.txt'
$shareLink = Join-Path ([Environment]::GetFolderPath('CommonDesktopDirectory')) 'Shared folder'
$linked = Get-Item -LiteralPath $shareLink -Force -ErrorAction SilentlyContinue
if (Test-Path -LiteralPath $shareFile) {
    $target = (Get-Content -LiteralPath $shareFile -TotalCount 1).Trim()
    if (Test-Path -LiteralPath $target) {
        if (-not $linked) {
            try {
                New-Item -ItemType SymbolicLink -Path $shareLink -Target $target -ErrorAction Stop | Out-Null
            } catch {
                Send-Line "share: the desktop's link couldn't be made: $($_.Exception.Message)"
            }
        }
        Send-Line "share: at $shareLink"
    } else {
        Send-Line "share: $target isn't reachable in this guest"
    }
} elseif ($linked -and $linked.LinkType -eq 'SymbolicLink') {
    # Not recursive, so only the link goes.
    [System.IO.Directory]::Delete($shareLink, $false)
}

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
