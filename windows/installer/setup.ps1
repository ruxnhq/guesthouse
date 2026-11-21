# What Guesthouse's installer does once its files are in place, and what its uninstaller undoes.
# Run by the installer (crates/installer-windows), elevated, hidden; never by hand except to repair:
#   powershell -ExecutionPolicy Bypass -File setup.ps1 install
#   powershell -ExecutionPolicy Bypass -File setup.ps1 uninstall [-DeleteMachines]
# install:   checks for Windows 11, and for the Virtual Machine Platform feature, which it turns on
#            if it is off (never Hyper-V). Then it registers the service "Guesthouse" (automatic,
#            LocalSystem, restarted if it ends, and given long enough at shutdown to ask the guests
#            to shut down) and starts it.
# uninstall: stops the service, which asks the guests to shut down, and deletes it. Virtual Machine
#            Platform is left on, as other programs use it too. The machines in
#            %ProgramData%\Guesthouse are kept unless -DeleteMachines says to delete them.
# The installer copies the files first (and takes them away after); this runs from the folder they
# are in, as the service's program is one of them.
# Exit code 0, or 3010 when it worked but Windows has to restart before a machine can run (the
# feature was just turned on), or 1 with the reason as the last line of setup.log.
param([Parameter(Mandatory)][ValidateSet('install', 'uninstall')][string]$Verb, [switch]$DeleteMachines)
$ErrorActionPreference = 'Stop'
$app = $PSScriptRoot
$exe = Join-Path $app 'guesthouse-service.exe'
$log = Join-Path $app 'setup.log'
$name = 'Guesthouse'
$data = Join-Path $env:ProgramData 'Guesthouse'
# Windows 11's first build.
$firstWindows11 = 22000
# The service asks each guest to shut down and gives it about a minute (GRACE in
# crates/service-windows/src/windows/mod.rs); Windows is told to wait a little longer than that
# when the PC shuts down.
$stopWaitSeconds = 90
$preshutdownMs = 90000

function Say([string]$line) { Add-Content -Path $log -Value "$(Get-Date -Format s) $line" }

# ChangeServiceConfig2 for the preshutdown timeout, which neither New-Service nor sc.exe sets.
$source = @'
using System; using System.Runtime.InteropServices;
public class GhService {
    const int SC_MANAGER_CONNECT = 0x1;
    const int SERVICE_CHANGE_CONFIG = 0x2;
    const int SERVICE_CONFIG_PRESHUTDOWN_INFO = 7;
    [DllImport("advapi32.dll", SetLastError=true, CharSet=CharSet.Unicode)] static extern IntPtr OpenSCManagerW(string machine, string db, int access);
    [DllImport("advapi32.dll", SetLastError=true, CharSet=CharSet.Unicode)] static extern IntPtr OpenServiceW(IntPtr scm, string name, int access);
    [DllImport("advapi32.dll", SetLastError=true)] static extern bool ChangeServiceConfig2W(IntPtr service, int level, ref uint info);
    [DllImport("advapi32.dll")] static extern bool CloseServiceHandle(IntPtr handle);
    public static string SetPreshutdown(string name, uint milliseconds) {
        IntPtr scm = OpenSCManagerW(null, null, SC_MANAGER_CONNECT);
        if (scm == IntPtr.Zero) return "OpenSCManager failed " + Marshal.GetLastWin32Error();
        IntPtr service = OpenServiceW(scm, name, SERVICE_CHANGE_CONFIG);
        if (service == IntPtr.Zero) { int e = Marshal.GetLastWin32Error(); CloseServiceHandle(scm); return "OpenService failed " + e; }
        bool ok = ChangeServiceConfig2W(service, SERVICE_CONFIG_PRESHUTDOWN_INFO, ref milliseconds);
        int error = Marshal.GetLastWin32Error();
        CloseServiceHandle(service); CloseServiceHandle(scm);
        return ok ? "ok" : "failed " + error;
    }
}
'@

# Stops the service and waits for it: a guest that is shutting down must not be cut off.
function Stop-Ours {
    $service = Get-Service -Name $name -ErrorAction SilentlyContinue
    if (-not $service -or $service.Status -eq 'Stopped') { return }
    Say "stopping the service (it asks each machine to shut down first, up to $stopWaitSeconds s)"
    Stop-Service -Name $name -NoWait -ErrorAction SilentlyContinue
    try { $service.WaitForStatus('Stopped', [TimeSpan]::FromSeconds($stopWaitSeconds)) }
    catch { throw "the service didn't stop within $stopWaitSeconds s: a machine may still be shutting down. Shut the machines down, or wait a little, and try again" }
    Say "the service stopped"
}

