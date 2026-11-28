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
$readMilliseconds = 10000

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
while ($true) {
    $client = $listener.AcceptTcpClient()
    try {
        if ($client.Client.RemoteEndPoint.Address.ToString() -ne $HostAddress) { continue }
        $stream = $client.GetStream()
        $stream.ReadTimeout = $readMilliseconds
        $reader = [System.IO.StreamReader]::new($stream)
        $writer = [System.IO.StreamWriter]::new($stream)
        $parts = $reader.ReadLine().Split(' ', 2)
        if ($parts[0] -eq $token) {
            Name-Display
            $script = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($parts[1]))
            try { $out = Invoke-Expression $script 2>&1 | Out-String -Width 220 } catch { $out = "ERROR: $_" }
            $writer.Write($out)
            $writer.Flush()
        }
    } catch {
    } finally {
        $client.Close()
    }
}
