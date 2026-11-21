# Says on the guest's first serial port what it sees of its GPU and network, so the host can read
# it with no screen. It runs as SYSTEM at every start, from the task SetupComplete.cmd makes.
$ErrorActionPreference = 'Continue'
$port = New-Object -TypeName System.IO.Ports.SerialPort -ArgumentList 'COM1', 115200
$port.Open()

function Send-Line([string]$Text) {
    $port.WriteLine("guesthouse: $Text")
}

Send-Line "started $(Get-Date -Format o)"

# The card's driver can take a while to start after Windows does.
$waitedSeconds = 0
while ($waitedSeconds -lt 120) {
    $cards = @(Get-CimInstance -ClassName Win32_VideoController | Where-Object { $_.Name -notlike 'Microsoft*' })
    if ($cards.Count -gt 0) { break }
    Start-Sleep -Seconds 5
    $waitedSeconds += 5
}

foreach ($card in Get-CimInstance -ClassName Win32_VideoController) {
    Send-Line "card: $($card.Name), driver $($card.DriverVersion), status $($card.Status)"
}

$store = 'C:\Windows\System32\HostDriverStore\FileRepository'
foreach ($folder in Get-ChildItem -LiteralPath $store -Directory -ErrorAction SilentlyContinue) {
    Send-Line "host driver: $($folder.Name)"
}

$smi = Get-ChildItem -LiteralPath $store -Recurse -Filter 'nvidia-smi.exe' -ErrorAction SilentlyContinue |
    Select-Object -First 1
if ($smi) {
    foreach ($line in (& $smi.FullName -L 2>&1)) { Send-Line "nvidia-smi: $line" }
}

foreach ($address in Get-NetIPAddress -AddressFamily IPv4 -ErrorAction SilentlyContinue) {
    if ($address.IPAddress -ne '127.0.0.1') { Send-Line "address: $($address.IPAddress)" }
}

Send-Line 'report done'
$port.Close()