function Install {
    Say "install from $app"

    # Windows 11 (the machines and their graphics card need what it has).
    $build = [int](Get-CimInstance Win32_OperatingSystem).BuildNumber
    if ($build -lt $firstWindows11) { throw "Guesthouse needs Windows 11 (this is build $build; Windows 11 starts at $firstWindows11)" }
    Say "Windows build $build"

    # Virtual Machine Platform is what the machines run on. Only this feature: never Hyper-V.
    $restart = $false
    $feature = Get-WindowsOptionalFeature -Online -FeatureName VirtualMachinePlatform -ErrorAction Stop
    Say "Virtual Machine Platform: $($feature.State)"
    if ($feature.State -eq 'EnablePending') {
    $restart = $true
    } elseif ($feature.State -ne 'Enabled') {
    Say "turning on Virtual Machine Platform"
    $result = Enable-WindowsOptionalFeature -Online -FeatureName VirtualMachinePlatform -NoRestart -All
    # Turned on from off, it isn't usable until Windows restarts, whatever RestartNeeded says.
    $restart = $true
    Say "Virtual Machine Platform turned on (restart needed: $($result.RestartNeeded))"
    }

    # An earlier install's service has to be stopped before its settings change.
    Stop-Ours

    if (-not (Test-Path $exe)) { throw "$exe is missing" }
    $binary = "`"$exe`" service"
    $description = 'Guesthouse: keeps your virtual machines and runs them with your graphics card, for the Guesthouse app.'
    $existing = Get-Service -Name $name -ErrorAction SilentlyContinue
    if ($existing) {
    # Installed over an earlier install: the command line straight into the service's key
    # (sc.exe can't be handed one with quotes in it from here).
    Set-ItemProperty "HKLM:\SYSTEM\CurrentControlSet\Services\$name" -Name ImagePath -Value $binary -Type ExpandString
    Set-Service -Name $name -StartupType Automatic -DisplayName $name -Description $description
    } else {
    New-Service -Name $name -BinaryPathName $binary -DisplayName $name -StartupType Automatic -Description $description | Out-Null
    }
    # Started again if it ever ends: after 5 s, twice, then after a minute.
    $out = & sc.exe failure $name reset= 86400 actions= restart/5000/restart/5000/restart/60000
    if ($LASTEXITCODE -ne 0) { Say "the service's restart rule wasn't set: $out" }
    Add-Type $source
    $pre = [GhService]::SetPreshutdown($name, $preshutdownMs)
    Say "shutdown wait of $($preshutdownMs / 1000) s: $pre"
    if ($pre -ne 'ok') { throw "the service's shutdown wait wasn't set: $pre" }

    Start-Service -Name $name
    # A service that starts and ends at once is no help; its own log says why.
    Start-Sleep -Seconds 3
    $status = (Get-Service -Name $name).Status
    if ($status -ne 'Running') {
    Get-Content (Join-Path $data 'service.log') -Tail 10 -ErrorAction SilentlyContinue | ForEach-Object { Say "service.log: $_" }
    throw "the service started but is $status now (service.log in $data may say why)"
    }
    Say "service registered and running"

    if ($restart) {
    Say "Windows has to restart before a machine can run"
    exit 3010
    }
}

function Uninstall([bool]$DeleteData) {
    Say "uninstall"
    Stop-Ours
    if (Get-Service -Name $name -ErrorAction SilentlyContinue) {
    $out = & sc.exe delete $name
    Say "service removed: $out"
    }
    # The service's program, if one is left from this folder (one started at a prompt, say), so
    # that the folder can go.
    Get-Process -Name guesthouse-service -ErrorAction SilentlyContinue | Where-Object Path -eq $exe |
    ForEach-Object { Stop-Process -Id $_.Id -Force }
    if ($DeleteData) {
    # Only the service's own folder, and not through a link to somewhere else.
    $folder = Get-Item -LiteralPath $data -Force -ErrorAction SilentlyContinue
    if ($folder -and -not $folder.Attributes.HasFlag([IO.FileAttributes]::ReparsePoint)) {
        Remove-Item -LiteralPath $data -Recurse -Force
        Say "the machines in $data deleted"
    } elseif ($folder) {
        throw "$data is a link, not the service's own folder, so it wasn't deleted"
    } else {
        Say "no machines folder at $data"
    }
    } else {
    Say "the machines in $data are kept"
    }
    Say "uninstalled (Virtual Machine Platform is left on)"
}

try {
    if ($Verb -eq 'install') { Install } else { Uninstall $DeleteMachines }
    exit 0
} catch {
    Say "FAILED: $($_.Exception.Message)"
    exit 1
}
