# Sent by the host through the agent. The host puts $helper, the helper in Base64, first, as with the
# clipboard's. It runs as the user with the highest rights the account has, as the driver's control
# device is an administrator's.
$user = (Get-Process explorer -IncludeUserName -ea 0 | Select -First 1).UserName
if (-not $user) { 'nobody is logged in'; return }
# The helper reads the token as that account, which the token's own rules leave to SYSTEM and
# administrators until a helper's start adds the account.
icacls C:\gpupv\token.txt /grant "${user}:(R)" | Out-Null
[IO.File]::WriteAllBytes('C:\gpupv\window-size.ps1', [Convert]::FromBase64String($helper))
$action = New-ScheduledTaskAction -Execute conhost.exe -Argument '--headless powershell.exe -NoProfile -ExecutionPolicy Bypass -File C:\gpupv\window-size.ps1'
$principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit 0
Register-ScheduledTask -TaskName gpupv-window-size -Action $action -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName gpupv-window-size
# It compiles its calls to Windows before it listens, which took a second or two in a guest.
for ($i = 0; $i -lt 40; $i++) {
    try { [System.Net.Sockets.TcpClient]::new('127.0.0.1', 8768).Close(); 'started'; return } catch { Start-Sleep -Milliseconds 250 }
}
'not listening'
