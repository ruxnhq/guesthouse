# Sent by the host through the agent once the desktop is up, on NVIDIA machines, with $name set above
# it. NVIDIA's OpenGL and Vulkan driver keeps a display named "RDPUDD Chained DD" as its own, which our
# display driver's INF names it so that the driver finds its GPU while Windows sees Microsoft's vendor
# number on the render device. Once Windows sees NVIDIA's number, the driver finds the GPU by itself,
# and the kept display only folds the GPU into the display's adapter: OpenGL is then composed through
# a copy of its window, and no Vulkan program can import memory. So the host names it by what Windows
# sees. Windows reads the name at boot, so a change counts from the next start.
$hardwareId = 'Root\GuesthouseDisplay'
try {
    # A guest without our display driver has no such key, and nothing to name.
    $device = Get-ChildItem 'HKLM:\SYSTEM\CurrentControlSet\Enum\ROOT\DISPLAY' -ea 0 |
        Where-Object { (Get-ItemProperty $_.PSPath -ea 0).HardwareID -contains $hardwareId } | Select-Object -First 1
    if (-not $device) { "kept: no display of ours"; return }
    $driver = (Get-ItemProperty $device.PSPath -ErrorAction Stop).Driver
    $key = Join-Path 'HKLM:\SYSTEM\CurrentControlSet\Control\Class' $driver
    $now = (Get-ItemProperty -LiteralPath $key -ErrorAction Stop).DriverDesc
    if ($now -eq $name) { "kept $name"; return }
    Set-ItemProperty -LiteralPath $key -Name DriverDesc -Value $name -ErrorAction Stop
    "named $name from the next start (was $now)"
} catch {
    "FAILED display name: $($_.Exception.Message)"
}
