# Runs in the user's session, with the user's full rights, while a viewer that sizes the screen to its
# window is open. It only answers: on loopback, and only requests that start with the agent's token.
# A request is a width, a height, a rate and the driver's list of modes. A size the driver's monitor
# doesn't list is added to those, which makes the driver show Windows the monitor again, and then
# Windows is switched to it. Both need this session, and the first needs an administrator.
# While an app has the screen in fullscreen, the screen is left alone: a game whose resolution
# changes under it can close, so the size waits until the app leaves fullscreen.
$source = @'
using System; using System.Runtime.InteropServices; using System.Text;
using Microsoft.Win32.SafeHandles;
public static class DriverControl {
  // CTL_CODE(FILE_DEVICE_UNKNOWN, 0x800, METHOD_BUFFERED, FILE_WRITE_DATA), IOCTL_VDD_COMMAND in the
  // driver's vdd_control_ioctl.h, which also has the interface's GUID.
  const uint Command = 0x22A000;
  const uint ListPresent = 0, GenericReadWrite = 0xC0000000, ShareReadWrite = 3, OpenExisting = 3;
  static Guid Interface = new Guid("DA9F8C2B-7E4F-49A1-9D4E-6F2B0E1A0C4D");
  [DllImport("cfgmgr32.dll", CharSet=CharSet.Unicode)] static extern int CM_Get_Device_Interface_List_SizeW(out uint size, ref Guid g, string id, uint flags);
  [DllImport("cfgmgr32.dll", CharSet=CharSet.Unicode)] static extern int CM_Get_Device_Interface_ListW(ref Guid g, string id, [Out] char[] list, uint size, uint flags);
  [DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)] static extern SafeFileHandle CreateFileW(string path, uint access, uint share, IntPtr security, uint disposition, uint flags, IntPtr template);
  [DllImport("kernel32.dll", SetLastError=true)] static extern bool DeviceIoControl(SafeFileHandle device, uint code, byte[] input, int inputSize, IntPtr output, int outputSize, out int returned, IntPtr overlapped);
  public static string Send(string command) {
    uint size;
    if (CM_Get_Device_Interface_List_SizeW(out size, ref Interface, null, ListPresent) != 0 || size <= 1) return "no control device";
    char[] list = new char[size];
    if (CM_Get_Device_Interface_ListW(ref Interface, null, list, size, ListPresent) != 0) return "no control device";
    string path = new string(list).Split('\0')[0];
    using (SafeFileHandle device = CreateFileW(path, GenericReadWrite, ShareReadWrite, IntPtr.Zero, OpenExisting, 0, IntPtr.Zero)) {
      if (device.IsInvalid) return "not opened, error " + Marshal.GetLastWin32Error();
      byte[] words = Encoding.Unicode.GetBytes(command + "\0");
      int returned;
      if (!DeviceIoControl(device, Command, words, words.Length, IntPtr.Zero, 0, out returned, IntPtr.Zero)) {
        return "refused, error " + Marshal.GetLastWin32Error();
      }
      return "sent";
    }
  }
}
public static class Session {
  // QUNS_BUSY and QUNS_RUNNING_D3D_FULL_SCREEN in shellapi.h: what Windows itself goes by to hold
  // back notifications while an app is fullscreen, borderless or exclusive.
  const int Busy = 2, Direct3DFullscreen = 3;
  [DllImport("shell32.dll")] static extern int SHQueryUserNotificationState(out int state);
  [StructLayout(LayoutKind.Sequential)] struct Rect { public int L, T, R, B; }
  [StructLayout(LayoutKind.Sequential)] struct MonitorInfo { public int Size; public Rect Monitor, Work; public int Flags; }
  [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out Rect r);
  [DllImport("user32.dll")] static extern IntPtr MonitorFromWindow(IntPtr h, uint flags);
  [DllImport("user32.dll")] static extern bool GetMonitorInfo(IntPtr m, ref MonitorInfo info);
  // Busy is also what Windows says while the app windows helper's overlay, a topmost window over the
  // whole screen that never takes the foreground, is up. So busy counts only when the foreground
  // window itself covers its monitor, as a game in borderless fullscreen does.
  public static bool AppFullscreen() {
    int state;
    if (SHQueryUserNotificationState(out state) != 0) return false;
    if (state == Direct3DFullscreen) return true;
    if (state != Busy) return false;
    IntPtr front = GetForegroundWindow();
    Rect r; MonitorInfo info = new MonitorInfo(); info.Size = Marshal.SizeOf(typeof(MonitorInfo));
    if (front == IntPtr.Zero || !GetWindowRect(front, out r) || !GetMonitorInfo(MonitorFromWindow(front, 2), ref info)) return true;
    return r.L <= info.Monitor.L && r.T <= info.Monitor.T && r.R >= info.Monitor.R && r.B >= info.Monitor.B;
  }
}
public static class Screen {
  [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)] public struct Mode {
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst=32)] public string Device; public short Spec, DriverVersion, Size, Extra; public int Fields;
    public int X, Y, Orientation, FixedOutput; public short Color, Duplex, YResolution, TrueType, Collate;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst=32)] public string Form; public short LogPixels;
    public int Bits, Width, Height, Flags, Rate, Icm1, Icm2, Media, Dither, Reserved1, Reserved2, PanWidth, PanHeight; }
  [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)] public struct Display { public int Size;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst=32)] public string Name; [MarshalAs(UnmanagedType.ByValTStr, SizeConst=128)] public string Text; public int Flags;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst=128)] public string Id; [MarshalAs(UnmanagedType.ByValTStr, SizeConst=128)] public string Key; }
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern bool EnumDisplaySettings(string display, int number, ref Mode mode);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern bool EnumDisplayDevices(string display, int number, ref Display found, int flags);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int ChangeDisplaySettingsEx(string display, ref Mode mode, IntPtr window, uint flags, IntPtr parameter);
  const int Current = -1, Attached = 1, WidthHeightRate = 0x80000 | 0x100000 | 0x400000;
  const uint UpdateRegistry = 1;
  // The display driver's monitor, from its EDID. Its adapter's name is no help: remoteman's is called the same.
  const string OurMonitor = @"MONITOR\GHS0001\";
  static Mode Empty() { Mode mode = new Mode(); mode.Size = (short)Marshal.SizeOf(typeof(Mode)); return mode; }
  // The driver's display, if it is the one Windows shows on.
  public static string Find() {
    for (int i = 0; ; i++) {
      Display found = new Display(); found.Size = Marshal.SizeOf(typeof(Display));
      if (!EnumDisplayDevices(null, i, ref found, 0)) return null;
      if ((found.Flags & Attached) != 0 && HasOurMonitor(found.Name)) return found.Name;
    }
  }
  static bool HasOurMonitor(string adapter) {
    for (int i = 0; ; i++) {
      Display monitor = new Display(); monitor.Size = Marshal.SizeOf(typeof(Display));
      if (!EnumDisplayDevices(adapter, i, ref monitor, 0)) return false;
      if (monitor.Id != null && monitor.Id.StartsWith(OurMonitor, StringComparison.OrdinalIgnoreCase)) return true;
    }
  }
  public static bool Lists(string display, int width, int height, int rate) {
    for (int i = 0; ; i++) {
      Mode mode = Empty();
      if (!EnumDisplaySettings(display, i, ref mode)) return false;
      if (mode.Width == width && mode.Height == height && mode.Rate == rate) return true;
    }
  }
  public static bool Is(string display, int width, int height, int rate) {
    Mode mode = Empty();
    return EnumDisplaySettings(display, Current, ref mode) && mode.Width == width && mode.Height == height && mode.Rate == rate;
  }
  // Saved as the display's mode. A fullscreen program that changed the mode puts back the saved one
  // when it leaves, and an unsaved size left the guest at the machine's setting, not the window's.
  // A start still ends at the machine's own setting, as apply-display.ps1 saves that in turn.
  public static int Set(string display, int width, int height, int rate) {
    Mode mode = Empty();
    EnumDisplaySettings(display, Current, ref mode);
    mode.Width = width; mode.Height = height; mode.Rate = rate; mode.Fields = WidthHeightRate;
    return ChangeDisplaySettingsEx(display, ref mode, IntPtr.Zero, UpdateRegistry, IntPtr.Zero);
  }
}
'@
Add-Type $source
# The monitor is away for about a tenth of a second. This is for a Windows that is busy.
$listWaitMilliseconds = 4000
# The size before this one stays in the list, so that Windows comes back to it, and not to some other
# size first, when the monitor returns.
$before = ''
function Size-Screen([int]$width, [int]$height, [int]$rate, [string]$modes) {
    $display = [Screen]::Find()
    if (-not $display) { return 'no driver display' }
    if ([Screen]::Is($display, $width, $height, $rate)) { return "set ${width}x$height" }
    if ([Session]::AppFullscreen()) { return 'held by a fullscreen app' }
    if (-not [Screen]::Lists($display, $width, $height, $rate)) {
        $mode = "${width}x${height}x$rate"
        $extra = @($script:before, $mode) | Where-Object { $_ -and $modes -notmatch "(^|,)$_(,|$)" } | Select-Object -Unique
        $sent = [DriverControl]::Send('SETMODES ' + (@($modes) + $extra -join ','))
        if ($sent -ne 'sent') { return "modes $sent" }
        $clock = [Diagnostics.Stopwatch]::StartNew()
        do {
            Start-Sleep -Milliseconds 30
            $display = [Screen]::Find()
        } until (($display -and [Screen]::Lists($display, $width, $height, $rate)) -or $clock.ElapsedMilliseconds -gt $listWaitMilliseconds)
        if (-not $display) { return 'no driver display' }
        if (-not [Screen]::Lists($display, $width, $height, $rate)) { return 'not listed' }
        $script:before = $mode
    }
    $result = [Screen]::Set($display, $width, $height, $rate)
    if ($result -eq 0) { "set ${width}x$height" } else { "refused $result" }
}
$token = (Get-Content C:\gpupv\token.txt -Raw).Trim()
$listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 8768)
# With the port taken or the listener gone there is nothing to wait for, and a loop that went on
# would only keep a core busy.
try { $listener.Start() } catch { exit 1 }
while ($true) {
    try { $client = $listener.AcceptTcpClient() } catch { exit 1 }
    try {
        $stream = $client.GetStream()
        $parts = [System.IO.StreamReader]::new($stream).ReadLine().Split(' ')
        if ($parts[0] -eq $token -and $parts.Count -eq 5) {
            $writer = [System.IO.StreamWriter]::new($stream)
            $writer.Write((Size-Screen $parts[1] $parts[2] $parts[3] $parts[4]))
            $writer.Flush()
        }
    } catch {
        # A request that makes no sense gets no answer, and the next one is heard.
    } finally {
        $client.Close()
    }
}
