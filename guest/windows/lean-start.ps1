# Sent by the host through the agent, which runs as SYSTEM. The host puts $data and $runner, both in
# Base64, first: lean Windows' list as JSON, and lean.ps1. Taking packages out can take longer than the
# agent waits for an answer, so the script runs in a task of its own, also as SYSTEM, and this returns
# at once. The host reads the status file the script writes.
$folder = 'C:\gpupv\lean'
Stop-ScheduledTask -TaskName gpupv-lean -ea 0
New-Item -ItemType Directory -Force $folder | Out-Null
Remove-Item "$folder\*" -Force -Recurse -ea 0
[IO.File]::WriteAllBytes("$folder\data.json", [Convert]::FromBase64String($data))
[IO.File]::WriteAllBytes("$folder\lean.ps1", [Convert]::FromBase64String($runner))
$action = New-ScheduledTaskAction -Execute conhost.exe -Argument '--headless powershell.exe -NoProfile -ExecutionPolicy Bypass -File C:\gpupv\lean\lean.ps1 -Data C:\gpupv\lean\data.json -Status C:\gpupv\lean\status.txt'
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Minutes 30)
Register-ScheduledTask -TaskName gpupv-lean -Action $action -User 'SYSTEM' -RunLevel Highest -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName gpupv-lean
'started'
