# Sent by the host through the agent. The host puts $helper, the helper in Base64, first, as with the
# clipboard's. It runs as the user with the user's ordinary rights: it only reads, and a program that
# runs as an administrator still shows its windows to one that doesn't.
$user = (Get-Process explorer -IncludeUserName -ea 0 | Select -First 1).UserName
if (-not $user) { 'nobody is logged in'; return }
[IO.File]::WriteAllBytes('C:\gpupv\windows.ps1', [Convert]::FromBase64String($helper))
$action = New-ScheduledTaskAction -Execute conhost.exe -Argument '--headless powershell.exe -NoProfile -ExecutionPolicy Bypass -File C:\gpupv\windows.ps1'
$principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit 0
Register-ScheduledTask -TaskName gpupv-windows -Action $action -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName gpupv-windows
# It compiles its calls to Windows before it listens, which took a second or two in a guest.
for ($i = 0; $i -lt 40; $i++) {
    try { [System.Net.Sockets.TcpClient]::new('127.0.0.1', 8769).Close(); 'started'; return } catch { Start-Sleep -Milliseconds 250 }
}
'not listening'
