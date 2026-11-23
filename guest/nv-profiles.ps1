# Sent by the host through the agent once the desktop is up. Makes the folder NVIDIA's settings store
# lives in, with a copy of the driver's own profile database in it. The driver's installer would do
# this, and a guest never runs it. A program that reads its profile through NVAPI stops at start
# without the folder, as NVAPI can't open the store. The files stay once they exist, as NVAPI writes
# them. The driver's database is read-only where it's copied from and a copy keeps that, so it's
# cleared here at every start: with it, nothing can save a profile.
#
# With $on set above it (the machine's "Faster programs in a window"), it also sets the driver's
# Vulkan and OpenGL present method to its DXGI swap chain in the global profile. Under the default the
# driver presents those through a copy of every frame into guest memory when vsync is off; through
# DXGI the frame stays on the card. Only a value written here is ever taken back, and a value the
# user set stays.
$store = Join-Path $env:ProgramData 'NVIDIA Corporation\Drs'
$usersSid = 'S-1-5-32-545'
$names = 'nvdrsdb0.bin', 'nvdrsdb1.bin'
$driverFolders = Join-Path $env:windir 'System32\HostDriverStore\FileRepository\nv_dispi.inf_amd64_*'

function Clear-ReadOnly {
    foreach ($name in $names) {
        $file = Get-Item -LiteralPath (Join-Path $store $name) -Force -ErrorAction Stop
        if ($file.IsReadOnly) { $file.IsReadOnly = $false }
    }
}

function Place-Store {
$missing = $names | Where-Object { -not (Test-Path -LiteralPath (Join-Path $store $_)) }
if (-not $missing) {
    try { Clear-ReadOnly; "kept $store" } catch { "FAILED $store`: $($_.Exception.Message)" }
    return
}
$database = Get-ChildItem -Path (Join-Path $driverFolders 'nvdrsdb.bin') -ea 0 |
    Sort-Object LastWriteTimeUtc -Descending | Select-Object -First 1
if (-not $database) { "no nvdrsdb.bin under $driverFolders"; return }
try {
    New-Item -ItemType Directory -Path $store -Force -ErrorAction Stop | Out-Null
    foreach ($name in $missing) {
        Copy-Item -LiteralPath $database.FullName -Destination (Join-Path $store $name) -Force -ErrorAction Stop
    }
    $users = (New-Object System.Security.Principal.SecurityIdentifier $usersSid).Translate([System.Security.Principal.NTAccount]).Value
    $said = & icacls $store /grant "${users}:(OI)(CI)M" /T 2>&1
    if ($LASTEXITCODE -ne 0) { throw "icacls: $said" }
    Clear-ReadOnly
    "made $store from $($database.FullName)"
} catch {
    "FAILED $store`: $($_.Exception.Message)"
}
}

# NVIDIA's "Vulkan/OpenGL present method" setting, and 1: prefer the layered DXGI swap chain.
$presentSetting = 0x20D690F8
$presentValue = 1
# Where it's noted that the value in the global profile is ours.
$markKey = 'HKLM:\SOFTWARE\Guesthouse'
$markName = 'NvidiaPresentMethod'

