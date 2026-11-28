# Sent by the host through the agent, with $helperName first: clipboard, files, window-size or app-windows. It ends
# that helper. The helper's task runs conhost, which gives PowerShell a console with no window, and
# stopping the task ends conhost alone. The PowerShell under it would go on holding the helper's
# port, so the next helper could not listen. Its own PowerShell is ended too, found by its script.
# The app windows helper has put a style on every window it shows, and it takes that off again when it ends. It
# ends itself when it finds this file, so it is given a moment before it is stopped like the others.
if ($helperName -eq 'app-windows') {
    [IO.File]::WriteAllText('C:\gpupv\app-windows.quit', 'quit')
    for ($i = 0; $i -lt 20 -and (Get-CimInstance Win32_Process -Filter "Name = 'powershell.exe'" -ea 0 | Where-Object { $_.CommandLine -like '*C:\gpupv\app-windows.ps1*' }); $i++) { Start-Sleep -Milliseconds 100 }
}
Stop-ScheduledTask -TaskName "gpupv-$helperName" -ea 0
Get-CimInstance Win32_Process -Filter "Name = 'powershell.exe'" -ea 0 |
    Where-Object { $_.CommandLine -like "*C:\gpupv\$helperName.ps1*" } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ea 0 }
