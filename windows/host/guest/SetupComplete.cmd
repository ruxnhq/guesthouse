@echo off
rem Windows runs this once, as SYSTEM, when it has finished setting itself up on first start.
rem The report runs at every start from now on, and once now through the same task, which it
rem registers itself: see report.ps1.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File C:\Guesthouse\report.ps1 -Register
rem Remote Desktop, the way to see this guest's screen for now. Windows Home has no server for it,
rem so on Home this changes nothing. The report's own firewall rule lets it in from this PC alone.
reg add "HKLM\SYSTEM\CurrentControlSet\Control\Terminal Server" /v fDenyTSConnections /t REG_DWORD /d 0 /f
