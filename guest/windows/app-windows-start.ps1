# Sent by the host through the agent, with two things first, both in Base64 as with the clipboard's:
# $helper, app-windows.ps1, and $source, app-windows.cs. The helper listens for the host on a port of its
# own, which Windows' firewall opens to the host's end of QEMU's private network and to nothing else.
# It runs as the user with the user's ordinary rights, as the files helper does: a program the user
# starts from the app then has no more rights than one started from the Start menu. What this loses
# is a window of an elevated program, which an ordinary program cannot move, so it is shown where it
# is, and its events reach the helper only by the slow look every second.
$user = (Get-Process explorer -IncludeUserName -ea 0 | Select -First 1).UserName
if (-not $user) { 'nobody is logged in'; return }
# The helper reads the token as that account, which the token's own rules leave to SYSTEM and
# administrators until a helper's start adds the account.
icacls C:\gpupv\token.txt /grant "${user}:(R)" | Out-Null
if (-not (Get-NetFirewallRule -DisplayName 'gpupv-app-windows' -ea 0)) {
    New-NetFirewallRule -DisplayName 'gpupv-app-windows' -Direction Inbound -Protocol TCP -LocalPort 8769 -RemoteAddress 10.0.2.2 -Action Allow | Out-Null
}
[IO.File]::WriteAllBytes('C:\gpupv\app-windows.cs', [Convert]::FromBase64String($source))
[IO.File]::WriteAllBytes('C:\gpupv\app-windows.ps1', [Convert]::FromBase64String($helper))
$action = New-ScheduledTaskAction -Execute conhost.exe -Argument '--headless powershell.exe -NoProfile -STA -ExecutionPolicy Bypass -File C:\gpupv\app-windows.ps1'
$principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit 0
Register-ScheduledTask -TaskName gpupv-app-windows -Action $action -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName gpupv-app-windows
# It compiles the helper before it listens, which takes a few seconds in a guest.
for ($i = 0; $i -lt 80; $i++) {
    try { [System.Net.Sockets.TcpClient]::new('127.0.0.1', 8769).Close(); 'started'; return } catch { Start-Sleep -Milliseconds 250 }
}
'not listening'
