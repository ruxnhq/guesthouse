# Sent by the host through the agent at every start, and when the setting changes while Windows runs,
# with $on, $package and $keys set above it. Setup runs it too, from the install disc, with the
# machine's choice there. OpenSSH comes from Microsoft's installer, which the host brings: Windows' own
# way of adding it downloads it, and the guest may have no internet. With no installer given, a guest
# without OpenSSH says so, and the host sends the installer and asks again.
# sshd listens on port 22, and Windows' firewall lets in only what comes from 10.0.2.2, the host's end
# of QEMU's user network, where every port forward comes from. The installer adds a rule of its own
# for port 22 from anywhere on a private network, which is turned off here. $keys is the host user's
# public keys in Base64, which go where sshd looks for an administrator's keys, as the account is one.
# Nothing here goes by a name that changes with Windows' language. The agent runs this in its own
# loop's scope, so no name here may be one of the agent's.
$rule = 'gpupv-ssh'
$installersRule = 'OpenSSH SSH Server Preview (sshd)'
$sshd = Join-Path $env:ProgramFiles 'OpenSSH\sshd.exe'
$keyFile = Join-Path $env:ProgramData 'ssh\administrators_authorized_keys'
$installLog = 'C:\gpupv\openssh-install.log'
# sshd refuses the key file unless only these two can change it. By SID, as the names are translated.
$administrators = '*S-1-5-32-544'
$system = '*S-1-5-18'

function Install-OpenSSH {
    $arguments = "/i `"$package`" /qn /norestart ADDLOCAL=Server /l*v `"$installLog`""
    $run = Start-Process msiexec.exe -ArgumentList $arguments -Wait -PassThru
    # 3010 is installed, with a restart asked for that sshd doesn't need.
    if ($run.ExitCode -notin 0, 3010) { throw "the installer stopped with $($run.ExitCode). $installLog says why." }
}

# The installer's rule names sshd.exe, under a name of its own that isn't translated. Either finds it.
# Says whether it turned anything off.
function Close-InstallersRule {
    $filters = @(Get-NetFirewallApplicationFilter -Program $sshd -ea 0)
    $found = @(Get-NetFirewallRule -DisplayName $installersRule -ea 0)
    if ($filters.Count -gt 0) { $found += @($filters | Get-NetFirewallRule) }
    $wide = @($found | Where-Object { $_.DisplayName -ne $rule -and $_.Enabled -eq 'True' })
    if ($wide.Count -eq 0) { return $false }
    $wide | Disable-NetFirewallRule -ErrorAction Stop
    $true
}

# Adds each key the file doesn't have yet. Says whether it added any.
function Add-Keys {
    if (-not $keys) { return $false }
    $text = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($keys))
    $wanted = @($text -split "`n" | ForEach-Object { $_.Trim() } | Where-Object { $_ })
    New-Item -ItemType Directory -Force (Split-Path $keyFile) | Out-Null
    $have = @(if (Test-Path -LiteralPath $keyFile) { Get-Content -LiteralPath $keyFile | ForEach-Object { $_.Trim() } })
    $new = @($wanted | Where-Object { $have -notcontains $_ })
    if ($new.Count -gt 0) {
        [IO.File]::WriteAllLines($keyFile, [string[]]($have + $new), [Text.UTF8Encoding]::new($false))
    }
    icacls $keyFile /inheritance:r /grant:r "${administrators}:F" "${system}:F" | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "icacls could not limit who may change $keyFile." }
    $new.Count -gt 0
}

try {
    $service = Get-Service sshd -ea 0
    $ours = @(Get-NetFirewallRule -DisplayName $rule -ea 0)
    $changed = $false
    if (-not $on) {
        if ($service -and $service.Status -ne 'Stopped') { Stop-Service sshd -Force -ErrorAction Stop; $changed = $true }
        if ($service -and $service.StartType -ne 'Disabled') { Set-Service sshd -StartupType Disabled -ErrorAction Stop; $changed = $true }
        if ($ours.Count -gt 0) { $ours | Remove-NetFirewallRule -ErrorAction Stop; $changed = $true }
        if ($changed) { 'off' } else { 'kept off' }
        return
    }
    if (-not $service) {
        if (-not $package) { 'needs the installer'; return }
        Install-OpenSSH
        $service = Get-Service sshd -ErrorAction Stop
        $changed = $true
    }
    if ($service.StartType -ne 'Automatic') { Set-Service sshd -StartupType Automatic -ErrorAction Stop; $changed = $true }
    # Its first start makes the host keys and the folder the key file goes in.
    if ($service.Status -ne 'Running') { Start-Service sshd -ErrorAction Stop; $changed = $true }
    if (Close-InstallersRule) { $changed = $true }
    if ($ours.Count -eq 0) {
        New-NetFirewallRule -DisplayName $rule -Direction Inbound -Protocol TCP -LocalPort 22 -RemoteAddress 10.0.2.2 -Action Allow -ErrorAction Stop | Out-Null
        $changed = $true
    } elseif ($ours | Where-Object { $_.Enabled -ne 'True' }) {
        $ours | Enable-NetFirewallRule -ErrorAction Stop
        $changed = $true
    }
    if (Add-Keys) { $changed = $true }
    if ($changed) { 'on' } else { 'kept on' }
} catch {
    "FAILED ssh: $($_.Exception.Message)"
}