$drs = @'
using System;
using System.Runtime.InteropServices;
public static class GuesthouseDrs {
    [DllImport("nvapi64.dll", EntryPoint = "nvapi_QueryInterface")] static extern IntPtr Query(uint id);
    delegate int NoArgs();
    delegate int Session(IntPtr session);
    delegate int NewSession(out IntPtr session);
    delegate int BaseProfile(IntPtr session, out IntPtr profile);
    delegate int GetSetting(IntPtr session, IntPtr profile, uint id, [In, Out] byte[] setting);
    delegate int SetSetting(IntPtr session, IntPtr profile, [In, Out] byte[] setting);
    delegate int DeleteSetting(IntPtr session, IntPtr profile, uint id);
    // NVDRS_SETTING_V1: version, name (2048 UTF-16), id, type, location, two flags, two 4100-byte values.
    const int SettingSize = 12320, IdAt = 4100, TypeAt = 4104, ValueAt = 8220;
    const int NotFound = -160;
    static T Call<T>(uint id) where T : class {
        IntPtr at = Query(id);
        if (at == IntPtr.Zero) throw new Exception("NVAPI has no function 0x" + id.ToString("x8"));
        return Marshal.GetDelegateForFunctionPointer(at, typeof(T)) as T;
    }
    static void Check(int status, string what) {
        if (status != 0) throw new Exception(what + " said " + status);
    }
    static byte[] Setting() {
        byte[] setting = new byte[SettingSize];
        BitConverter.GetBytes(SettingSize | (1 << 16)).CopyTo(setting, 0);
        return setting;
    }
    // The global profile's value, or -1 when it has none.
    public static long Read(uint id) {
        return Run(s => {
            byte[] setting = Setting();
            int status = Call<GetSetting>(0x73BF8338)(s.Item1, s.Item2, id, setting);
            if (status == NotFound) return -1;
            Check(status, "reading the setting");
            return BitConverter.ToUInt32(setting, ValueAt);
        });
    }
    public static void Write(uint id, uint value) {
        Run(s => {
            byte[] setting = Setting();
            BitConverter.GetBytes(id).CopyTo(setting, IdAt);
            BitConverter.GetBytes(0).CopyTo(setting, TypeAt);
            BitConverter.GetBytes(value).CopyTo(setting, ValueAt);
            Check(Call<SetSetting>(0x577DD202)(s.Item1, s.Item2, setting), "writing the setting");
            Check(Call<Session>(0xFCBC7E14)(s.Item1), "saving");
            return 0;
        });
    }
    public static void Remove(uint id) {
        Run(s => {
            Check(Call<DeleteSetting>(0xE4A26362)(s.Item1, s.Item2, id), "removing the setting");
            Check(Call<Session>(0xFCBC7E14)(s.Item1), "saving");
            return 0;
        });
    }
    static long Run(Func<Tuple<IntPtr, IntPtr>, long> work) {
        Check(Call<NoArgs>(0x0150E828)(), "starting NVAPI");
        IntPtr session, profile;
        Check(Call<NewSession>(0x0694D52E)(out session), "opening the settings");
        try {
            Check(Call<Session>(0x375DBD6B)(session), "loading the settings");
            Check(Call<BaseProfile>(0xDA8466A0)(session, out profile), "finding the global profile");
            return work(Tuple.Create(session, profile));
        } finally {
            Call<Session>(0xDAD9CFF8)(session);
        }
    }
}
'@

function Set-PresentMethod {
    try {
        if (-not ('GuesthouseDrs' -as [type])) { Add-Type -TypeDefinition $drs -ErrorAction Stop }
        $value = [GuesthouseDrs]::Read($presentSetting)
        $ours = (Get-ItemProperty -LiteralPath $markKey -Name $markName -ea 0).$markName -eq 1
        if ($on) {
            if ($value -eq $presentValue) { return "kept the present method" }
            if ($value -ge 0 -and -not $ours) { return "kept the present method the user set ($value)" }
            [GuesthouseDrs]::Write($presentSetting, $presentValue)
            New-Item -Path $markKey -Force -ErrorAction Stop | Out-Null
            New-ItemProperty -LiteralPath $markKey -Name $markName -PropertyType DWord -Value 1 -Force -ErrorAction Stop | Out-Null
            return "present method set to DXGI"
        }
        if (-not $ours) { return "kept the present method" }
        if ($value -eq $presentValue) { [GuesthouseDrs]::Remove($presentSetting) }
        Remove-ItemProperty -LiteralPath $markKey -Name $markName -ErrorAction Stop
        "present method back to the driver's"
    } catch {
        "FAILED present method: $($_.Exception.Message)"
    }
}

$said = @(Place-Store)
if (-not ($said -match '^(FAILED|no nvdrsdb)')) { $said += Set-PresentMethod }
$said -join "`n"
