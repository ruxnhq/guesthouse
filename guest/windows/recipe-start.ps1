# Sent by the host through the agent, which runs as SYSTEM. The host puts $data and $runner, both in
# Base64, first: the recipe's steps as JSON, and recipe-run.ps1. winget belongs to a user, so the steps
# run in a task of their own as the account that is logged in, and this returns at once, which leaves
# the agent free for everything else the host asks while they do.
$user = (Get-Process explorer -IncludeUserName -ea 0 | Select -First 1).UserName
if (-not $user) { 'nobody is logged in'; return }
$folder = 'C:\gpupv\recipe'
Stop-ScheduledTask -TaskName gpupv-recipe -ea 0
# The task runs conhost, and stopping it leaves the runner's PowerShell going. An earlier run is
# ended by the process it wrote down, with all it started, and only if that process is the runner:
# the number may since belong to another program.
$earlier = Get-Content -LiteralPath "$folder\run.pid" -ea 0 | Select-Object -First 1
if ("$earlier" -match '\A\d+\z') {
    $process = Get-CimInstance Win32_Process -Filter "ProcessId = $earlier" -ea 0
    if ($process -and $process.CommandLine -like '*-File C:\gpupv\recipe\run.ps1*') {
        & taskkill.exe /PID $earlier /T /F | Out-Null
    }
}
New-Item -ItemType Directory -Force $folder | Out-Null
Remove-Item "$folder\*" -Force -Recurse -ea 0
[IO.File]::WriteAllBytes("$folder\data.json", [Convert]::FromBase64String($data))
[IO.File]::WriteAllBytes("$folder\run.ps1", [Convert]::FromBase64String($runner))
$action = New-ScheduledTaskAction -Execute conhost.exe -Argument '--headless powershell.exe -NoProfile -ExecutionPolicy Bypass -File C:\gpupv\recipe\run.ps1'
$principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit 0
Register-ScheduledTask -TaskName gpupv-recipe -Action $action -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName gpupv-recipe
'started'
