# Says what the guest sees of its GPU and network, so the host can read it with no screen. Each line
# goes to a file on the guest's own disk, which the host reads once the machine has stopped, and to
# the first serial port too if there is one, which the host prints while the guest runs. It runs as
# SYSTEM at every start, from the task SetupComplete.cmd makes.
$ErrorActionPreference = 'Continue'
$report = 'C:\Guesthouse\report.txt'
Set-Content -LiteralPath $report -Value "started $(Get-Date -Format o)" -Encoding utf8

$port = $null
try {
    $port = New-Object -TypeName System.IO.Ports.SerialPort -ArgumentList 'COM1', 115200
    $port.Open()
} catch {
    Add-Content -LiteralPath $report -Value "no serial port: $($_.Exception.Message)" -Encoding utf8
    $port = $null
}

function Send-Line([string]$Text) {
    Add-Content -LiteralPath $report -Value $Text -Encoding utf8
    if ($port) {
        try { $port.WriteLine("guesthouse: $Text") } catch { $port = $null }
    }
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

# The guest's screen, when the host gave it one, before the last line, as the host takes that line
# to mean the guest is set up.
$screenScript = 'C:\Guesthouse\screen\screen.ps1'
if (Test-Path -LiteralPath $screenScript) {
    try { . $screenScript } catch { Send-Line "screen: failed: $($_.Exception.Message)" }
}

Send-Line 'report done'
if ($port) { $port.Close() }
