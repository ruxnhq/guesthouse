# What a bug report takes from a running Windows guest, sent by the host when the tester asks for a
# report. It only reads: the logs Guesthouse's own guest pieces write, recent System events from the
# display stack, the display adapter as Windows sees it, Windows' build, and whether a crash dump was
# written, by name, time and size. Never a dump's contents, memory or anyone's files.
# Each part starts with a line '=== name', which the host splits on. 'names' is the accounts and the
# computer's name, which the host takes out of the rest and never puts in the report.
$ErrorActionPreference = 'SilentlyContinue'
$mostLines = 2000
$mostEvents = 300
$mostMessage = 500
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
Get-WinEvent -FilterHashtable @{ LogName = 'System'; StartTime = $since } -MaxEvents 20000 |
    Where-Object { $providers -contains $_.ProviderName -or $_.ProviderName -like '*Guesthouse*' } |
    Select-Object -First $mostEvents |
    ForEach-Object {
        $message = (($_.Message -split "`n")[0]).Trim()
        if ($message.Length -gt $mostMessage) { $message = $message.Substring(0, $mostMessage) }
        "$($_.TimeCreated.ToString('s')) $($_.ProviderName) $($_.Id) $($_.LevelDisplayName): $message"
    }

'=== adapter'
Get-CimInstance Win32_VideoController | ForEach-Object {
    "$($_.Name), driver $($_.DriverVersion) of $($_.DriverDate), $($_.AdapterCompatibility), status $($_.Status)"
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
