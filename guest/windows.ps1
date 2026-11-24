# Runs in the user's session, as only a program there sees that session's windows. It only answers: on
# loopback, and only requests that start with the agent's token. The one request is `list`, and the
# answer is the visible top-level windows as JSON, in UTF-8 and then Base64, as the agent wraps long
# lines and a title could be cut anywhere. It reads and changes nothing.
#
# Left out, as no user would call them an app window:
#   - windows that are not visible, or that DWM cloaks: a hidden helper window of a program, and the
#     UWP window of an app on another virtual desktop or one Windows keeps suspended. Cloaked windows
#     report as visible, which is why IsWindowVisible alone is not enough.
#   - windows with no area. Programs keep these to receive messages, and some show as visible.
#   - the console's hidden pseudo window (PseudoConsoleWindow) that every terminal program has.
#   - the desktop and the taskbars (Progman, WorkerW, Shell_TrayWnd, Shell_SecondaryTrayWnd), which are
#     Windows' own and not one program's window.
# Menus, tooltips and drop-downs are top-level windows of their own. They are not left out: they come
# with their owner and `popup` set, so the host can place them by the window they belong to.
$source = @'
using System; using System.Collections.Generic; using System.IO; using System.Runtime.InteropServices; using System.Text;
public static class WindowList {
  [StructLayout(LayoutKind.Sequential)] struct Rect { public int Left, Top, Right, Bottom; }
  delegate bool EnumProc(IntPtr window, IntPtr parameter);
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc callback, IntPtr parameter);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr window);
  [DllImport("user32.dll")] static extern bool IsIconic(IntPtr window);
  [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] static extern IntPtr GetWindow(IntPtr window, uint relation);
  [DllImport("user32.dll")] static extern int GetWindowLongW(IntPtr window, int index);
  [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr window, out Rect rect);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetWindowTextLengthW(IntPtr window);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetWindowTextW(IntPtr window, StringBuilder text, int size);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetClassNameW(IntPtr window, StringBuilder name, int size);
  [DllImport("user32.dll")] static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] static extern bool SetProcessDpiAwarenessContext(IntPtr context);
  [DllImport("dwmapi.dll", EntryPoint="DwmGetWindowAttribute")] static extern int DwmRect(IntPtr window, int attribute, out Rect value, int size);
  [DllImport("dwmapi.dll", EntryPoint="DwmGetWindowAttribute")] static extern int DwmInt(IntPtr window, int attribute, out int value, int size);
  [DllImport("kernel32.dll")] static extern IntPtr OpenProcess(uint access, bool inherit, uint process);
  [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode)] static extern bool QueryFullProcessImageNameW(IntPtr process, uint flags, StringBuilder path, ref int size);
  const uint GetOwner = 4, QueryLimitedInformation = 0x1000;
  const int StyleIndex = -16, ExStyleIndex = -20;
  const int Popup = unchecked((int)0x80000000), Caption = 0x00C00000, ToolWindow = 0x80;
  const int ExtendedFrameBounds = 9, Cloaked = 14;
  const int PathChars = 1024;
  // Per-monitor awareness version 2, as a value for SetProcessDpiAwarenessContext.
  static readonly IntPtr PerMonitorV2 = new IntPtr(-4);
  static readonly string[] NotAppWindows = { "PseudoConsoleWindow", "Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd" };
  // The classes that are menus and tooltips whatever their style says.
  static readonly string[] AlwaysPopups = { "#32768", "tooltips_class32" };

  // Without this Windows hands a program that doesn't say it is aware of the scale the coordinates of
  // a 100% screen, and every rectangle would be wrong on a guest at 150% or 200%.
  public static void BeAware() {
    try { if (SetProcessDpiAwarenessContext(PerMonitorV2)) return; } catch (EntryPointNotFoundException) { }
    SetProcessDPIAware();
  }

  static string Text(IntPtr window) {
    int length = GetWindowTextLengthW(window);
    if (length <= 0) return "";
    StringBuilder text = new StringBuilder(length + 1);
    GetWindowTextW(window, text, text.Capacity);
    return text.ToString();
  }

  static string Class(IntPtr window) {
    StringBuilder name = new StringBuilder(256);
    GetClassNameW(window, name, name.Capacity);
    return name.ToString();
  }

  // The file name only. A program Windows won't let this one open, such as a protected one, has none.
  static string Exe(uint process, Dictionary<uint, string> known) {
    string name;
    if (known.TryGetValue(process, out name)) return name;
    name = "";
    IntPtr handle = OpenProcess(QueryLimitedInformation, false, process);
    if (handle != IntPtr.Zero) {
      StringBuilder path = new StringBuilder(PathChars);
      int size = path.Capacity;
      if (QueryFullProcessImageNameW(handle, 0, path, ref size)) name = Path.GetFileName(path.ToString());
      CloseHandle(handle);
    }
    known[process] = name;
    return name;
  }

  static Rect FrameOf(IntPtr window, Rect rect) {
    Rect frame;
    return DwmRect(window, ExtendedFrameBounds, out frame, Marshal.SizeOf(typeof(Rect))) == 0 ? frame : rect;
  }

  static bool IsCloaked(IntPtr window) {
    int how;
    return DwmInt(window, Cloaked, out how, 4) == 0 && how != 0;
  }

  static void Quote(StringBuilder json, string text) {
    json.Append('"');
    foreach (char c in text) {
      if (c == '"') json.Append("\\\"");
      else if (c == '\\') json.Append("\\\\");
      else if (c < ' ') json.Append("\\u" + ((int)c).ToString("x4"));
      else json.Append(c);
    }
    json.Append('"');
  }

  static void Box(StringBuilder json, string name, Rect box) {
    json.Append(",\"" + name + "\":{\"left\":" + box.Left + ",\"top\":" + box.Top + ",\"right\":" + box.Right + ",\"bottom\":" + box.Bottom + "}");
  }

  static string Bool(bool value) { return value ? "true" : "false"; }

  public static string Json() {
    IntPtr foreground = GetForegroundWindow();
    Dictionary<uint, string> programs = new Dictionary<uint, string>();
    StringBuilder json = new StringBuilder("{\"windows\":[");
    int z = 0;
    // EnumWindows goes from the topmost window down, so the count is the window's place in the stack.
    EnumWindows(delegate(IntPtr window, IntPtr parameter) {
      if (!IsWindowVisible(window) || IsCloaked(window)) return true;
      string name = Class(window);
      if (Array.IndexOf(NotAppWindows, name) >= 0) return true;
      Rect rect;
      if (!GetWindowRect(window, out rect)) return true;
      Rect frame = FrameOf(window, rect);
      bool minimized = IsIconic(window);
      // A minimized window has no area on the screen, and is still one the user has.
      if (!minimized && (frame.Right <= frame.Left || frame.Bottom <= frame.Top)) return true;
      IntPtr owner = GetWindow(window, GetOwner);
      int style = GetWindowLongW(window, StyleIndex), extended = GetWindowLongW(window, ExStyleIndex);
      // An owned pop-up with no title bar is a menu, tooltip or drop-down. One with a title bar is a
      // dialog, which is a window of the app. A pop-up with no owner is a frameless app window.
      bool popup = Array.IndexOf(AlwaysPopups, name) >= 0 || (owner != IntPtr.Zero && (style & Popup) != 0 && (style & Caption) != Caption);
      uint process;
      GetWindowThreadProcessId(window, out process);
      if (z > 0) json.Append(',');
      json.Append("{\"handle\":" + window.ToInt64() + ",\"owner\":" + owner.ToInt64() + ",\"pid\":" + process + ",\"exe\":");
      Quote(json, Exe(process, programs));
      json.Append(",\"class\":");
      Quote(json, name);
      json.Append(",\"title\":");
      Quote(json, Text(window));
      Box(json, "frame", frame);
      Box(json, "rect", rect);
      json.Append(",\"z\":" + z + ",\"minimized\":" + Bool(minimized) + ",\"tool_window\":" + Bool((extended & ToolWindow) != 0)
        + ",\"popup\":" + Bool(popup) + ",\"foreground\":" + Bool(window == foreground) + "}");
      z++;
      return true;
    }, IntPtr.Zero);
    return json.Append("]}").ToString();
  }
}
'@
Add-Type $source
[WindowList]::BeAware()
$token = (Get-Content C:\gpupv\token.txt -Raw).Trim()
$listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 8769)
# With the port taken or the listener gone there is nothing to wait for, and a loop that went on
# would only keep a core busy.
try { $listener.Start() } catch { exit 1 }
while ($true) {
    try { $client = $listener.AcceptTcpClient() } catch { exit 1 }
    try {
        $stream = $client.GetStream()
        $parts = [System.IO.StreamReader]::new($stream).ReadLine().Split(' ')
        if ($parts[0] -eq $token -and $parts.Count -eq 2 -and $parts[1] -eq 'list') {
            $writer = [System.IO.StreamWriter]::new($stream)
            $writer.Write([Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes([WindowList]::Json())))
            $writer.Flush()
        }
    } catch {
        # A request that makes no sense gets no answer, and the next one is heard.
    } finally {
        $client.Close()
    }
}
