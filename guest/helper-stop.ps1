# Sent by the host through the agent, with $helperName first: clipboard, files or window-size. It ends
# that helper. The helper's task runs conhost, which gives PowerShell a console with no window, and
# stopping the task ends conhost alone. The PowerShell under it would go on holding the helper's
# port, so the next helper could not listen. Its own PowerShell is ended too, found by its script.
Stop-ScheduledTask -TaskName "gpupv-$helperName" -ea 0
Get-CimInstance Win32_Process -Filter "Name = 'powershell.exe'" -ea 0 |
    Where-Object { $_.CommandLine -like "*C:\gpupv\$helperName.ps1*" } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ea 0 }
