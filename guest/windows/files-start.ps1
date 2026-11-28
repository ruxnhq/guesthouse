# Sent by the host through the agent, with $helper, the helper in Base64, first, like the clipboard's.
# The helper listens for the host on a port of its own, which Windows' firewall opens to the host's
# end of QEMU's private network and to nothing else.
$user = (Get-Process explorer -IncludeUserName -ea 0 | Select -First 1).UserName
if (-not $user) { 'nobody is logged in'; return }
# The helper reads the token as that account, which the token's own rules leave to SYSTEM and
# administrators until a helper's start adds the account.
icacls C:\gpupv\token.txt /grant "${user}:(R)" | Out-Null
if (-not (Get-NetFirewallRule -DisplayName 'gpupv-files' -ea 0)) {
    New-NetFirewallRule -DisplayName 'gpupv-files' -Direction Inbound -Protocol TCP -LocalPort 8767 -RemoteAddress 10.0.2.2 -Action Allow | Out-Null
}
[IO.File]::WriteAllBytes('C:\gpupv\files.ps1', [Convert]::FromBase64String($helper))
$action = New-ScheduledTaskAction -Execute conhost.exe -Argument '--headless powershell.exe -NoProfile -STA -ExecutionPolicy Bypass -File C:\gpupv\files.ps1'
$principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit 0
Register-ScheduledTask -TaskName gpupv-files -Action $action -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName gpupv-files
'started'
