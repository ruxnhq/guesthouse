# Sent by the host through the agent once the desktop is up, on a machine whose disk is still on the
# emulated SATA controller, just before the host shuts Windows down and starts it again with the disk
# on NVMe. Windows installed on SATA keeps its NVMe driver from loading at boot with a StartOverride
# key, and puts the key back at every boot from a disk that isn't NVMe. Deleting it is enough as long
# as the next boot is on NVMe. The driver itself is set to load at boot already. One that isn't is
# left alone, and the disk stays on SATA.
$ErrorActionPreference = 'Stop'
try {
    $service = 'HKLM:\SYSTEM\CurrentControlSet\Services\stornvme'
    $start = (Get-ItemProperty -LiteralPath $service -Name Start).Start
    if ($start -ne 0) { throw "stornvme starts as $start, not at boot" }
    Remove-Item -LiteralPath "$service\StartOverride" -Recurse -Force -ErrorAction SilentlyContinue
    if (Test-Path -LiteralPath "$service\StartOverride") { throw 'its StartOverride key is still there' }
    'ready'
} catch {
    "FAILED nvme: $($_.Exception.Message)"
}
