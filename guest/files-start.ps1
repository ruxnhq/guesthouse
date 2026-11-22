# Sent by the host through the agent, with $helper, the helper in Base64, first, like the clipboard's.
# The helper listens for the host on a port of its own, which Windows' firewall opens to the host's
# end of QEMU's private network and to nothing else.
$user = (Get-Process explorer -IncludeUserName -ea 0 | Select -First 1).UserName
if (-not $user) { 'nobody is logged in'; return }
if (-not (Get-NetFirewallRule -DisplayName 'gpupv-files' -ea 0)) {
    New-NetFirewallRule -DisplayName 'gpupv-files' -Direction Inbound -Protocol TCP -LocalPort 8767 -RemoteAddress 10.0.2.2 -Action Allow | Out-Null
}
# A helper from an earlier viewer may still be there, and Windows wouldn't start a second.
Stop-ScheduledTask -TaskName gpupv-files -ea 0
[IO.File]::WriteAllBytes('C:\gpupv\files.ps1', [Convert]::FromBase64String($helper))
$action = New-ScheduledTaskAction -Execute conhost.exe -Argument '--headless powershell.exe -NoProfile -STA -ExecutionPolicy Bypass -File C:\gpupv\files.ps1'
$principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit 0
Register-ScheduledTask -TaskName gpupv-files -Action $action -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName gpupv-files
'started'
