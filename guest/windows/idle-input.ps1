# Run once by idle.ps1 in the user's session, as that user, and only there can Windows say when this
# session last had a key or the mouse. It writes the seconds since then to the file it is given, and
# leaves nothing else. It writes to a name of its own first, so the file that idle.ps1 waits for is
# whole when it appears.
param([string]$Out)
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class GpupvInput {
    [StructLayout(LayoutKind.Sequential)]
    struct Last { public uint Size; public uint Time; }
    [DllImport("user32.dll")]
    static extern bool GetLastInputInfo(ref Last info);
    [DllImport("kernel32.dll")]
    static extern uint GetTickCount();
    // Both are 32-bit tick counts that wrap, so the difference is taken in 32 bits too.
    public static long Milliseconds() {
        Last info = new Last();
        info.Size = (uint)Marshal.SizeOf(typeof(Last));
        if (!GetLastInputInfo(ref info)) return -1;
        return unchecked(GetTickCount() - info.Time);
    }
}
'@
$milliseconds = [GpupvInput]::Milliseconds()
if ($milliseconds -ge 0) {
    $partial = "$Out.part"
    Set-Content -LiteralPath $partial -Value ([math]::Floor($milliseconds / 1000)) -Encoding Ascii
    Move-Item -LiteralPath $partial -Destination $Out -Force
}
