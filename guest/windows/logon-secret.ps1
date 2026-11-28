# Signing in by itself with no password in the registry, for every Windows guest. Each host puts
# this before a script it runs as SYSTEM at every start: the Linux host before guest-rights.ps1, the
# Windows host before screen.ps1. One copy, so both do the same.

# Gives Windows the password it signs in with by itself as the LSA secret DefaultPassword, which only
# SYSTEM can read, as Sysinternals' Autologon does. Winlogon takes it from there when its registry
# key has no DefaultPassword value. Says whether Windows took it.
function Save-LogonSecret([string]$Secret) {
    if (-not ('Guesthouse.LogonSecret' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
namespace Guesthouse {
public static class LogonSecret {
[StructLayout(LayoutKind.Sequential)]
public struct UnicodeText { public ushort Length; public ushort MaximumLength; public IntPtr Buffer; }
[StructLayout(LayoutKind.Sequential)]
public struct ObjectAttributes { public int Length; public IntPtr RootDirectory; public IntPtr ObjectName; public uint Attributes; public IntPtr SecurityDescriptor; public IntPtr SecurityQualityOfService; }
[DllImport("advapi32.dll")] static extern uint LsaOpenPolicy(IntPtr system, ref ObjectAttributes attributes, uint access, out IntPtr policy);
[DllImport("advapi32.dll")] static extern uint LsaStorePrivateData(IntPtr policy, ref UnicodeText key, ref UnicodeText data);
[DllImport("advapi32.dll")] static extern uint LsaClose(IntPtr policy);
const uint CreateSecret = 0x20;
static UnicodeText Text(string value) {
    UnicodeText text = new UnicodeText();
    text.Buffer = Marshal.StringToHGlobalUni(value);
    text.Length = (ushort)(value.Length * 2);
    text.MaximumLength = (ushort)(value.Length * 2 + 2);
    return text;
}
// 0 once Windows has it, or the status it said.
public static uint Store(string name, string value) {
    ObjectAttributes attributes = new ObjectAttributes();
    attributes.Length = Marshal.SizeOf(typeof(ObjectAttributes));
    IntPtr policy;
    uint status = LsaOpenPolicy(IntPtr.Zero, ref attributes, CreateSecret, out policy);
    if (status != 0) return status;
    UnicodeText key = Text(name);
    UnicodeText data = Text(value);
    try { return LsaStorePrivateData(policy, ref key, ref data); }
    finally {
        Marshal.FreeHGlobal(key.Buffer);
        Marshal.ZeroFreeGlobalAllocUnicode(data.Buffer);
        LsaClose(policy);
    }
}
}
}
'@
    }
    [Guesthouse.LogonSecret]::Store('DefaultPassword', $Secret) -eq 0
}

# A DefaultPassword value in Winlogon's key, $Key, which every account may read, as Windows' own
# answer-file sign-in or an earlier Guesthouse left it. It moves to the LSA secret and goes from the
# registry whatever Windows says. Says 'moved', 'refused' or 'none'.
function Move-LeftPassword([string]$Key) {
    $found = Get-ItemProperty -LiteralPath $Key -Name DefaultPassword -ErrorAction SilentlyContinue
    if ($null -eq $found -or $null -eq $found.DefaultPassword) { return 'none' }
    $kept = Save-LogonSecret ([string]$found.DefaultPassword)
    Remove-ItemProperty -LiteralPath $Key -Name DefaultPassword
    if ($kept) { 'moved' } else { 'refused' }
}
