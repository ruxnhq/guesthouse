# Sent by the host through the agent to read how busy this Windows is, with $helper, the file
# idle-input.ps1 in Base64, put first. It only reads. The one thing it does is a task in the user's
# session that lives for a moment and is removed: only a program there can see that session's input.
# Everything it says is in lines of our own, as Windows' words change with its language, and the host
# takes the layout and no word of Windows' own:
#   sessions ok|failed, then s|ID|STATE|STATION for each session, by Windows' numbers and station names
#   power ok|failed, then p|LINE for each line of powercfg's text, each cut short
#   input SECONDS|nobody|failed
# The agent runs this in the scope of its own loop, so no name here may be one of the agent's.
if (-not ('GpupvSessions' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class GpupvSessions {
    [StructLayout(LayoutKind.Sequential)]
    struct Info { public int Id; public IntPtr Station; public int State; }
    [DllImport("wtsapi32.dll", SetLastError = true)]
    static extern bool WTSEnumerateSessions(IntPtr server, int reserved, int version, out IntPtr info, out int count);
    [DllImport("wtsapi32.dll")]
    static extern void WTSFreeMemory(IntPtr memory);
    public static string List() {
        IntPtr info;
        int count;
        if (!WTSEnumerateSessions(IntPtr.Zero, 0, 1, out info, out count)) return null;
        StringBuilder lines = new StringBuilder();
        int size = Marshal.SizeOf(typeof(Info));
        for (int i = 0; i < count; i++) {
            Info one = (Info)Marshal.PtrToStructure(IntPtr.Add(info, i * size), typeof(Info));
            lines.AppendLine("s|" + one.Id + "|" + one.State + "|" + Marshal.PtrToStringUni(one.Station));
        }
        WTSFreeMemory(info);
        return lines.ToString();
    }
}
'@
}

$idleCharsKept = 150
$idleWaitSteps = 40
$idleTask = 'gpupv-idle-input'
$idleHelper = 'C:\gpupv\idle-input.ps1'
$idleResult = Join-Path ([Environment]::GetFolderPath('CommonDocuments')) 'gpupv-idle-input.txt'

try {
    $idleSessions = [GpupvSessions]::List()
    if ($idleSessions) { 'sessions ok'; $idleSessions.TrimEnd() } else { 'sessions failed' }
} catch { 'sessions failed' }

try {
    $idlePower = @(powercfg /requests 2>&1)
    if ($LASTEXITCODE -ne 0 -or $idlePower.Count -eq 0) { 'power failed' } else {
        'power ok'
        foreach ($idleLine in $idlePower) {
            $idleText = "$idleLine"
            'p|' + $idleText.Substring(0, [Math]::Min($idleCharsKept, $idleText.Length))
        }
    }
} catch { 'power failed' }

try {
    $idleUser = (Get-Process explorer -IncludeUserName -ea 0 | Sort-Object SessionId | Select-Object -First 1).UserName
    if (-not $idleUser) { 'input nobody' } else {
        Remove-Item -LiteralPath $idleResult -ea 0
        [IO.File]::WriteAllBytes($idleHelper, [Convert]::FromBase64String($helper))
        $idleAction = New-ScheduledTaskAction -Execute conhost.exe -Argument "--headless powershell.exe -NoProfile -ExecutionPolicy Bypass -File $idleHelper -Out $idleResult"
        $idlePrincipal = New-ScheduledTaskPrincipal -UserId $idleUser -LogonType Interactive
        $idleSettings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Minutes 1)
        Register-ScheduledTask -TaskName $idleTask -Action $idleAction -Principal $idlePrincipal -Settings $idleSettings -Force | Out-Null
        Start-ScheduledTask -TaskName $idleTask
        $idleSeconds = $null
        for ($idleStep = 0; $idleStep -lt $idleWaitSteps -and -not $idleSeconds; $idleStep++) {
            Start-Sleep -Milliseconds 250
            if (Test-Path -LiteralPath $idleResult) { $idleSeconds = (Get-Content -LiteralPath $idleResult -Raw -ea 0).Trim() }
        }
        if ($idleSeconds -match '^[0-9]+$') { "input $idleSeconds" } else { 'input failed' }
    }
} catch { 'input failed' } finally {
    Unregister-ScheduledTask -TaskName $idleTask -Confirm:$false -ea 0
    Remove-Item -LiteralPath $idleResult, $idleHelper -ea 0
}
