# Sent by the host through the agent at every start, and when the setting changes while Windows runs,
# with $on set above it. Setup runs it too, from the install disc, with the machine's choice there.
# Remote Desktop listens on port 3389, and Windows' firewall lets in only what comes from 10.0.2.2, the
# host's end of QEMU's user network. Every port forward reaches the guest from that address, whatever
# the machine's network is, so a forward is the way in and a bridged network isn't. Windows' own rules
# for Remote Desktop are left as they are: a fresh Windows has them off, and if someone turned them on
# for their network, that was their choice. Nothing here goes by a name that changes with Windows'
# language. The agent runs this in the scope of its own loop, so no name here may be one of the agent's.
$server = 'HKLM:\SYSTEM\CurrentControlSet\Control\Terminal Server'
$station = Join-Path $server 'WinStations\RDP-Tcp'
$rule = 'gpupv-remote-desktop'

# Says whether it had to change the value.
function Set-Value($key, $name, $value) {
    if ((Get-ItemProperty -LiteralPath $key -ea 0).$name -eq $value) { return $false }
    Set-ItemProperty -LiteralPath $key -Name $name -Value $value -Type DWord -ErrorAction Stop
    $true
}

try {
    $changed = Set-Value $server 'fDenyTSConnections' ([int](-not $on))
    $ours = @(Get-NetFirewallRule -DisplayName $rule -ea 0)
    if ($on) {
        # Windows asks for the account's password before a session starts. That's its default, and
        # the account always has a password.
        if (Set-Value $station 'UserAuthentication' 1) { $changed = $true }
        if ($ours.Count -eq 0) {
            New-NetFirewallRule -DisplayName $rule -Direction Inbound -Protocol TCP -LocalPort 3389 -RemoteAddress 10.0.2.2 -Action Allow -ErrorAction Stop | Out-Null
            $changed = $true
        } elseif ($ours | Where-Object { $_.Enabled -ne 'True' }) {
            $ours | Enable-NetFirewallRule -ErrorAction Stop
            $changed = $true
        }
        if ((Get-Service TermService).Status -ne 'Running') {
            Start-Service TermService -ErrorAction Stop
            $changed = $true
        }
    } elseif ($ours.Count -gt 0) {
        $ours | Remove-NetFirewallRule -ErrorAction Stop
        $changed = $true
    }
    $state = if ($on) { 'on' } else { 'off' }
    if ($changed) { $state } else { "kept $state" }
} catch {
    "FAILED remote desktop: $($_.Exception.Message)"
}
