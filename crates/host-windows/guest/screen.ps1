# The guest's screen: remoteman's host, which shows its desktop to the remoteman viewer on the PC
# that runs it, encoded on the guest's card. report.ps1 runs this at every start, as SYSTEM, before
# its last line, so Send-Line is there. The host puts what it needs in C:\Guesthouse\screen before
# each boot: remoteman's host's files, the viewer's fingerprint, and which encoder suits the card.
#
# remoteman's own installer can't be used here: it starts by asking Windows to run it again
# elevated, and as SYSTEM with nobody signed in that never comes back. So its files are copied
# where it would put them, and its setup.ps1, which does every step the installer does, is run.
$screen = 'C:\Guesthouse\screen'
$files = Join-Path $screen 'remoteman-host'
$folder = 'C:\Program Files\remoteman host'
$hostExe = Join-Path $folder 'remoteman-host.exe'
$state = 'C:\ProgramData\remoteman\host'
# remoteman keeps a host's identity and trust list in host\ under the folder it's given.
$identity = Join-Path $state 'host'
$service = 'remoteman host'

# Signing in by itself at every start, so the screen opens on the desktop and not on a sign-in
# screen that would ask the viewer for the password. The host writes the account here once, and it
# goes as soon as Windows has it. It takes effect from the next start.
$logon = Join-Path $screen 'logon.txt'
if (Test-Path -LiteralPath $logon) {
    $lines = Get-Content -LiteralPath $logon
    $user = (($lines | Where-Object { $_ -like 'user: *' }) -replace '^user: ', '').Trim()
    $password = (($lines | Where-Object { $_ -like 'password: *' }) -replace '^password: ', '').Trim()
    if ($user -and $password) {
        $winlogon = 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon'
        Set-ItemProperty -LiteralPath $winlogon -Name AutoAdminLogon -Value '1'
        Set-ItemProperty -LiteralPath $winlogon -Name DefaultUserName -Value $user
        Set-ItemProperty -LiteralPath $winlogon -Name DefaultPassword -Value $password
        Set-ItemProperty -LiteralPath $winlogon -Name DefaultDomainName -Value $env:COMPUTERNAME
        Remove-ItemProperty -LiteralPath $winlogon -Name AutoLogonCount -ErrorAction SilentlyContinue
        # Windows 11 hides signing in with a password until this is 0.
        $passwordless = 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\PasswordLess\Device'
        New-Item -Path $passwordless -Force | Out-Null
        Set-ItemProperty -LiteralPath $passwordless -Name DevicePasswordLessBuildVersion -Value 0 -Type DWord
        # No lock screen, and no locking or sleeping when nobody types, as a locked guest shows the
        # viewer a sign-in screen.
        $personalization = 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\Personalization'
        New-Item -Path $personalization -Force | Out-Null
        Set-ItemProperty -LiteralPath $personalization -Name NoLockScreen -Value 1 -Type DWord
        $system = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System'
        Set-ItemProperty -LiteralPath $system -Name InactivityTimeoutSecs -Value 0 -Type DWord
        powercfg.exe /change monitor-timeout-ac 0 | Out-Null
        powercfg.exe /change standby-timeout-ac 0 | Out-Null
        Send-Line "screen: $user signs in by itself from the next start"
    }
    Remove-Item -LiteralPath $logon -Force
}

if (-not (Test-Path -LiteralPath (Join-Path $files 'setup.ps1'))) {
    Send-Line "screen: remoteman's host wasn't given, so the guest has no screen"
    return
}

# remoteman's host is installed again whenever the host brings different files.
$wanted = (Get-ChildItem -LiteralPath $files -Recurse -File | Sort-Object FullName |
    ForEach-Object { (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash }) -join ''
$installedFile = Join-Path $screen 'installed.txt'
$installed = if (Test-Path -LiteralPath $installedFile) { (Get-Content -LiteralPath $installedFile -Raw).Trim() } else { '' }
if (-not (Test-Path -LiteralPath $hostExe) -or $installed -ne $wanted) {
    Send-Line 'screen: installing remoteman host'
    Stop-Service -Name $service -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force -Path $folder | Out-Null
    Copy-Item -Path (Join-Path $files '*') -Destination $folder -Recurse -Force
    $setupScript = Join-Path $folder 'setup.ps1'
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $setupScript install *> $null
    $code = $LASTEXITCODE
    if ($code -ne 0) {
        Send-Line "screen: remoteman's setup failed, with $code"
        $log = Join-Path $folder 'setup.log'
        if (Test-Path -LiteralPath $log) {
            foreach ($line in (Get-Content -LiteralPath $log -Tail 8)) { Send-Line "screen: setup: $line" }
        }
        return
    }
    Set-Content -LiteralPath $installedFile -Value $wanted -Encoding ascii
    Send-Line 'screen: remoteman host installed'
}

# The host is told whom to trust, and which encoder to use, before it starts: it reads its trust
# list once, when it starts. The installer gives it no --pair, so nobody else is let in, and the
# command line here drops --ask, which would wait on a question nobody sees.
Stop-Service -Name $service -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $identity | Out-Null
Copy-Item -LiteralPath (Join-Path $screen 'trusted') -Destination (Join-Path $identity 'trusted') -Force
$encoder = (Get-Content -LiteralPath (Join-Path $screen 'encoder.txt') -Raw).Trim()
$command = "`"$hostExe`" service --state `"$state`" --log `"$state\host.log`" --encoder $encoder"
Set-ItemProperty -LiteralPath "HKLM:\SYSTEM\CurrentControlSet\Services\$service" -Name ImagePath -Value $command
Start-Service -Name $service

# The host's fingerprint, which the viewer checks it against. The host makes its identity when it
# first starts.
$cert = Join-Path $identity 'cert.der'
$waited = 0
while (-not (Test-Path -LiteralPath $cert) -and $waited -lt 30) {
    Start-Sleep -Seconds 1
    $waited += 1
}
if (Test-Path -LiteralPath $cert) {
    $fingerprint = (Get-FileHash -LiteralPath $cert -Algorithm SHA256).Hash.ToLowerInvariant()
    Send-Line "screen: fingerprint $fingerprint"
} else {
    Send-Line "screen: remoteman's host made no identity in $identity"
}
