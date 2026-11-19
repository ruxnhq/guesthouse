# Tells Windows about AMD's display driver package. The guest loads AMD's user-mode driver from the host
# driver store, where no package owns it, and anti-cheat that asks Windows whose a module is blocks it
# there. With AMD's INF and catalog in the driver store, the same files count as AMD's driver.
# Runs as SYSTEM, sent by the host, which puts $inf, $filesFolder and $hostStore in front of this and
# has put the INF and its catalog in $staging. The package is only added to the driver store. Nothing
# is installed on a device, and the adapter keeps Windows' own render driver.
# The last line says 'package registered', or 'not registered: ' and why.
$ErrorActionPreference = 'Continue'
$staging = 'C:\gpupv\amd-package'
$driverStore = 'C:\Windows\System32\DriverStore\FileRepository'
$files = Join-Path $staging $filesFolder

function Test-Registered {
    Test-Path (Join-Path $driverStore "${inf}_amd64_*")
}

$said = ''
try {
    New-Item -ItemType Directory -Force $files | Out-Null
    # The INF names its files in this folder. They're big and already on this disk, so each is a hard
    # link to the host driver store's copy, which costs no space and no time.
    foreach ($file in Get-ChildItem -LiteralPath $hostStore -File) {
        $link = Join-Path $files $file.Name
        if (-not (Test-Path -LiteralPath $link)) {
            New-Item -ItemType HardLink -Path $link -Target $file.FullName -ErrorAction Stop | Out-Null
        }
    }
    # No /install: that would put AMD's kernel driver on any device its INF lists.
    $said = (& pnputil.exe /add-driver (Join-Path $staging $inf) 2>&1 | Out-String)
} catch {
    $said = "$_"
} finally {
    # These are links, so the host driver store keeps its files.
    Remove-Item -LiteralPath $staging -Recurse -Force -ErrorAction SilentlyContinue
}
if (Test-Registered) {
    'package registered'
} else {
    'not registered: ' + ($said -replace '\s+', ' ').Trim()
}
