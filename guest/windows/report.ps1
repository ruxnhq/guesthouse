# What a bug report takes from a running Windows guest, sent by the host when the tester asks for a
# report. It only reads: the logs Guesthouse's own guest pieces write, recent events from the display
# stack and about devices that failed to start, the display devices as Windows sees them, Windows'
# build, and whether a crash dump was written, by name, time and size. Never a dump's contents, memory
# or anyone's files.
# Each part starts with a line '=== name', which the host splits on. 'names' is the accounts and the
# computer's name, which the host takes out of the rest and never puts in the report.
$ErrorActionPreference = 'SilentlyContinue'
$mostLines = 2000
$mostEvents = 300
# An event's message goes on one line. Plug and Play's put the problem and its status on the lines
# after the first, so the whole message is kept, up to this many characters.
$mostMessage = 800
$eventDays = 7
# The logs Guesthouse's guest pieces write: setup, the NVIDIA driver files, the display driver's
# take-over and the mode and display checks.
$logs = [ordered]@{
    'setup-out.log' = 'C:\gpupv\setup-out.log'
    'setup.log' = 'C:\gpupv\setup.log'
    'nv-files.log' = 'C:\gpupv\nv-files.log'
    'display.log' = (Join-Path $env:ProgramData 'Guesthouse\display.log')
    'setmode.txt' = 'C:\gpupv\cap\setmode.txt'
    'display.txt' = 'C:\gpupv\cap\display.txt'
}
# Who writes about the display stack in the System log. 4101 is a driver that stopped answering and
# was recovered, the timeout and reset a device-removed error follows from.
$providers = @('Display', 'Microsoft-Windows-DxgKrnl', 'dxgkrnl', 'nvlddmkm', 'amdkmdag', 'amdwddmg', 'BasicDisplay', 'BasicRender')
# None of those says when Windows stops a device. Plug and Play does, in the System log: 411 is a
# device that had a problem starting, 219 a driver that failed to load. Its other events are about
# every device there is, so only these two are taken.
$pnpProvider = 'Microsoft-Windows-Kernel-PnP'
$pnpEvents = @(411, 219)
# Windows logs this 219 for the Guesthouse display at every boot, on a healthy guest too, so it says
# nothing and only misleads.
$usualPnp = 'WUDFRd*ROOT\DISPLAY\*0xC0000365'
# The graphics kernel writes its own logs, apart from System: where it says it reset or stopped an
# adapter. Every event in them is about the display stack. They're named with a hyphen where Event
# Viewer shows a slash.
$graphicsLogs = @('Microsoft-Windows-DxgKrnl-Admin', 'Microsoft-Windows-DxgKrnl-Operational')
$dumps = @('C:\Windows\MEMORY.DMP', 'C:\Windows\Minidump\*.dmp', 'C:\Windows\LiveKernelReports\*\*.dmp')

'=== names'
$env:COMPUTERNAME
(Get-CimInstance Win32_ComputerSystem).UserName -replace '^.*\\', ''
Get-LocalUser | Where-Object Enabled | ForEach-Object Name

foreach ($name in $logs.Keys) {
    "=== log $name"
    if (Test-Path -LiteralPath $logs[$name]) { Get-Content -LiteralPath $logs[$name] -Tail $mostLines }
    else { '(not there)' }
}

'=== events'
$since = (Get-Date).AddDays(-$eventDays)
$system = Get-WinEvent -FilterHashtable @{ LogName = 'System'; StartTime = $since } -MaxEvents 20000 |
    Where-Object {
        $providers -contains $_.ProviderName -or $_.ProviderName -like '*Guesthouse*' -or
            ($_.ProviderName -eq $pnpProvider -and $pnpEvents -contains $_.Id -and
                -not ($_.Id -eq 219 -and $_.Message -like "*$usualPnp*"))
    }
# A log Windows doesn't have stops the whole script with "The parameter is incorrect", whatever the
# error preference, and with it every part after this one, so each is tried on its own.
$graphics = foreach ($log in $graphicsLogs) {
    try { Get-WinEvent -FilterHashtable @{ LogName = $log; StartTime = $since } -MaxEvents $mostEvents } catch { }
}
@($system) + @($graphics) | Where-Object { $_ } | Sort-Object TimeCreated -Descending |
    Select-Object -First $mostEvents |
    ForEach-Object {
        $record = $_
        $lines = $record.Message -split "`r?`n" | ForEach-Object { $_.Trim() } | Where-Object { $_ }
        $message = $lines -join ' | '
        if ($message.Length -gt $mostMessage) { $message = $message.Substring(0, $mostMessage) }
        "$($record.TimeCreated.ToString('s')) $($record.ProviderName) $($record.Id) $($record.LevelDisplayName): $message"
    }

'=== adapter'
Get-CimInstance Win32_VideoController | ForEach-Object {
    "$($_.Name), driver $($_.DriverVersion) of $($_.DriverDate), $($_.AdapterCompatibility), status $($_.Status)"
}
# Every display device Plug and Play has, started or not, with its problem and the status its driver
# failed with, which say why Windows stopped it. A device with no problem has neither.
'--- display devices'
Get-PnpDevice -Class Display | ForEach-Object {
    $failed = (Get-PnpDeviceProperty -InstanceId $_.InstanceId -KeyName 'DEVPKEY_Device_ProblemStatus').Data
    $status = if ($null -ne $failed) { '0x{0:X8}' -f $failed } else { 'none' }
    "$($_.FriendlyName), $($_.InstanceId), $($_.Status), problem $($_.Problem) ($([int]$_.ConfigManagerErrorCode)), problem status $status"
}

'=== windows'
$os = Get-CimInstance Win32_OperatingSystem
$revision = (Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion').UBR
"$($os.Caption) $($os.Version).$revision"

'=== dumps'
foreach ($pattern in $dumps) {
    Get-ChildItem -Path $pattern -File | ForEach-Object { "$($_.FullName), $($_.LastWriteTime.ToString('s')), $($_.Length) bytes" }
}
'=== end'
