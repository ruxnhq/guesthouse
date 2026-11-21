@echo off
rem Windows runs this once, as SYSTEM, when it has finished setting itself up on first start.
rem The report runs at every start from now on, and once now through the same task.
schtasks /create /tn "Guesthouse report" /sc onstart /ru SYSTEM /rl HIGHEST /f /tr "powershell.exe -NoProfile -ExecutionPolicy Bypass -File C:\Guesthouse\report.ps1"
schtasks /run /tn "Guesthouse report"
rem Remote Desktop, the way to see this guest's screen for now. Windows Home has no server for it,
rem so on Home these change nothing.
reg add "HKLM\SYSTEM\CurrentControlSet\Control\Terminal Server" /v fDenyTSConnections /t REG_DWORD /d 0 /f
netsh advfirewall firewall set rule group="remote desktop" new enable=Yes
