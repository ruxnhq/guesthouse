# Sent by the host through the agent once the desktop is up, on a machine with a shared folder.
# Windows' WebDAV client does the work. A drive belongs to one logon, so the mapping must run as the
# user, in the same way as the clipboard helper. It lasts until the guest shuts down.
$user = (Get-Process explorer -IncludeUserName -ea 0 | Select -First 1).UserName
if (-not $user) { 'nobody is logged in'; return }
# Windows refuses files over 50 MB here, to stop a web server filling the disk. This is the largest
# it allows, 4 GiB less a byte, and it needs a restart of the service.
$parameters = 'HKLM:\SYSTEM\CurrentControlSet\Services\WebClient\Parameters'
if ((Get-ItemProperty $parameters).FileSizeLimitInBytes -ne -1) {
    Set-ItemProperty $parameters FileSizeLimitInBytes -1 -Type DWord
    Stop-Service WebClient -Force -ea 0
}
Start-Service WebClient
# The host puts the share's address here. It holds the key the folder is served under, so the task
# that maps it is removed once it has run, and only the user's own logon has the drive.
$address = '@SHARE_ADDRESS@'
$key = ($address.TrimEnd('/') -split '/')[-1]
$map = "if (-not (net use | Select-String -SimpleMatch '$key')) { net use * $address /persistent:no }"
$action = New-ScheduledTaskAction -Execute conhost.exe -Argument "--headless powershell.exe -NoProfile -Command `"$map`""
$principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive
Register-ScheduledTask -TaskName gpupv-share -Action $action -Principal $principal -Force | Out-Null
Start-ScheduledTask -TaskName gpupv-share
for ($i = 0; $i -lt 40 -and (Get-ScheduledTask gpupv-share).State -ne 'Ready'; $i++) { Start-Sleep -Milliseconds 500 }
$result = (Get-ScheduledTaskInfo gpupv-share).LastTaskResult
Unregister-ScheduledTask -TaskName gpupv-share -Confirm:$false -ea 0
if ($result -eq 0) { 'mapped' } else { 'not mapped' }
