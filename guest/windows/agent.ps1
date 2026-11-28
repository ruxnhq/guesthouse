# Runs PowerShell sent by the host, which always comes through QEMU's user network, so the gateway of
# that network is the only address it talks to. Each request has to start with this machine's token.
# A guest runs it with what is set here. A test runs it on its own loopback, with a token file and a
# display key of its own.
param(
    [string]$TokenFile = 'C:\gpupv\token.txt',
    [string]$HostAddress = '10.0.2.2',
    [string]$ListenAddress = '0.0.0.0',
    [int]$Port = 8765,
    [string]$VideoKey = 'HKLM:\SYSTEM\CurrentControlSet\Control\Video'
)
# How long a connection has to send its whole first line. Connections wait for theirs side by side, so
# one that is slow to send, or sends nothing, holds up no other.
$lineMilliseconds = 10000
# How many connections wait for their line at once. One more takes the place of the one that has
# waited longest.
$mostWaiting = 16

# Vulkan won't use a display unless it looks like a remote session's, and it goes by the name. The
# key can show up after setup or on a later boot, so this looks every time. Windows reads it at boot.
function Name-Display {
    Get-ChildItem $VideoKey -ea 0 | ForEach-Object { Join-Path $_.PSPath '0000' } |
        Where-Object { (Get-ItemProperty $_ -ea 0).DriverDesc -eq 'Microsoft Basic Display Adapter' } |
        ForEach-Object { Set-ItemProperty $_ DriverDesc 'Microsoft Basic Display Adapter (RDPUDD Chained DD)' }
}
Name-Display
$token = (Get-Content -LiteralPath $TokenFile -Raw).Trim()
$listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Parse($ListenAddress), $Port)
$listener.Start()

# Runs the request on a connection whose line has come, and answers it.
function Serve-Line($Waiting) {
    try {
        $line = $Waiting.Line.Result
        if ($null -eq $line) { return }
        $parts = $line.Split(' ', 2)
        if ($parts[0] -eq $token) {
            Name-Display
            $script = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($parts[1]))
            try { $out = Invoke-Expression $script 2>&1 | Out-String -Width 220 } catch { $out = "ERROR: $_" }
            $writer = [System.IO.StreamWriter]::new($Waiting.Client.GetStream())
            $writer.Write($out)
            $writer.Flush()
        }
    } catch {
    } finally {
        $Waiting.Client.Close()
    }
}

$waiting = [System.Collections.ArrayList]::new()
while ($true) {
    $busy = $false
    while ($listener.Pending()) {
        $busy = $true
        $client = $listener.AcceptTcpClient()
        if ($client.Client.RemoteEndPoint.Address.ToString() -ne $HostAddress) { $client.Close(); continue }
        if ($waiting.Count -ge $mostWaiting) {
            $waiting[0].Client.Close()
            $waiting.RemoveAt(0)
        }
        $reader = [System.IO.StreamReader]::new($client.GetStream())
        $by = [DateTime]::UtcNow.AddMilliseconds($lineMilliseconds)
        [void]$waiting.Add([pscustomobject]@{ Client = $client; Line = $reader.ReadLineAsync(); By = $by })
    }
    foreach ($one in @($waiting)) {
        if ($one.Line.IsCompleted) {
            $busy = $true
            $waiting.Remove($one)
            Serve-Line $one
        } elseif ([DateTime]::UtcNow -gt $one.By) {
            $waiting.Remove($one)
            $one.Client.Close()
        }
    }
    if (-not $busy) { Start-Sleep -Milliseconds 10 }
}
