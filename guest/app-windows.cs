// Runs in the user's session and gives the host the app windows of this machine, one at a time.
//
// One topmost, click-through window (the overlay) covers the screen. Every app window has a cell in
// it, filled with a live DWM thumbnail at 1:1, and the host crops the cells out of the screen's
// picture. The real window is moved so that its frame sits exactly under its cell ("parked"), so a
// click at a point of a cell is a click at the same point of the real window. A thumbnail is still
// what is shown, because a popup of one window can lie over a neighbour's real window.
//
// The host talks to it on TCP port 8769: the token, a line, and then one JSON object a line both
// ways. The messages are described in the app's notes on app windows. Everything that touches a
// window runs on the UI thread. The threads that read and write the socket only hand it work.
//
// C# 5 only, as the compiler that ships with Windows speaks no more.
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Drawing;
using System.Drawing.Imaging;
using System.Globalization;
using System.IO;
using System.Net;
using System.Net.Sockets;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Web.Script.Serialization;
using System.Windows.Forms;

namespace Guesthouse {

public class AppWindows : Form
{
    // ================================================================ Win32
    [StructLayout(LayoutKind.Sequential)] struct RECT { public int L, T, R, B; }
    [StructLayout(LayoutKind.Sequential)] struct SIZE { public int W, H; }

    // Native layout: DWORD flags; RECT dest; RECT src; BYTE opacity; BOOL visible; BOOL clientOnly  (48 bytes)
    [StructLayout(LayoutKind.Sequential)]
    struct DWM_THUMBNAIL_PROPERTIES
    {
        public uint dwFlags;
        public RECT rcDestination;
        public RECT rcSource;
        public byte opacity;
        [MarshalAs(UnmanagedType.Bool)] public bool fVisible;
        [MarshalAs(UnmanagedType.Bool)] public bool fSourceClientAreaOnly;
    }

    [StructLayout(LayoutKind.Sequential)]
    struct BITMAPINFOHEADER
    {
        public int biSize, biWidth, biHeight; public short biPlanes, biBitCount;
        public int biCompression, biSizeImage, biXPelsPerMeter, biYPelsPerMeter, biClrUsed, biClrImportant;
    }

    // rgstate[0] is the title bar itself, then five buttons.
    [StructLayout(LayoutKind.Sequential)]
    struct TITLEBARINFO
    {
        public int cbSize;
        public RECT rcTitleBar;
        [MarshalAs(UnmanagedType.ByValArray, SizeConst = 6)] public int[] rgstate;
    }

    [StructLayout(LayoutKind.Sequential)]
    struct GUITHREADINFO
    {
        public int cbSize, flags;
        public IntPtr hwndActive, hwndFocus, hwndCapture, hwndMenuOwner, hwndMoveSize, hwndCaret;
        public RECT rcCaret;
    }

    delegate bool EnumProc(IntPtr h, IntPtr l);
    delegate void WinEventProc(IntPtr hook, uint evt, IntPtr hwnd, int idObject, int idChild, uint thread, uint time);

    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc p, IntPtr l);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern bool IsIconic(IntPtr h);
    [DllImport("user32.dll")] static extern bool IsZoomed(IntPtr h);
    [DllImport("user32.dll")] static extern bool IsWindow(IntPtr h);
    [DllImport("user32.dll")] static extern bool IsHungAppWindow(IntPtr h);
    [DllImport("user32.dll")] static extern IntPtr GetWindow(IntPtr h, uint cmd);
    [DllImport("user32.dll")] static extern IntPtr GetAncestor(IntPtr h, uint flags);
    [DllImport("user32.dll")] static extern IntPtr GetDesktopWindow();
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] static extern bool BringWindowToTop(IntPtr h);
    [DllImport("user32.dll")] static extern bool ShowWindowAsync(IntPtr h, int cmd);
    [DllImport("user32.dll")] static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] static extern bool AttachThreadInput(uint from, uint to, bool attach);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] static extern bool GetTitleBarInfo(IntPtr h, ref TITLEBARINFO info);
    [DllImport("user32.dll")] static extern bool GetGUIThreadInfo(uint thread, ref GUITHREADINFO info);
    [DllImport("user32.dll", EntryPoint = "GetWindowLongPtrW")] static extern IntPtr GetWindowLongPtr(IntPtr h, int i);
    [DllImport("user32.dll", EntryPoint = "SetWindowLongPtrW")] static extern IntPtr SetWindowLongPtr(IntPtr h, int i, IntPtr v);
    [DllImport("user32.dll")] static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int w, int hh, uint f);
    [DllImport("user32.dll")] static extern bool SetLayeredWindowAttributes(IntPtr h, uint key, byte alpha, uint flags);
    [DllImport("user32.dll")] static extern bool SetProcessDpiAwarenessContext(IntPtr c);
    [DllImport("user32.dll")] static extern int GetSystemMetrics(int i);
    [DllImport("user32.dll")] static extern bool SystemParametersInfo(uint action, uint param, out RECT rect, uint flags);
    [DllImport("user32.dll")] static extern IntPtr SetWinEventHook(uint eventMin, uint eventMax, IntPtr hmod, WinEventProc proc, uint pid, uint thread, uint flags);
    [DllImport("user32.dll")] static extern bool UnhookWinEvent(IntPtr hook);
    [DllImport("user32.dll")] static extern IntPtr OpenInputDesktop(uint flags, bool inherit, uint access);
    [DllImport("user32.dll")] static extern bool CloseDesktop(IntPtr desktop);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern bool GetUserObjectInformation(IntPtr obj, int index, StringBuilder info, int length, out int needed);
    [DllImport("wtsapi32.dll")] static extern bool WTSRegisterSessionNotification(IntPtr h, uint flags);
    [DllImport("wtsapi32.dll")] static extern bool WTSUnRegisterSessionNotification(IntPtr h);
    [DllImport("dwmapi.dll")] static extern int DwmRegisterThumbnail(IntPtr dest, IntPtr src, out IntPtr thumb);
    [DllImport("dwmapi.dll")] static extern int DwmUnregisterThumbnail(IntPtr thumb);
    [DllImport("dwmapi.dll")] static extern int DwmUpdateThumbnailProperties(IntPtr thumb, ref DWM_THUMBNAIL_PROPERTIES p);
    [DllImport("dwmapi.dll")] static extern int DwmQueryThumbnailSourceSize(IntPtr thumb, out SIZE s);
    [DllImport("dwmapi.dll")] static extern int DwmGetWindowAttribute(IntPtr h, int attr, out int v, int cb);
    [DllImport("dwmapi.dll")] static extern int DwmGetWindowAttribute(IntPtr h, int attr, out RECT v, int cb);
    [DllImport("user32.dll")] static extern IntPtr GetDC(IntPtr h);
    [DllImport("user32.dll")] static extern int ReleaseDC(IntPtr h, IntPtr dc);
    [DllImport("gdi32.dll")] static extern IntPtr CreateCompatibleDC(IntPtr dc);
    [DllImport("gdi32.dll")] static extern IntPtr CreateCompatibleBitmap(IntPtr dc, int w, int h);
    [DllImport("gdi32.dll")] static extern IntPtr SelectObject(IntPtr dc, IntPtr obj);
    [DllImport("gdi32.dll")] static extern bool BitBlt(IntPtr dst, int x, int y, int w, int h, IntPtr src, int sx, int sy, uint rop);
    [DllImport("gdi32.dll")] static extern int GetDIBits(IntPtr dc, IntPtr bmp, uint start, uint lines, int[] bits, ref BITMAPINFOHEADER info, uint usage);
    [DllImport("gdi32.dll")] static extern bool DeleteObject(IntPtr obj);
    [DllImport("gdi32.dll")] static extern bool DeleteDC(IntPtr dc);
    [DllImport("kernel32.dll")] static extern uint GetCurrentProcessId();
    [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();
    [DllImport("kernel32.dll")] static extern IntPtr OpenProcess(uint access, bool inherit, uint pid);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] static extern int GetApplicationUserModelId(IntPtr process, ref uint length, StringBuilder id);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] static extern int GetPackageFullName(IntPtr process, ref uint length, StringBuilder name);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] static extern int GetPackagePathByFullName(string fullName, ref uint length, StringBuilder path);
    [DllImport("user32.dll")] static extern bool EnumChildWindows(IntPtr parent, EnumProc p, IntPtr l);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern uint PrivateExtractIcons(string file, int index, int cx, int cy, IntPtr[] icons, int[] ids, uint count, uint flags);
    [DllImport("user32.dll")] static extern bool DestroyIcon(IntPtr icon);
    [DllImport("user32.dll")] static extern IntPtr SendMessageTimeout(IntPtr h, uint msg, IntPtr w, IntPtr l, uint flags, uint timeout, out IntPtr result);
    [DllImport("user32.dll", EntryPoint = "GetClassLongPtrW")] static extern IntPtr GetClassLongPtr(IntPtr h, int i);
    [DllImport("shlwapi.dll", CharSet = CharSet.Unicode)] static extern int SHLoadIndirectString(string source, StringBuilder output, uint size, IntPtr reserved);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] static extern bool QueryFullProcessImageNameW(IntPtr process, uint flags, StringBuilder path, ref int size);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)] static extern bool MoveFileEx(string existing, string replacement, uint flags);

    const int DWMWA_CAPTION_BUTTON_BOUNDS = 5, DWMWA_EXTENDED_FRAME_BOUNDS = 9, DWMWA_CLOAKED = 14;
    const uint DWM_TNP_RECTDESTINATION = 1, DWM_TNP_RECTSOURCE = 2, DWM_TNP_OPACITY = 4, DWM_TNP_VISIBLE = 8, DWM_TNP_SOURCECLIENTAREAONLY = 0x10;
    const int GWL_STYLE = -16, GWL_EXSTYLE = -20;
    const uint GW_OWNER = 4, GA_PARENT = 1;
    const uint LWA_ALPHA = 2;
    const uint SWP_NOSIZE = 1, SWP_NOMOVE = 2, SWP_NOZORDER = 4, SWP_NOACTIVATE = 0x10, SWP_SHOWWINDOW = 0x40, SWP_ASYNCWINDOWPOS = 0x4000;
    const uint MOVEFILE_REPLACE_EXISTING = 1;
    const int SM_CXSCREEN = 0, SM_CYSCREEN = 1, SM_XVIRTUALSCREEN = 76, SM_YVIRTUALSCREEN = 77, SM_CXVIRTUALSCREEN = 78, SM_CYVIRTUALSCREEN = 79;
    const uint SPI_GETWORKAREA = 0x30;
    const int WM_DISPLAYCHANGE = 0x7E, WM_SETTINGCHANGE = 0x1A, WM_WTSSESSION_CHANGE = 0x2B1;
    const uint WM_CLOSE = 0x10;
    const int SW_MINIMIZE = 6, SW_RESTORE = 9;
    const int GUI_INMOVESIZE = 2;
    const int STATE_SYSTEM_INVISIBLE = 0x8000, STATE_SYSTEM_OFFSCREEN = 0x10000;
    const int WTS_SESSION_LOCK = 7, WTS_SESSION_UNLOCK = 8;
    const uint DESKTOP_READOBJECTS = 1;
    const int UOI_NAME = 2;
    const uint PROCESS_QUERY_LIMITED_INFORMATION = 0x1000;

    // Window styles are read as 64-bit and masked to 32 bits before testing bits.
    const long MASK32 = 0xFFFFFFFFL;
    const long WS_SYSMENU = 0x00080000L, WS_MINIMIZEBOX = 0x00020000L, WS_MAXIMIZEBOX = 0x00010000L;
    const long WS_POPUP = 0x80000000L, WS_CAPTION = 0x00C00000L, WS_THICKFRAME = 0x00040000L;
    const long WS_EX_TOPMOST = 0x8L, WS_EX_TRANSPARENT = 0x20L, WS_EX_TOOLWINDOW = 0x80L, WS_EX_LAYERED = 0x80000L, WS_EX_NOACTIVATE = 0x08000000L;

    // WinEvents. OBJECT events carry idObject 0 (OBJID_WINDOW) when they are about the window itself.
    const uint EV_FOREGROUND = 0x0003, EV_MOVESIZESTART = 0x000A, EV_MOVESIZEEND = 0x000B, EV_MINIMIZESTART = 0x0016, EV_MINIMIZEEND = 0x0017;
    const uint EV_DESTROY = 0x8001, EV_SHOW = 0x8002, EV_HIDE = 0x8003, EV_LOCATION = 0x800B, EV_NAME = 0x800C, EV_CLOAKED = 0x8017, EV_UNCLOAKED = 0x8018;
    const uint WINEVENT_OUTOFCONTEXT = 0, WINEVENT_SKIPOWNPROCESS = 2;

    // ================================================================ Wire, files and timings
    const int Port = 8769;
    // The host's end of QEMU's private network. The loopback address is for a program in the guest, which the token still has to let in.
    const string HostAddress = "10.0.2.2";
    const int TokenWaitMs = 5000, TokenMaxBytes = 256, LineMaxBytes = 1 << 20, WriteTimeoutMs = 5000;
    const string Dir = @"C:\gpupv\";
    const string ConfigFile = Dir + "app-windows.cfg";    // lab switches only
    const string FeedFile = Dir + "app-windows-feed.json";   // the layout lines as a file, with feed=1
    const string FeedTmp = Dir + "app-windows-feed.json.tmp";
    const string ErrorFile = Dir + "app-windows-error.txt";
    const string QuitFile = Dir + "app-windows.quit";    // written by the stop script, so the layered style is undone first
    const int ProtocolVersion = 1;

    const int HeartbeatMs = 250;                // config, quit file, input desktop, pending-feed retry, dormant expiry; not a layout poll
    const int DebugLabelH = 16;                 // only used when debug=1
    const int MinCellSide = 8;                  // the smallest window picture that gets a cell
    const int AlphaForCure = 254;
    const uint SRCCOPY = 0x00CC0020, CAPTUREBLT = 0x40000000;
    const int ShadowMaxLum = 90, StepLum = 12, InsetMax = 40;
    const int InsetFirstMs = 60, InsetRetryMs = 100, InsetTriesMax = 10;
    const int DebounceMs = 30, TitleMs = 500, ScanMs = 1000, GraceMs = 2000, PopupGraceMs = 300, SettleMs = 400;
    const int PendingStepMs = 40, RestoreRetryMs = 60, FocusRetryMs = 100;
    const int MinimizeWaitMs = 1500, RestoreWaitMs = 3000, FocusWaitMs = 800, ResizeWaitMs = 1000;
    // A window that is put back more often than this in a row, and keeps leaving, is left alone for a while.
    const int ParkTriesMax = 6, ParkBackoffMs = 3000, ParkSettleMs = 120;

    // Windows that are never app windows. Exact class names.
    static readonly string[] AlwaysSkip = new string[] {
        "Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd", "SysShadow", "PseudoConsoleWindow",
        "DummyDWMListenerWindow", "MSCTFIME UI", "IME", "Default IME", "NotifyIconOverflowWindow",
        "TopLevelWindowForOverflowXamlIsland", "XamlExplorerHostIslandWindow" };

    // Menus, tips and drop-downs. Matched as substrings, since WinForms adds suffixes (WindowsForms10.tooltips_class32.app.0.xxx).
    static readonly string[] PopupClassParts = new string[] {
        "#32768", "tooltips_class32", "ComboLBox", "Xaml_WindowedPopupClass", "PopupWindowSiteBridge", "DropDown" };

    // ================================================================ State
    class Cell
    {
        public IntPtr Src, Thumb;
        public int Order;                       // creation order, never changes; placement order and tie-break
        public int Stamp;                       // starts as Order; a focus raises it, so of two overflow cells the one asked for last is on top
        public uint Pid;
        public string Exe = "?", Cls = "", Title = "";
        public string App = "", Name = "", ExePath = "";   // which program the window is of (see Identify)
        public uint AppPid;                     // the process that is the program: the frame host's CoreWindow's for a Store app
        public int IdTries; public bool IdSettled;
        public long Style, ExStyle;
        public IntPtr Owner;                    // direct owner window (0 = none)
        public bool Popup;
        public IntPtr Parent;                   // for popups: owner root, else window it sits over (0 = none)
        public bool HasRel; public int RelX, RelY;   // popup frame origin minus parent frame origin
        public bool Active;                     // qualifies right now (has a thumbnail)
        public long DormantAt;
        public bool Iconic, Cloaked, Fg;
        public int Z;                           // 1 = top of the z-order
        public int SrcW, SrcH;                  // size of the window's picture (what DWM thumbnails)
        public int WantW, WantH;                // size it should have once it is moved (during settle)
        public int CellW, CellH;                // size of the cell as shown (never more than the source)
        public int X, Y;                        // top-left of the cell inside the overlay
        public bool Placed, Overflow, Hidden;   // Hidden: an overflow cell lies over it
        public long MoveAt;                     // when to move a cell that outgrew its slot (0 = not pending)
        public RECT Frame, Win;                 // last known real rectangles (screen coordinates)
        public bool Cured; public int CureTries;
        public bool NameDirty, LocDirty, ThumbDirty, ThumbApplied;
        public long TitleAt;
        public int CaptionH, ButtonsW;          // what Windows says of the title bar; 0 when it cannot be known
        public int ParkTries; public long ParkAfter;
        // A popup's window has an invisible margin around the menu for its shadow, which DWM thumbnails as a dark band.
        // The insets say how much of each side is shadow; the cell and the frame we report are the menu alone.
        public int InL, InT, InR, InB; public bool InDone; public long InAt; public int InTries;
        public bool HaveCand; public int CandL, CandT, CandR, CandB;   // the margins read last time, to be read again before they count
    }

    class Probe
    {
        public IntPtr H, Owner; public uint Pid; public string Cls; public long Style, Ex;
        public bool Popup, Iconic; public RECT Frame;
    }

    // A request that waits for Windows to do what it was asked.
    class Pending
    {
        public Link Link; public long N; public string Kind; public IntPtr Win;
        public long Deadline, NextTry; public int W, H;
    }

    // The host's connection. The UI thread only puts lines in; the writer thread sends them, so a host that stops reading cannot stop the parking.
    class Link
    {
        readonly TcpClient client;
        readonly NetworkStream stream;
        readonly object gate = new object();
        readonly Queue<string> lines = new Queue<string>();
        string latestLayout;        // only the newest layout matters, the ones it replaced are not sent
        bool closed;
        public bool Side;           // a short look or job beside the host's driving connection: never the current link

        public Link(TcpClient client, NetworkStream stream) { this.client = client; this.stream = stream; }

        public void Send(string line) { lock (gate) { lines.Enqueue(line); Monitor.Pulse(gate); } }
        public void SendLayout(string line) { lock (gate) { latestLayout = line; Monitor.Pulse(gate); } }

        public void Close()
        {
            lock (gate) { closed = true; Monitor.Pulse(gate); }
            try { client.Close(); } catch { }
        }

        public void WriteLoop()
        {
            try
            {
                while (true)
                {
                    string next;
                    lock (gate)
                    {
                        while (!closed && lines.Count == 0 && latestLayout == null) Monitor.Wait(gate);
                        if (closed) return;
                        if (lines.Count > 0) next = lines.Dequeue();
                        else { next = latestLayout; latestLayout = null; }
                    }
                    byte[] bytes = new UTF8Encoding(false).GetBytes(next + "\n");
                    stream.Write(bytes, 0, bytes.Length);
                    stream.Flush();
                }
            }
            catch { Close(); }
        }
    }

    Dictionary<IntPtr, Cell> cells = new Dictionary<IntPtr, Cell>();
    List<IntPtr> enumList = new List<IntPtr>();
    EnumProc enumProc;
    WinEventProc winEventProc;                  // kept in a field so the GC cannot collect the delegate
    List<IntPtr> hooks = new List<IntPtr>();
    System.Windows.Forms.Timer work = new System.Windows.Forms.Timer();
    System.Windows.Forms.Timer heartbeat = new System.Windows.Forms.Timer();
    System.Windows.Forms.Timer pendingTimer = new System.Windows.Forms.Timer();
    static readonly Stopwatch Clock = Stopwatch.StartNew();
    Font font = new Font("Segoe UI", 8f);
    uint myPid = GetCurrentProcessId();
    Dictionary<uint, string> exeNames = new Dictionary<uint, string>();
    long exeNamesAt;
    JavaScriptSerializer json = new JavaScriptSerializer();
    List<Pending> pending = new List<Pending>();

    readonly string token;
    readonly string session = Guid.NewGuid().ToString("N");
    readonly object linkGate = new object();
    int sideLinks; const int SideLinksMax = 4;
    Link current;                               // the host's connection; a newer one replaces it
    int vx, vy, ovW, ovH;                       // virtual screen origin and size = overlay
    int packL, packT, packR, packB;             // where cells are packed, inside the overlay: the screen less the taskbar
    int orderCounter, seq;
    long workDue, lastTop, lastScan;
    bool scanDue = true, shutDown, quitting;
    bool feedPending; string lastCore = "", lastLayout = "";
    string desktopKind = "normal"; bool sessionLocked;

    // ---- lab switches, read from the config file, one key=value a line
    bool cureOn = true, debugOn, feedToFile, workAreaOn = true;
    DateTime cfgTime = DateTime.MinValue;

    // ================================================================ Form plumbing
    protected override CreateParams CreateParams
    {
        get
        {
            CreateParams cp = base.CreateParams;
            cp.ExStyle |= (int)(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_LAYERED | WS_EX_TOPMOST);
            return cp;
        }
    }
    protected override bool ShowWithoutActivation { get { return true; } }

    AppWindows(string token)
    {
        this.token = token;
        FormBorderStyle = FormBorderStyle.None;
        ShowInTaskbar = false;
        StartPosition = FormStartPosition.Manual;
        BackColor = Color.FromArgb(40, 40, 48);
        SetStyle(ControlStyles.AllPaintingInWmPaint | ControlStyles.OptimizedDoubleBuffer, true);
        enumProc = EnumCollect;
        winEventProc = OnWinEvent;
        FitToScreen();
        work.Tick += OnWorkTick;
        heartbeat.Interval = HeartbeatMs;
        heartbeat.Tick += OnHeartbeat;
        pendingTimer.Interval = PendingStepMs;
        pendingTimer.Tick += OnPendingTick;
    }

    void FitToScreen()
    {
        vx = GetSystemMetrics(SM_XVIRTUALSCREEN); vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        ovW = GetSystemMetrics(SM_CXVIRTUALSCREEN); ovH = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        Bounds = new Rectangle(vx, vy, ovW, ovH);
        FitPackArea();
    }

    // Windows pack inside the work area, so that a click at the bottom of a cell is not a click at the taskbar, which lies above everything.
    void FitPackArea()
    {
        packL = 0; packT = 0; packR = ovW; packB = ovH;
        RECT wa;
        if (workAreaOn && SystemParametersInfo(SPI_GETWORKAREA, 0, out wa, 0))
        {
            packL = Math.Max(0, wa.L - vx); packT = Math.Max(0, wa.T - vy);
            packR = Math.Min(ovW, wa.R - vx); packB = Math.Min(ovH, wa.B - vy);
        }
    }

    protected override void OnHandleCreated(EventArgs e)
    {
        base.OnHandleCreated(e);
        // Fully opaque; only here so that WS_EX_LAYERED + WS_EX_TRANSPARENT makes the overlay click-through.
        SetLayeredWindowAttributes(Handle, 0, 255, LWA_ALPHA);
        WTSRegisterSessionNotification(Handle, 0);
        ReadConfig();
        try { File.Delete(QuitFile); } catch { }
        InstallHooks();
        heartbeat.Start();
        scanDue = true; Kick();
        Thread listening = new Thread(Listen);
        listening.IsBackground = true;
        listening.Start();
    }

    protected override void OnFormClosed(FormClosedEventArgs e)
    {
        Shutdown();
        base.OnFormClosed(e);
    }

    protected override void WndProc(ref Message m)
    {
        if (m.Msg == WM_DPICHANGED)
        {
            // The overlay keeps covering the screen in physical pixels: Windows' suggested rectangle is not taken.
            FitToScreen();
            foreach (Cell c in cells.Values) { c.ThumbDirty = true; if (!c.Popup) ReadCaption(c); }
            scanDue = true; Kick();
            m.Result = IntPtr.Zero;
            return;
        }
        if (m.Msg == WM_DISPLAYCHANGE) OnDisplayChange();
        else if (m.Msg == WM_SETTINGCHANGE) { FitPackArea(); }
        else if (m.Msg == WM_WTSSESSION_CHANGE)
        {
            int what = m.WParam.ToInt32();
            if (what == WTS_SESSION_LOCK) sessionLocked = true;
            else if (what == WTS_SESSION_UNLOCK) sessionLocked = false;
            CheckDesktop();
        }
        base.WndProc(ref m);
    }

    // The screen size changed: the overlay follows, and cells that no longer fit are placed again (the one time cells may jump).
    void OnDisplayChange()
    {
        FitToScreen();
        KeepScale();
        foreach (Cell c in cells.Values)
        {
            // A larger screen may have room for an overflow cell, and a window cut to the old screen gets its whole size back.
            c.CellW = Math.Min(c.SrcW > 0 ? c.SrcW - c.InL - c.InR : c.CellW, ovW); c.CellH = Math.Min(c.SrcH > 0 ? c.SrcH - c.InT - c.InB : c.CellH, ovH);
            // Every window is placed again, in order, so the packing is the one the need was worked out for.
            if (!c.Popup) c.Placed = false;
            if (c.Placed && (c.X + c.CellW > packR || c.Y + c.CellH > packB)) c.Placed = false;
            c.ThumbDirty = true;
        }
        scanDue = true; Kick();
    }

    void Shutdown()
    {
        if (shutDown) return;
        shutDown = true;
        work.Stop(); heartbeat.Stop(); pendingTimer.Stop();
        foreach (IntPtr hk in hooks) UnhookWinEvent(hk);
        hooks.Clear();
        WTSUnRegisterSessionNotification(Handle);
        ReleaseAll();                           // unregisters thumbnails and undoes the layered cure
        Link link;
        lock (linkGate) { link = current; current = null; }
        if (link != null) link.Close();
    }

    static long Now() { return Clock.ElapsedMilliseconds; }

    // ================================================================ Events
    void InstallHooks()
    {
        // Narrow ranges only: one wide range would deliver every focus, state and value change in the system.
        uint flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
        uint[,] ranges = new uint[,] {
            { EV_FOREGROUND, EV_FOREGROUND }, { EV_MOVESIZESTART, EV_MOVESIZEEND }, { EV_MINIMIZESTART, EV_MINIMIZEEND },
            { EV_DESTROY, EV_HIDE }, { EV_LOCATION, EV_NAME }, { EV_CLOAKED, EV_UNCLOAKED } };
        for (int i = 0; i < ranges.GetLength(0); i++)
        {
            IntPtr hk = SetWinEventHook(ranges[i, 0], ranges[i, 1], IntPtr.Zero, winEventProc, 0, 0, flags);
            if (hk != IntPtr.Zero) hooks.Add(hk);
        }
    }

    // Runs on the UI thread. Cheap on purpose: it only sets flags and arms the timer.
    void OnWinEvent(IntPtr hook, uint evt, IntPtr hwnd, int idObject, int idChild, uint thread, uint time)
    {
        if (shutDown || hwnd == IntPtr.Zero) return;
        if (idObject != 0 || idChild != 0) return;      // caret, cursor, children: the cursor alone fires LOCATIONCHANGE on every move
        Cell c;
        switch (evt)
        {
            case EV_LOCATION:
                if (cells.TryGetValue(hwnd, out c)) { c.LocDirty = true; Kick(); }
                break;
            case EV_MOVESIZESTART:
            case EV_MOVESIZEEND:
                // The end is when a window the user dragged away is put back.
                if (cells.TryGetValue(hwnd, out c)) { c.LocDirty = true; Kick(); }
                break;
            case EV_NAME:
                if (cells.TryGetValue(hwnd, out c))
                {
                    // A title that changes every frame must not wake the pass every frame: wait out TitleMs.
                    c.NameDirty = true;
                    long since = Now() - c.TitleAt;
                    ArmWork(since >= TitleMs ? DebounceMs : (int)(TitleMs - since));
                }
                break;
            case EV_FOREGROUND:
                Kick();                                  // foreground only changes the "focus" flag, found by the next pass
                break;
            case EV_DESTROY:
                if (cells.ContainsKey(hwnd)) { scanDue = true; Kick(); }
                break;
            default:                                     // show, hide, cloak, uncloak, minimize, restore
                if (GetAncestor(hwnd, GA_PARENT) == GetDesktopWindow()) { scanDue = true; Kick(); }
                break;
        }
    }

    // Arm the work timer to fire in DebounceMs, unless it already fires sooner.
    void Kick() { ArmWork(DebounceMs); }

    void ArmWork(int ms)
    {
        if (shutDown) return;
        if (ms < 1) ms = 1;
        long due = Now() + ms;
        if (work.Enabled && workDue <= due) return;
        work.Stop();
        work.Interval = ms;
        workDue = due;
        work.Start();
    }

    void OnWorkTick(object sender, EventArgs e)
    {
        work.Stop();
        try { Work(); }
        catch (Exception ex) { LogError(ex); }
    }

    // Only a slow safety net. Hooks can miss windows of higher integrity and windows that change without an event.
    void OnHeartbeat(object sender, EventArgs e)
    {
        try
        {
            long now = Now();
            ReadConfig();
            if (File.Exists(QuitFile)) quitting = true;
            if (quitting) { Close(); return; }
            CheckDesktop();
            if (now - lastTop >= 1000) { lastTop = now; RaiseOverlay(); }
            if (now - lastScan >= ScanMs) scanDue = true;
            foreach (Cell c in cells.Values) if (!c.Active) { scanDue = true; break; }   // lets dormant cells expire
            if (scanDue || feedPending) Kick();
        }
        catch (Exception ex) { LogError(ex); }
    }

    void RaiseOverlay()
    {
        // Menus and tips are topmost too; putting the overlay back on top keeps real windows out of the picture.
        SetWindowPos(Handle, new IntPtr(-1), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }

    static void LogError(Exception ex)
    {
        try { File.WriteAllText(ErrorFile, DateTime.Now.ToString("s") + "\r\n" + ex.ToString()); } catch { }
    }

    // ================================================================ Lab switches
    // insets=0 shows popups whole, shadow margin and all; debug=1 draws a label and a border at each cell, layered=0 turns the layered cure off, feed=1 also writes each layout line to a file,
    // workarea=0 packs cells over the taskbar as well. Anything else in the file is ignored.
    void ReadConfig()
    {
        try
        {
            FileInfo fi = new FileInfo(ConfigFile);
            DateTime stamp = fi.Exists ? fi.LastWriteTimeUtc : DateTime.MinValue;
            if (stamp == cfgTime) return;
            cfgTime = stamp;
            bool wasCure = cureOn;
            cureOn = true; debugOn = false; feedToFile = false; workAreaOn = true; insetsOn = true;
            if (fi.Exists)
            {
                foreach (string line in File.ReadAllLines(fi.FullName))
                {
                    string[] kv = line.Split(new char[] { '=' }, 2);
                    if (kv.Length != 2) continue;
                    string key = kv[0].Trim().ToLowerInvariant();
                    bool on = kv[1].Trim() == "1";
                    if (key == "layered") cureOn = on;
                    else if (key == "debug") debugOn = on;
                    else if (key == "feed") feedToFile = on;
                    else if (key == "workarea") workAreaOn = on;
                    else if (key == "insets") insetsOn = on;
                }
            }
            FitPackArea();
            if (wasCure && !cureOn) foreach (Cell c in cells.Values) Uncure(c);
            foreach (Cell c in cells.Values) c.ThumbDirty = true;
            Invalidate();
            scanDue = true; Kick();
        }
        catch { }                                      // file being written: try again next beat
    }

    // ================================================================ Reading windows
    bool EnumCollect(IntPtr h, IntPtr l) { enumList.Add(h); return true; }

    static string ClassOf(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetClassName(h, sb, 256); return sb.ToString(); }
    static string TextOf(IntPtr h) { StringBuilder sb = new StringBuilder(512); GetWindowText(h, sb, 512); return sb.ToString(); }

    // The program's file name, as the host lists it. Cached; the cache is dropped every 30 s so reused process ids are looked up again.
    string ExeOf(uint pid)
    {
        long now = Now();
        if (now - exeNamesAt > 30000) { exeNames.Clear(); exeNamesAt = now; }
        string n;
        if (exeNames.TryGetValue(pid, out n)) return n;
        n = "?";
        IntPtr process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid);
        if (process != IntPtr.Zero)
        {
            StringBuilder path = new StringBuilder(1024);
            int size = path.Capacity;
            if (QueryFullProcessImageNameW(process, 0, path, ref size)) n = Path.GetFileName(path.ToString());
            CloseHandle(process);
        }
        exeNames[pid] = n;
        return n;
    }

    static bool IsKnownPopupClass(string cls)
    {
        for (int i = 0; i < PopupClassParts.Length; i++)
            if (cls.IndexOf(PopupClassParts[i], StringComparison.OrdinalIgnoreCase) >= 0) return true;
        return false;
    }

    // Decides whether a top-level window is one a cell is made for. Cheap tests first.
    Probe ProbeWindow(IntPtr h)
    {
        bool iconic = IsIconic(h);
        if (!IsWindowVisible(h) && !iconic) return null;
        uint pid; GetWindowThreadProcessId(h, out pid);
        if (pid == myPid) return null;
        string cls = ClassOf(h);
        if (Array.IndexOf(AlwaysSkip, cls) >= 0) return null;
        int cloak = 0; DwmGetWindowAttribute(h, DWMWA_CLOAKED, out cloak, 4);
        if (cloak != 0 && !iconic) return null;

        long st = GetWindowLongPtr(h, GWL_STYLE).ToInt64() & MASK32;
        long ex = GetWindowLongPtr(h, GWL_EXSTYLE).ToInt64() & MASK32;
        IntPtr owner = GetWindow(h, GW_OWNER);
        bool known = IsKnownPopupClass(cls);
        bool caption = (st & WS_CAPTION) == WS_CAPTION;
        bool toolish = (ex & (WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE)) != 0;
        // A popup is a menu, tip or drop-down: a known class, or a window with no title bar that is a tool window,
        // never activates, or is an owned WS_POPUP. A heuristic.
        bool popup = known || (!caption && (toolish || (owner != IntPtr.Zero && (st & WS_POPUP) != 0)));
        if ((ex & WS_EX_TOOLWINDOW) != 0 && owner == IntPtr.Zero && !known) return null;

        RECT f = new RECT();
        if (!iconic)
        {
            if (DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, out f, 16) != 0)
                if (!GetWindowRect(h, out f)) return null;
            if (f.R - f.L < MinCellSide || f.B - f.T < MinCellSide) return null;
        }
        Probe p = new Probe();
        p.H = h; p.Owner = owner; p.Pid = pid; p.Cls = cls; p.Style = st; p.Ex = ex; p.Popup = popup; p.Iconic = iconic; p.Frame = f;
        return p;
    }

    // ================================================================ Scan: which windows have cells
    void Scan(long now)
    {
        lastScan = now;
        RaiseOverlay();
        enumList.Clear();
        EnumWindows(enumProc, IntPtr.Zero);
        IntPtr fg = GetForegroundWindow();
        HashSet<IntPtr> seen = new HashSet<IntPtr>();
        int z = 0;
        foreach (IntPtr h in enumList)
        {
            z++;
            Probe p = ProbeWindow(h);
            if (p == null) continue;
            Cell c;
            if (!cells.TryGetValue(h, out c))
            {
                if (p.Iconic) continue;                 // a window first seen minimized has no known size yet; it gets its cell when restored
                c = NewCell(p, now);
                if (c == null) continue;
            }
            else if (!c.Active)
            {
                if (!Wake(c, now)) continue;            // thumbnail could not be registered; stays dormant, tried again next scan
            }
            seen.Add(h);
            c.Z = z;
            c.Fg = (h == fg);
            c.Style = p.Style; c.ExStyle = p.Ex; c.Owner = p.Owner; c.Popup = p.Popup;
            ReadState(c, now);
            c.LocDirty = false;
            if (now - c.TitleAt >= TitleMs) { c.Title = TextOf(h); c.TitleAt = now; c.NameDirty = false; }
            if (!c.IdSettled) Identify(c);
            if (!c.Popup && cureOn) KeepCured(c);
        }
        // Cells whose window did not qualify this time.
        foreach (Cell c in new List<Cell>(cells.Values))
        {
            if (seen.Contains(c.Src)) continue;
            if (!IsWindow(c.Src)) { Release(c); continue; }
            if (c.Active) Sleep(c, now);
            else if (now - c.DormantAt >= (c.Popup ? PopupGraceMs : GraceMs)) Release(c);
        }
    }

    Cell NewCell(Probe p, long now)
    {
        IntPtr t;
        if (DwmRegisterThumbnail(Handle, p.H, out t) != 0) return null;
        int sw = p.Frame.R - p.Frame.L, sh = p.Frame.B - p.Frame.T;
        SIZE s;
        if (DwmQueryThumbnailSourceSize(t, out s) == 0 && s.W > 0 && s.H > 0) { sw = s.W; sh = s.H; }
        if (sw <= 0 || sh <= 0) { DwmUnregisterThumbnail(t); return null; }
        Cell c = new Cell();
        c.Src = p.H; c.Thumb = t; c.Order = ++orderCounter; c.Stamp = c.Order; c.Pid = p.Pid;
        c.Exe = ExeOf(p.Pid); Identify(c); c.Cls = p.Cls; c.Title = TextOf(p.H); c.TitleAt = now;
        c.Style = p.Style; c.ExStyle = p.Ex; c.Owner = p.Owner; c.Popup = p.Popup;
        c.Frame = p.Frame; c.Win = p.Frame;
        c.Active = true; c.ThumbDirty = true;
        c.InDone = !c.Popup || !insetsOn; c.InAt = now + InsetFirstMs;
        c.SrcW = sw; c.SrcH = sh;
        c.CellW = Math.Min(sw, ovW); c.CellH = Math.Min(sh, ovH);
        cells[p.H] = c;
        if (!c.Popup) ReadCaption(c);
        if (!c.Popup && cureOn) Cure(c);              // "when it first appears": before it can be covered
        return c;
    }

    bool Wake(Cell c, long now)
    {
        IntPtr t;
        if (DwmRegisterThumbnail(Handle, c.Src, out t) != 0) return false;
        c.Thumb = t; c.Active = true; c.ThumbDirty = true; c.ThumbApplied = false;
        if (c.Popup && c.Placed && Collides(c, c.X, c.Y - Label, c.CellW, c.CellH + Label)) c.Placed = false;   // its place was given to another meanwhile: it is placed again
        return true;
    }

    // The window stopped qualifying (hidden, cloaked, filtered) but still exists: keep its place for a grace period.
    void Sleep(Cell c, long now)
    {
        if (c.Thumb != IntPtr.Zero) { DwmUnregisterThumbnail(c.Thumb); c.Thumb = IntPtr.Zero; }
        c.Active = false; c.DormantAt = now; c.ThumbApplied = false;
    }

    void Release(Cell c)
    {
        if (c.Thumb != IntPtr.Zero) { DwmUnregisterThumbnail(c.Thumb); c.Thumb = IntPtr.Zero; }
        Uncure(c);
        cells.Remove(c.Src);
    }

    void ReleaseAll()
    {
        foreach (Cell c in new List<Cell>(cells.Values)) Release(c);
    }

    // Reads the window's real rectangles, minimized state and picture size.
    void ReadState(Cell c, long now)
    {
        IntPtr h = c.Src;
        c.Iconic = IsIconic(h);
        int cloak = 0; DwmGetWindowAttribute(h, DWMWA_CLOAKED, out cloak, 4);
        c.Cloaked = cloak != 0;
        if (c.Iconic) return;                           // GetWindowRect says -32000 and the thumbnail size means nothing: keep the last values
        RECT w;
        if (GetWindowRect(h, out w)) c.Win = w;
        RECT f;
        if (DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, out f, 16) == 0) c.Frame = f; else c.Frame = c.Win;
        int sw = 0, sh = 0;
        SIZE s;
        if (c.Thumb != IntPtr.Zero && DwmQueryThumbnailSourceSize(c.Thumb, out s) == 0) { sw = s.W; sh = s.H; }
        if (sw <= 0 || sh <= 0) { sw = c.Frame.R - c.Frame.L; sh = c.Frame.B - c.Frame.T; }
        if (sw > 0 && sh > 0 && (sw != c.SrcW || sh != c.SrcH)) OnSrcSize(c, sw, sh, now);
    }

    // The title bar as Windows reports it for this window: its height down from the top of the frame, and the width of the
    // caption buttons at its right end. A window that has no title bar, or reports none, gets 0. Read when the window is
    // first seen and when its size changes, which is when a title bar changes.
    void ReadCaption(Cell c)
    {
        int height = 0, buttons = 0;
        TITLEBARINFO info = new TITLEBARINFO();
        info.cbSize = Marshal.SizeOf(typeof(TITLEBARINFO));
        info.rgstate = new int[6];
        if ((c.Style & WS_CAPTION) == WS_CAPTION && GetTitleBarInfo(c.Src, ref info)
            && (info.rgstate[0] & (STATE_SYSTEM_INVISIBLE | STATE_SYSTEM_OFFSCREEN)) == 0)
        {
            height = Math.Max(0, info.rcTitleBar.B - c.Frame.T);
            RECT b;
            if (DwmGetWindowAttribute(c.Src, DWMWA_CAPTION_BUTTON_BOUNDS, out b, 16) == 0) buttons = Math.Max(0, b.R - b.L);
            // Store apps (Calculator, Settings) draw their title bar in a frame window that reports no button bounds. Without a width the
            // host's drag strip would cover the buttons, so the width is worked out from the style, as Windows' own buttons are 46 px
            // (at 100%) wide, and a little more is left for the border at the right.
            if (buttons == 0 && (c.Style & WS_SYSMENU) != 0)
            {
                int count = 1 + ((c.Style & (WS_MINIMIZEBOX | WS_MAXIMIZEBOX)) != 0 ? 2 : 0);
                buttons = (count * 46 + 8) * (int)GetDpiForWindow(c.Src) / 96;
            }
        }
        c.CaptionH = height; c.ButtonsW = buttons;
    }

    // ================================================================ Placement
    static bool Hit(int ax, int ay, int aw, int ah, int bx, int by, int bw, int bh)
    {
        return ax < bx + bw && bx < ax + aw && ay < by + bh && by < ay + ah;
    }

    int Label { get { return debugOn ? DebugLabelH : 0; } }

    // Does a box of the given size at (x, y) collide with another cell's box? Overflow cells take no space of their own.
    bool Collides(Cell self, int x, int y, int bw, int bh)
    {
        foreach (Cell o in cells.Values)
        {
            if (o == self || !o.Placed || o.Overflow || (o.Popup && !o.Active)) continue;   // a menu that closed keeps its place only so it can come back, not against other windows
            if (Hit(x, y, bw, bh, o.X, o.Y - Label, o.CellW, o.CellH + Label)) return true;
        }
        return false;
    }

    // The window's picture changed size. The cell keeps its place when the new size still fits there, otherwise
    // it keeps showing the part that fits and moves once after SettleMs (so a drag-resize moves it once, not per step).
    void OnSrcSize(Cell c, int sw, int sh, long now)
    {
        c.SrcW = sw; c.SrcH = sh;
        if (!c.Popup) ReadCaption(c);
        else if (c.InDone) { c.InDone = false; c.InTries = 0; c.HaveCand = false; c.InAt = now + InsetFirstMs; c.InL = c.InT = c.InR = c.InB = 0; }   // a new size has new margins
        int wantW = Math.Min(sw - c.InL - c.InR, ovW), wantH = Math.Min(sh - c.InT - c.InB, ovH);
        c.ThumbDirty = true;
        if (!c.Placed) { c.CellW = wantW; c.CellH = wantH; return; }
        if (wantW == c.CellW && wantH == c.CellH && c.MoveAt == 0) return;
        if (c.Overflow || !FitsInPlace(c, wantW, wantH))
        {
            c.CellW = Math.Min(c.CellW, wantW); c.CellH = Math.Min(c.CellH, wantH);
            c.WantW = wantW; c.WantH = wantH;
            c.MoveAt = now + SettleMs;
            if (c.MoveAt == 0) c.MoveAt = 1;
        }
        else { c.CellW = wantW; c.CellH = wantH; c.MoveAt = 0; }
    }

    bool FitsInPlace(Cell c, int w, int h)
    {
        if (c.X + w > packR || c.Y + h > packB) return false;
        return !Collides(c, c.X, c.Y - Label, w, h + Label);
    }

    // Moves cells whose resize settled. This is the only time a cell moves because of its own size.
    void Settle(long now)
    {
        foreach (Cell c in cells.Values)
        {
            if (c.MoveAt == 0 || now < c.MoveAt) continue;
            c.MoveAt = 0;
            c.ThumbDirty = true;
            if (!c.Overflow && FitsInPlace(c, c.WantW, c.WantH)) { c.CellW = c.WantW; c.CellH = c.WantH; continue; }
            c.CellW = c.WantW; c.CellH = c.WantH;
            c.Placed = false;                           // PlaceAll finds a new spot
        }
    }

    // First spot (top to bottom, left to right) where the cell's box fits among the cells that are already placed.
    // Candidates are the corners next to existing boxes, so cells pack against each other and the edge of the work area.
    bool TrySpot(Cell c)
    {
        int bw = c.CellW, bh = c.CellH + Label;
        List<int> xs = new List<int>(); List<int> ys = new List<int>();
        xs.Add(packL); ys.Add(packT);
        foreach (Cell o in cells.Values)
        {
            if (o == c || !o.Placed || o.Overflow || (o.Popup && !o.Active)) continue;
            xs.Add(o.X + o.CellW);
            ys.Add(o.Y + o.CellH);
        }
        xs.Sort(); ys.Sort();
        foreach (int y in ys)
        {
            if (y + bh > packB) break;
            foreach (int x in xs)
            {
                if (x + bw > packR) break;
                if (!Collides(c, x, y, bw, bh)) { c.X = x; c.Y = y + Label; c.Placed = true; c.Overflow = false; return true; }
            }
        }
        return false;
    }

    // Place a cell: a free spot; else an overflow cell at the corner of the work area that hides the cells it lies over.
    // A popup never overflows (it would hide real cells for a moment); one with no place gets no picture.
    void PlaceCell(Cell c)
    {
        if (TrySpot(c)) return;
        if (!c.Popup) { c.X = packL; c.Y = packT + Label; c.Placed = true; c.Overflow = true; }
        else { c.Placed = false; c.Overflow = false; }
    }

    void PlaceAll()
    {
        List<Cell> todo = new List<Cell>();
        foreach (Cell c in cells.Values) if (!c.Placed && c.Active) todo.Add(c);
        if (todo.Count == 0) return;
        todo.Sort(delegate(Cell a, Cell b) { return a.Order.CompareTo(b.Order); });
        foreach (Cell c in todo) { PlaceCell(c); c.ThumbDirty = true; }
    }

    // An overflow cell hides (not moves) every cell it lies over; of two overflow cells the one raised last wins.
    void UpdateHidden()
    {
        foreach (Cell a in cells.Values)
        {
            bool hid = false;
            if (a.Active && a.Placed)
            {
                foreach (Cell b in cells.Values)
                {
                    if (b == a || !b.Active || !b.Placed || !b.Overflow) continue;
                    if (a.Overflow && b.Stamp < a.Stamp) continue;
                    if (Hit(a.X, a.Y, a.CellW, a.CellH, b.X, b.Y, b.CellW, b.CellH)) { hid = true; break; }
                }
            }
            if (hid != a.Hidden) { a.Hidden = hid; a.ThumbDirty = true; }
        }
    }

    // Writes the thumbnail properties of cells that changed. Destination = source size = cell size, so nothing is scaled.
    void ApplyThumbs()
    {
        foreach (Cell c in cells.Values)
        {
            if (!c.Active || c.Thumb == IntPtr.Zero || !c.ThumbDirty) continue;
            if (c.Iconic && c.ThumbApplied) { c.ThumbDirty = false; continue; }   // keep what a minimized window last showed
            int w = Math.Min(c.CellW, c.SrcW - c.InL - c.InR), h = Math.Min(c.CellH, c.SrcH - c.InT - c.InB);
            DWM_THUMBNAIL_PROPERTIES p = new DWM_THUMBNAIL_PROPERTIES();
            p.dwFlags = DWM_TNP_RECTDESTINATION | DWM_TNP_RECTSOURCE | DWM_TNP_OPACITY | DWM_TNP_VISIBLE | DWM_TNP_SOURCECLIENTAREAONLY;
            p.rcDestination = new RECT { L = c.X, T = c.Y, R = c.X + w, B = c.Y + h };
            // The source rectangle is measured from the window rectangle's corner, which on Windows 10 and 11 lies in the
            // invisible resize border, about 7 px left of what DWM calls the frame. Start at the frame, or the picture
            // sits that much to the right with the border's emptiness on its left (seen on a GPU guest, 2026-11-24).
            int ox = Math.Max(0, c.Frame.L - c.Win.L) + c.InL, oy = Math.Max(0, c.Frame.T - c.Win.T) + c.InT;
            p.rcSource = new RECT { L = ox, T = oy, R = ox + w, B = oy + h };
            p.opacity = 255;
            p.fVisible = c.Placed && !c.Hidden;
            p.fSourceClientAreaOnly = false;
            if (DwmUpdateThumbnailProperties(c.Thumb, ref p) == 0) { c.ThumbDirty = false; c.ThumbApplied = true; }
        }
        if (debugOn) Invalidate();
    }

    // ================================================================ Popup margins
    // A menu of a WinUI program lives in a window that is larger than the menu: around it is room for its shadow, which the
    // window leaves clear and DWM's thumbnail shows as a dark band. Windows reports no rectangle for the menu alone, so the
    // picture the overlay shows of the popup is read from the screen, and the margin is where its edge is: the shadow is
    // dark and smooth, the menu begins at a step. A window whose outer pixels are not dark has no margin.
    bool insetsOn = true;                     // the lab switch insets=0 turns it off
    static int Lum(int px) { return ((px >> 16) & 255) * 3 / 10 + ((px >> 8) & 255) * 6 / 10 + (px & 255) / 10; }

    // The distance from one end of a line of pixels to where the menu starts, or -1 if the line says nothing.
    static int EdgeMargin(int[] line)
    {
        int first = Lum(line[0]);
        if (first > ShadowMaxLum) return 0;
        int limit = Math.Min(line.Length / 2, InsetMax + 1);
        for (int i = 1; i < limit; i++)
            if (Math.Abs(Lum(line[i]) - first) > StepLum) return i;
        return -1;
    }

    // Finds the margins of the popups whose picture is up. A picture that is not there yet (all black) is tried again shortly.
    void MeasureInsets(long now)
    {
        foreach (Cell c in cells.Values)
        {
            if (c.InDone || !c.Active || !c.Popup || !c.Placed || c.Thumb == IntPtr.Zero || c.ThumbDirty || now < c.InAt) continue;
            int w = c.SrcW, h = c.SrcH;
            int[] px = (w >= 8 && h >= 8 && w <= 4096 && h <= 4096) ? CaptureOverlay(vx + c.X, vy + c.Y, w, h) : null;
            bool blank = px == null;
            if (!blank)
            {
                blank = true;
                for (int i = 0; i < px.Length && blank; i += 37) if ((px[i] & 0xFFFFFF) != 0) blank = false;
            }
            if (blank && ++c.InTries < InsetTriesMax) { c.InAt = now + InsetRetryMs; continue; }
            int l = 0, t = 0, r = 0, b = 0;
            if (!blank)
            {
                int[] row = new int[w], col = new int[h];
                int mid = h / 2, cmid = w / 2;
                for (int x = 0; x < w; x++) row[x] = px[mid * w + x];
                int[] rrow = new int[w]; for (int x = 0; x < w; x++) rrow[x] = row[w - 1 - x];
                for (int y = 0; y < h; y++) col[y] = px[y * w + cmid];
                int[] rcol = new int[h]; for (int y = 0; y < h; y++) rcol[y] = col[h - 1 - y];
                int ml = EdgeMargin(row), mr = EdgeMargin(rrow), mt = EdgeMargin(col), mb = EdgeMargin(rcol);
                bool all = ml >= 0 && mr >= 0 && mt >= 0 && mb >= 0;
                // A WinUI menu opens with an animation, and its picture is not the final one until that is over. Two readings
                // of the same margins, a beat apart, are taken as the real ones; a menu that never settles is shown whole.
                bool same = all && c.HaveCand && c.CandL == ml && c.CandT == mt && c.CandR == mr && c.CandB == mb;
                if (!same && ++c.InTries < InsetTriesMax)
                {
                    if (all) { c.HaveCand = true; c.CandL = ml; c.CandT = mt; c.CandR = mr; c.CandB = mb; } else c.HaveCand = false;
                    c.InAt = now + InsetRetryMs;
                    continue;
                }
                if (same) { l = ml; r = mr; t = mt; b = mb; }
            }
            if (l + r >= w - MinCellSide || t + b >= h - MinCellSide) { l = t = r = b = 0; }
            c.InL = l; c.InT = t; c.InR = r; c.InB = b;
            c.CellW = Math.Min(w - l - r, ovW); c.CellH = Math.Min(h - t - b, ovH);
            c.InDone = true; c.ThumbDirty = true;
            ApplyThumbs();
        }
    }

    static int[] CaptureOverlay(int x, int y, int w, int h)
    {
        IntPtr screen = GetDC(IntPtr.Zero);
        if (screen == IntPtr.Zero) return null;
        IntPtr dc = CreateCompatibleDC(screen), bmp = CreateCompatibleBitmap(screen, w, h);
        int[] bits = null;
        if (dc != IntPtr.Zero && bmp != IntPtr.Zero)
        {
            IntPtr old = SelectObject(dc, bmp);
            bool copied = BitBlt(dc, 0, 0, w, h, screen, x, y, SRCCOPY | CAPTUREBLT);
            SelectObject(dc, old);                      // GetDIBits wants a bitmap that no device context holds
            if (copied)
            {
                BITMAPINFOHEADER info = new BITMAPINFOHEADER();
                info.biSize = Marshal.SizeOf(typeof(BITMAPINFOHEADER)); info.biWidth = w; info.biHeight = -h; info.biPlanes = 1; info.biBitCount = 32;
                bits = new int[w * h];
                if (GetDIBits(dc, bmp, 0, (uint)h, bits, ref info, 0) == 0) bits = null;
            }
        }
        if (bmp != IntPtr.Zero) DeleteObject(bmp);
        if (dc != IntPtr.Zero) DeleteDC(dc);
        ReleaseDC(IntPtr.Zero, screen);
        return bits;
    }

    // ================================================================ Parking
    static bool InMoveSize(IntPtr h)
    {
        uint pid;
        uint thread = GetWindowThreadProcessId(h, out pid);
        GUITHREADINFO info = new GUITHREADINFO();
        info.cbSize = Marshal.SizeOf(typeof(GUITHREADINFO));
        return GetGUIThreadInfo(thread, ref info) && (info.flags & GUI_INMOVESIZE) != 0;
    }

    bool CoversScreen(Cell c)
    {
        return (c.Frame.R - c.Frame.L) >= GetSystemMetrics(SM_CXSCREEN) && (c.Frame.B - c.Frame.T) >= GetSystemMetrics(SM_CYSCREEN);
    }

    // Moves every app window that has a cell so that its frame sits at its cell, and puts back one that has left. Left alone: popups, which
    // cannot be moved; minimized and maximized windows and ones as big as the screen, which Windows places itself; a window the user is
    // moving or sizing right now (it is put back when the user lets go); a window that is stuck; and a window that keeps leaving.
    void ParkAll(long now)
    {
        if (desktopKind != "normal") return;
        foreach (Cell c in cells.Values) Park(c, now);
    }

    void Park(Cell c, long now)
    {
        if (!c.Active || c.Popup || !c.Placed || c.Iconic || c.Cloaked) return;
        if (now < c.ParkAfter) return;
        IntPtr h = c.Src;
        if (IsZoomed(h) || CoversScreen(c) || IsHungAppWindow(h) || InMoveSize(h)) return;
        ReadState(c, now);                              // the pass may have run before the window's last move was seen
        int wantL = vx + c.X, wantT = vy + c.Y;
        if (c.Frame.L == wantL && c.Frame.T == wantT) { c.ParkTries = 0; return; }
        if (++c.ParkTries > ParkTriesMax) { c.ParkTries = 0; c.ParkAfter = now + ParkBackoffMs; return; }
        // The window rectangle reaches past the frame into the invisible border, so the corner to aim at is further out.
        int x = wantL - (c.Frame.L - c.Win.L), y = wantT - (c.Frame.T - c.Win.T);
        SetWindowPos(h, IntPtr.Zero, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
        c.ParkAfter = now + ParkSettleMs;
    }

    // ================================================================ Layered cure
    // Chromium treats a window as covered, and stops painting, when a window above it fully covers it, unless that
    // window is layered. So each app window is given WS_EX_LAYERED with alpha 254 once. Only styles set here are undone.
    void Cure(Cell c)
    {
        if (!cureOn || c.Popup || c.Cured || c.CureTries >= 3) return;
        IntPtr h = c.Src;
        if (IsHungAppWindow(h)) return;                 // SetWindowLongPtr would wait for a stuck window
        long ex = GetWindowLongPtr(h, GWL_EXSTYLE).ToInt64();
        if ((ex & WS_EX_LAYERED) != 0) return;          // the app's own layered window: leave it alone
        bool noCaption = (c.Style & WS_CAPTION) != WS_CAPTION;
        if (noCaption && CoversScreen(c)) return;       // borderless fullscreen (a game): layering could cost it direct presentation
        SetWindowLongPtr(h, GWL_EXSTYLE, new IntPtr(ex | WS_EX_LAYERED));
        SetLayeredWindowAttributes(h, 0, (byte)AlphaForCure, LWA_ALPHA);
        if ((GetWindowLongPtr(h, GWL_EXSTYLE).ToInt64() & WS_EX_LAYERED) != 0) { c.Cured = true; c.CureTries++; }
    }

    // Called on each scan: sets the cure on windows that qualified late, and again where an app cleared it (at most 3 times).
    void KeepCured(Cell c)
    {
        if (c.Cured)
        {
            long ex = GetWindowLongPtr(c.Src, GWL_EXSTYLE).ToInt64();
            if ((ex & WS_EX_LAYERED) != 0) return;
            c.Cured = false;
        }
        Cure(c);
    }

    void Uncure(Cell c)
    {
        if (!c.Cured) return;
        c.Cured = false;
        IntPtr h = c.Src;
        if (!IsWindow(h) || IsHungAppWindow(h)) return;
        long ex = GetWindowLongPtr(h, GWL_EXSTYLE).ToInt64();
        if ((ex & WS_EX_LAYERED) != 0) SetWindowLongPtr(h, GWL_EXSTYLE, new IntPtr(ex & ~WS_EX_LAYERED));
    }

    // ================================================================ Work: one coalesced pass
    void Work()
    {
        if (shutDown) return;
        long now = Now();
        if (scanDue) { scanDue = false; Scan(now); }
        int next = FastPass(now);
        Settle(now);
        PlaceAll();
        UpdateHidden();
        ApplyThumbs();
        MeasureInsets(now);
        ComputeParents();
        ParkAll(now);
        LayoutStep();
        foreach (Cell c in cells.Values)
        {
            if (c.MoveAt != 0) { long r = c.MoveAt - now; if (r < 1) r = 1; if (r < next) next = (int)r; }
            if (c.ParkAfter > now) { long r = c.ParkAfter - now; if (r < next) next = (int)r; }
            if (!c.InDone && c.Active && c.Popup && c.Placed && c.Thumb != IntPtr.Zero) { long r = c.InAt - now; if (r < 10) r = 10; if (r < next) next = (int)r; }   // never a timer of 1 ms for a popup that has no place
        }
        if (next != int.MaxValue) ArmWork(next);
    }

    // Handles cells flagged by location and name events without a full scan. Returns ms until a deferred title is due.
    int FastPass(long now)
    {
        int next = int.MaxValue;
        IntPtr fg = GetForegroundWindow();
        foreach (Cell c in cells.Values)
        {
            if (!c.Active) continue;
            c.Fg = (c.Src == fg);
            if (c.LocDirty) { c.LocDirty = false; ReadState(c, now); }
            if (c.NameDirty)
            {
                long since = now - c.TitleAt;
                if (since >= TitleMs) { c.Title = TextOf(c.Src); c.TitleAt = now; c.NameDirty = false; }
                else { long r = TitleMs - since; if (r < next) next = (int)r; }   // title changes at most once per TitleMs
            }
        }
        return next;
    }

    // ================================================================ Popup parents
    // Owner chain first (the root owner). Failing that, the app window the popup lies over the most,
    // windows of the same process before others.
    void ComputeParents()
    {
        foreach (Cell c in cells.Values)
        {
            if (!c.Active) continue;
            c.Parent = IntPtr.Zero; c.HasRel = false;
            if (!c.Popup) continue;
            IntPtr root = IntPtr.Zero, o = c.Owner;
            for (int depth = 0; o != IntPtr.Zero && depth < 8; depth++) { root = o; o = GetWindow(o, GW_OWNER); }
            if (root != IntPtr.Zero) c.Parent = root;
            else
            {
                long best = 0; IntPtr bestH = IntPtr.Zero;
                foreach (Cell b in cells.Values)
                {
                    if (b == c || !b.Active || b.Popup || b.Iconic) continue;
                    long ix = Math.Min(c.Frame.R, b.Frame.R) - Math.Max(c.Frame.L, b.Frame.L);
                    long iy = Math.Min(c.Frame.B, b.Frame.B) - Math.Max(c.Frame.T, b.Frame.T);
                    if (ix <= 0 || iy <= 0) continue;
                    long score = ix * iy + (b.Pid == c.Pid ? (1L << 40) : 0);
                    if (score > best || (score == best && b.Z < ZOf(bestH))) { best = score; bestH = b.Src; }
                }
                if (bestH != IntPtr.Zero) c.Parent = bestH;
            }
            if (c.Parent != IntPtr.Zero)
            {
                RECT pf;
                Cell pc;
                if (cells.TryGetValue(c.Parent, out pc) && !pc.Iconic) pf = pc.Frame;
                else if (DwmGetWindowAttribute(c.Parent, DWMWA_EXTENDED_FRAME_BOUNDS, out pf, 16) != 0 && !GetWindowRect(c.Parent, out pf)) continue;
                c.HasRel = true; c.RelX = c.Frame.L + c.InL - pf.L; c.RelY = c.Frame.T + c.InT - pf.T;
            }
        }
    }

    int ZOf(IntPtr h) { Cell c; return cells.TryGetValue(h, out c) ? c.Z : int.MaxValue; }

    // ================================================================ Messages to the host
    static string I(int v) { return v.ToString(CultureInfo.InvariantCulture); }
    static string L(long v) { return v.ToString(CultureInfo.InvariantCulture); }
    static string B(bool v) { return v ? "true" : "false"; }

    static void Js(StringBuilder sb, string s)
    {
        sb.Append('"');
        foreach (char ch in s)
        {
            switch (ch)
            {
                case '"': sb.Append("\\\""); break;
                case '\\': sb.Append("\\\\"); break;
                case '\n': sb.Append("\\n"); break;
                case '\r': sb.Append("\\r"); break;
                case '\t': sb.Append("\\t"); break;
                default:
                    if (ch < 0x20) sb.Append("\\u").Append(((int)ch).ToString("x4", CultureInfo.InvariantCulture));
                    else sb.Append(ch);
                    break;
            }
        }
        sb.Append('"');
    }

    static void Rc(StringBuilder sb, string name, int x, int y, int w, int h)
    {
        sb.Append(",\"").Append(name).Append("\":{\"x\":").Append(I(x)).Append(",\"y\":").Append(I(y))
          .Append(",\"w\":").Append(I(w)).Append(",\"h\":").Append(I(h)).Append('}');
    }

    static string StateOf(Cell c) { return c.Iconic ? "minimized" : (c.Hidden ? "covered" : "shown"); }

    void AppendCell(StringBuilder sb, Cell c)
    {
        sb.Append("{\"id\":\"").Append(L(c.Src.ToInt64())).Append('"');
        sb.Append(",\"pid\":").Append(L(c.Pid));
        sb.Append(",\"exe\":"); Js(sb, c.Exe);
        sb.Append(",\"app\":"); Js(sb, c.App);
        sb.Append(",\"name\":"); Js(sb, c.Name);
        sb.Append(",\"cls\":"); Js(sb, c.Cls);
        sb.Append(",\"title\":"); Js(sb, c.Title);
        // cell: screen coordinates of the picture's top-left; w,h = size shown, 0 when not placed
        int w = c.Placed ? Math.Min(c.CellW, c.SrcW - c.InL - c.InR) : 0, h = c.Placed ? Math.Min(c.CellH, c.SrcH - c.InT - c.InB) : 0;
        Rc(sb, "cell", c.Placed ? vx + c.X : 0, c.Placed ? vy + c.Y : 0, w, h);
        Rc(sb, "frame", c.Frame.L + c.InL, c.Frame.T + c.InT, c.Frame.R - c.Frame.L - c.InL - c.InR, c.Frame.B - c.Frame.T - c.InT - c.InB);   // where the window really is: where input goes
        sb.Append(",\"popup\":").Append(B(c.Popup));
        sb.Append(",\"parent\":\"").Append(c.Parent == IntPtr.Zero ? "" : L(c.Parent.ToInt64())).Append('"');
        sb.Append(",\"rel\":{\"x\":").Append(I(c.HasRel ? c.RelX : 0)).Append(",\"y\":").Append(I(c.HasRel ? c.RelY : 0)).Append('}');
        sb.Append(",\"state\":\"").Append(StateOf(c)).Append('"');
        sb.Append(",\"overflow\":").Append(B(c.Overflow));
        sb.Append(",\"focus\":").Append(B(c.Fg));
        sb.Append(",\"caption\":{\"h\":").Append(I(c.CaptionH)).Append(",\"buttons_w\":").Append(I(c.ButtonsW)).Append('}');
        sb.Append(",\"resizable\":").Append(B(!c.Popup && (c.Style & WS_THICKFRAME) != 0)).Append('}');
    }

    string ScreenJson() { return "{\"w\":" + I(ovW) + ",\"h\":" + I(ovH) + "}"; }

    string HelloLine()
    {
        int scale = PercentNow();
        return "{\"t\":\"hello\",\"v\":" + I(ProtocolVersion) + ",\"session\":\"" + session + "\",\"screen\":" + ScreenJson() + ",\"scale\":" + I(scale) + "}";
    }

    // Builds the layout from the current cells and, when something in it changed, sends the whole of it again.
    void LayoutStep()
    {
        List<Cell> list = new List<Cell>();
        foreach (Cell c in cells.Values) if (c.Active && c.InDone && c.IdSettled) list.Add(c);   // a popup waits until its margins are known
        list.Sort(delegate(Cell a, Cell b) { return a.Order.CompareTo(b.Order); });
        StringBuilder core = new StringBuilder();
        core.Append("\"screen\":").Append(ScreenJson()).Append(",\"cells\":[");
        for (int i = 0; i < list.Count; i++)
        {
            if (i > 0) core.Append(',');
            AppendCell(core, list[i]);
        }
        core.Append(']');
        string cs = core.ToString();
        if (cs != lastCore || seq == 0)
        {
            lastCore = cs;
            seq++;
            lastLayout = "{\"t\":\"layout\",\"seq\":" + I(seq) + "," + cs + "}";
            Link link = Current();
            if (link != null) link.SendLayout(lastLayout);
            if (feedToFile) feedPending = true;
        }
        if (feedPending) WriteFeed();
        DisplayStep(false);
    }

    Link Current() { lock (linkGate) { return current; } }

    // Temp file, then rename over the feed, so a reader sees the old file or the new one, never half of one.
    void WriteFeed()
    {
        try
        {
            File.WriteAllBytes(FeedTmp, new UTF8Encoding(false).GetBytes(lastLayout + "\n"));
            if (MoveFileEx(FeedTmp, FeedFile, MOVEFILE_REPLACE_EXISTING)) feedPending = false;
        }
        catch { }
    }

    // ================================================================ The input desktop
    // While the secure desktop (UAC) or the lock screen has the input, no window of this session can be seen or moved.
    void CheckDesktop()
    {
        string kind = "normal";
        IntPtr d = OpenInputDesktop(0, false, DESKTOP_READOBJECTS);
        if (d == IntPtr.Zero) kind = sessionLocked ? "locked" : "secure";
        else
        {
            StringBuilder name = new StringBuilder(256);
            int needed;
            if (GetUserObjectInformation(d, UOI_NAME, name, name.Capacity * 2, out needed) && !string.Equals(name.ToString(), "Default", StringComparison.OrdinalIgnoreCase))
                kind = sessionLocked ? "locked" : "secure";
            CloseDesktop(d);
        }
        if (kind == desktopKind) return;
        desktopKind = kind;
        Link link = Current();
        if (link != null) link.Send(DesktopLine());
        if (kind == "normal") { scanDue = true; Kick(); }
    }

    string DesktopLine() { return "{\"t\":\"desktop\",\"kind\":\"" + desktopKind + "\"}"; }

    // ================================================================ The host's connection
    void Listen()
    {
        TcpListener listener = new TcpListener(IPAddress.Any, Port);
        // With the port taken or the listener gone there is nothing to serve, and the helper that is not listening ends so it can be started again.
        try { listener.Start(); } catch (Exception ex) { LogError(ex); Environment.Exit(1); }
        while (true)
        {
            TcpClient client;
            try { client = listener.AcceptTcpClient(); } catch (Exception ex) { LogError(ex); Environment.Exit(1); return; }
            Thread serving = new Thread(delegate() { Serve(client); });
            serving.IsBackground = true;
            serving.Start();
        }
    }

    // One line, up to a limit, without waiting for more than the stream's read timeout. Null when the other end closed or the line is too long.
    static string ReadLine(Stream stream, int limit)
    {
        MemoryStream line = new MemoryStream();
        byte[] one = new byte[1];
        while (true)
        {
            if (stream.Read(one, 0, 1) != 1) return null;
            if (one[0] == (byte)'\n') break;
            if (line.Length >= limit) return null;
            line.WriteByte(one[0]);
        }
        return Encoding.UTF8.GetString(line.ToArray()).TrimEnd('\r');
    }

    void Serve(TcpClient client)
    {
        Link link = null;
        bool counted = false;
        try
        {
            IPAddress from = ((IPEndPoint)client.Client.RemoteEndPoint).Address;
            if (!IPAddress.IsLoopback(from) && from.ToString() != HostAddress) return;
            client.NoDelay = true;
            NetworkStream stream = client.GetStream();
            stream.ReadTimeout = TokenWaitMs;
            stream.WriteTimeout = WriteTimeoutMs;
            string first = ReadLine(stream, TokenMaxBytes);
            if (first == null) return;
            // "TOKEN side": a command's connection, which does not take the viewer's.
            bool side = false;
            int space = first.IndexOf(' ');
            if (space > 0 && first.Substring(space + 1) == "side") { side = true; first = first.Substring(0, space); }
            if (first != token) return;
            stream.ReadTimeout = Timeout.Infinite;
            link = new Link(client, stream);
            link.Side = side;
            if (side)
            {
                lock (linkGate) { if (sideLinks >= SideLinksMax) return; sideLinks++; counted = true; }
            }
            else
            {
                Link old;
                lock (linkGate) { old = current; current = link; }
                if (old != null) old.Close();
            }
            Thread writing = new Thread(link.WriteLoop);
            writing.IsBackground = true;
            writing.Start();
            Link mine = link;
            BeginInvoke(new MethodInvoker(delegate { Greet(mine); }));
            string line;
            while ((line = ReadLine(stream, LineMaxBytes)) != null)
            {
                string text = line;
                BeginInvoke(new MethodInvoker(delegate { OnRequest(mine, text); }));
            }
        }
        catch (Exception ex)
        {
            if (!(ex is IOException) && !(ex is SocketException) && !(ex is ObjectDisposedException) && !(ex is InvalidOperationException)) LogError(ex);
        }
        finally
        {
            if (link != null)
            {
                lock (linkGate) { if (current == link) current = null; if (counted) sideLinks--; }
                link.Close();
            }
            try { client.Close(); } catch { }
        }
    }

    // On the UI thread, so that what the host is told first is what the helper is in.
    void Greet(Link link)
    {
        link.Send(HelloLine());
        if (seq == 0) LayoutStep();
        link.SendLayout(lastLayout);
        if (desktopKind != "normal") link.Send(DesktopLine());
        lastDisplay = DisplayLine();
        link.Send(lastDisplay);
    }

    // ================================================================ Windows' scale and the screen the windows need
    // Windows has one scale for its one monitor. The host asks for a step ("scale"), and the helper sets it live with the undocumented
    // DisplayConfigSetDeviceInfo type -4 (what Settings uses; no rights needed), which takes the step relative to the step Windows
    // recommends for the monitor. The recommended step changes with the resolution, so the helper works in percentages and sets the
    // percentage again after a change of mode. Only Windows' steps can be set live: a custom percentage takes a sign-out.
    [StructLayout(LayoutKind.Sequential)] struct LUID { public uint Low; public int High; }
    [StructLayout(LayoutKind.Sequential)] struct DcHeader { public int Type; public int Size; public LUID Adapter; public uint Id; }
    [StructLayout(LayoutKind.Sequential)] struct DcDpiGet { public DcHeader H; public int Min, Cur, Max; }
    [StructLayout(LayoutKind.Sequential)] struct DcDpiSet { public DcHeader H; public int Rel; }
    [StructLayout(LayoutKind.Sequential)] struct DcPathSource { public LUID Adapter; public uint Id, ModeIdx, Flags; }
    [StructLayout(LayoutKind.Sequential)] struct DcPathTarget { public LUID Adapter; public uint Id, ModeIdx; public int Tech, Rot, Scaling; public uint RateN, RateD; public int Order, Avail; public uint Flags; }
    [StructLayout(LayoutKind.Sequential)] struct DcPath { public DcPathSource S; public DcPathTarget T; public uint Flags; }
    [StructLayout(LayoutKind.Sequential)] struct DcMode { public int Type; public uint Id; public LUID Adapter; [MarshalAs(UnmanagedType.ByValArray, SizeConst = 48)] public byte[] Data; }
    [DllImport("user32.dll")] static extern int GetDisplayConfigBufferSizes(uint flags, out uint paths, out uint modes);
    [DllImport("user32.dll")] static extern int QueryDisplayConfig(uint flags, ref uint np, [Out] DcPath[] p, ref uint nm, [Out] DcMode[] m, IntPtr topology);
    [DllImport("user32.dll")] static extern int DisplayConfigGetDeviceInfo(ref DcDpiGet g);
    [DllImport("user32.dll")] static extern int DisplayConfigSetDeviceInfo(ref DcDpiSet s);
    [DllImport("user32.dll")] static extern uint GetDpiForWindow(IntPtr h);
    [DllImport("user32.dll")] static extern IntPtr MonitorFromWindow(IntPtr h, uint flags);
    [DllImport("shcore.dll")] static extern int GetDpiForMonitor(IntPtr monitor, int type, out uint x, out uint y);
    const int QDC_ONLY_ACTIVE_PATHS = 2, DC_GET_DPI_SCALE = -3, DC_SET_DPI_SCALE = -4, WM_DPICHANGED = 0x02E0;
    static readonly int[] DpiSteps = { 100, 125, 150, 175, 200, 225, 250, 300, 350, 400, 450, 500 };

    int wantedPercent;                          // the step the host asked for last, set again after a change of mode
    int modeW, modeH;                           // the screen's size when the scale was last looked after
    int floorW, floorH, ceilW, ceilH;           // from the host's "screen": the smallest and largest screen it wants
    string lastDisplay = "";

    static bool FirstPath(out DcPath path)
    {
        path = new DcPath();
        uint np, nm;
        if (GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, out np, out nm) != 0 || np == 0) return false;
        DcPath[] paths = new DcPath[np]; DcMode[] modes = new DcMode[nm];
        if (QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, ref np, paths, ref nm, modes, IntPtr.Zero) != 0 || np == 0) return false;
        path = paths[0];
        return true;
    }

    // Windows' percentage now, from the monitor (the helper is per-monitor aware). GetDpiForSystem keeps the one of sign-in, and the
    // overlay's own DPI follows a change only once its WM_DPICHANGED has come.
    int PercentNow()
    {
        uint x, y;
        if (GetDpiForMonitor(MonitorFromWindow(Handle, 1), 0, out x, out y) != 0) x = GetDpiForWindow(Handle);
        return (int)Math.Round(x * 100.0 / 96.0);
    }

    // The steps Windows offers now, from the smallest; empty when DisplayConfig does not say.
    List<int> OfferedSteps(out int recommendedIndex)
    {
        List<int> offered = new List<int>();
        recommendedIndex = -1;
        DcPath path;
        if (!FirstPath(out path)) return offered;
        DcDpiGet g = new DcDpiGet();
        g.H.Type = DC_GET_DPI_SCALE; g.H.Size = Marshal.SizeOf(typeof(DcDpiGet)); g.H.Adapter = path.S.Adapter; g.H.Id = path.S.Id;
        if (DisplayConfigGetDeviceInfo(ref g) != 0) return offered;
        int current = Array.IndexOf(DpiSteps, PercentNow());
        if (current < 0) return offered;        // a custom percentage: no step to count from
        recommendedIndex = current - g.Cur;
        for (int rel = g.Min; rel <= g.Max; rel++)
        {
            int i = recommendedIndex + rel;
            if (i >= 0 && i < DpiSteps.Length) offered.Add(DpiSteps[i]);
        }
        return offered;
    }

    // Sets Windows' scale to a step it offers. Null when it took it, else why not.
    string SetScale(int percent)
    {
        int recommended;
        List<int> offered = OfferedSteps(out recommended);
        if (!offered.Contains(percent)) return "Windows does not offer " + percent + " % at this resolution (it offers " + string.Join(",", offered) + ")";
        if (PercentNow() == percent) return null;
        DcPath path;
        if (!FirstPath(out path)) return "no display path";
        DcDpiSet set = new DcDpiSet();
        set.H.Type = DC_SET_DPI_SCALE; set.H.Size = Marshal.SizeOf(typeof(DcDpiSet)); set.H.Adapter = path.S.Adapter; set.H.Id = path.S.Id;
        set.Rel = Array.IndexOf(DpiSteps, percent) - recommended;
        int result = DisplayConfigSetDeviceInfo(ref set);
        return result == 0 ? null : "Windows refused the scale, error " + result;
    }

    // Whether boxes of these sizes, in this order, all find a spot in a pack area of w x h, by the same rule as TrySpot: the first
    // spot, top to bottom and left to right, among the corners next to boxes already placed.
    static bool PacksInto(List<int[]> sizes, int w, int h)
    {
        List<int[]> placed = new List<int[]>();
        foreach (int[] size in sizes)
        {
            List<int> xs = new List<int>(); List<int> ys = new List<int>();
            xs.Add(0); ys.Add(0);
            foreach (int[] b in placed) { xs.Add(b[0] + b[2]); ys.Add(b[1] + b[3]); }
            xs.Sort(); ys.Sort();
            bool found = false;
            foreach (int y in ys)
            {
                if (y + size[1] > h) break;
                foreach (int x in xs)
                {
                    if (x + size[0] > w) break;
                    bool hit = false;
                    foreach (int[] b in placed) if (Hit(x, y, size[0], size[1], b[0], b[1], b[2], b[3])) { hit = true; break; }
                    if (!hit) { placed.Add(new int[] { x, y, size[0], size[1] }); found = true; break; }
                }
                if (found) break;
            }
            if (!found) return false;
        }
        return true;
    }

    // The smallest screen, within the host's floor and ceiling, that holds every window at its own size with none overflowing,
    // packed as the helper packs them after a change of mode (every cell placed again, in order). The height is the floor's, or
    // the tallest window's; the width grows from the floor in steps of 16 until all fit. Null until the host has said its floor.
    string NeedJson()
    {
        if (floorW <= 0 || floorH <= 0) return null;
        int taskbarH = Math.Max(0, ovH - (packB - packT)), taskbarW = Math.Max(0, ovW - (packR - packL));
        List<Cell> order = new List<Cell>();
        foreach (Cell c in cells.Values)
        {
            if (!c.Active || c.Popup || c.Iconic) continue;
            // A maximized or screen-sized window takes the screen it is given, and the floor is a monitor's worth:
            // counting it would make the screen grow to the ceiling and stay there.
            if (IsZoomed(c.Src) || CoversScreen(c)) continue;
            if (c.SrcW - c.InL - c.InR <= 0 || c.SrcH - c.InT - c.InB <= 0) continue;
            order.Add(c);
        }
        order.Sort(delegate(Cell a, Cell b) { return a.Order.CompareTo(b.Order); });
        List<int[]> sizes = new List<int[]>();
        int tallest = 0, widest = 0;
        foreach (Cell c in order)
        {
            int w = c.SrcW - c.InL - c.InR, h = c.SrcH - c.InT - c.InB + Label;
            sizes.Add(new int[] { w, h });
            tallest = Math.Max(tallest, h); widest = Math.Max(widest, w);
        }
        int maxW = ceilW > 0 ? ceilW : 7680, maxH = ceilH > 0 ? ceilH : 4320;
        int needH = Math.Min(Math.Max(floorH, tallest + taskbarH), maxH);
        int needW = Math.Min(Math.Max(floorW, widest + taskbarW), maxW);
        while (needW < maxW && !PacksInto(sizes, needW - taskbarW, needH - taskbarH)) needW = Math.Min(maxW, needW + 16);
        return "{\"w\":" + I(needW & ~1) + ",\"h\":" + I(needH & ~1) + "}";
    }

    string DisplayLine()
    {
        int recommended;
        List<int> offered = OfferedSteps(out recommended);
        StringBuilder sb = new StringBuilder("{\"t\":\"display\",\"scale\":" + I(PercentNow()) + ",\"steps\":[");
        for (int i = 0; i < offered.Count; i++) { if (i > 0) sb.Append(','); sb.Append(I(offered[i])); }
        sb.Append(']');
        string need = NeedJson();
        if (need != null) sb.Append(",\"need\":").Append(need);
        return sb.Append('}').ToString();
    }

    // Sends the display line when anything in it changed, after each layout.
    void DisplayStep(bool always)
    {
        string line = DisplayLine();
        if (line == lastDisplay && !always) return;
        lastDisplay = line;
        Link link = Current();
        if (link != null) link.Send(line);
    }

    // A change of mode can change the step Windows recommends, and with it the percentage a relative step means.
    void KeepScale()
    {
        // A change of scale sends WM_DISPLAYCHANGE too: only a new size is a change of mode.
        if (ovW == modeW && ovH == modeH) return;
        modeW = ovW; modeH = ovH;
        if (wantedPercent <= 0 || PercentNow() == wantedPercent) return;
        string why = SetScale(wantedPercent);
        if (why != null) LogError(new Exception("scale " + wantedPercent + " after a change of mode: " + why));
    }

    void OnScaleRequest(Link link, long n, Dictionary<string, object> r)
    {
        int percent = (int)Num(r, "percent", 0);
        string why = SetScale(percent);
        if (why == null) { wantedPercent = percent; modeW = ovW; modeH = ovH; }
        Answer(link, n, why == null, why);
        scanDue = true; Kick();
        DisplayStep(true);
    }

    void OnScreenRequest(Link link, long n, Dictionary<string, object> r)
    {
        Dictionary<string, object> floor = r.ContainsKey("floor") ? r["floor"] as Dictionary<string, object> : null;
        Dictionary<string, object> ceiling = r.ContainsKey("ceiling") ? r["ceiling"] as Dictionary<string, object> : null;
        if (floor == null) { Answer(link, n, false, "screen needs a floor"); return; }
        modeW = ovW; modeH = ovH;
        floorW = (int)Num(floor, "w", 0); floorH = (int)Num(floor, "h", 0);
        ceilW = ceiling != null ? (int)Num(ceiling, "w", 0) : 0; ceilH = ceiling != null ? (int)Num(ceiling, "h", 0) : 0;
        Answer(link, n, true, null);
        DisplayStep(true);
    }

    // ================================================================ Requests
    static string Str(Dictionary<string, object> r, string key)
    {
        object v;
        return r.TryGetValue(key, out v) && v != null ? Convert.ToString(v, CultureInfo.InvariantCulture) : "";
    }

    static long Num(Dictionary<string, object> r, string key, long fallback)
    {
        object v;
        if (!r.TryGetValue(key, out v) || v == null) return fallback;
        try { return Convert.ToInt64(v, CultureInfo.InvariantCulture); } catch { return fallback; }
    }

    static void Answer(Link link, long n, bool ok, string error)
    {
        StringBuilder sb = new StringBuilder("{\"t\":\"answer\",\"re\":" + L(n) + ",\"ok\":" + B(ok));
        if (!ok) { sb.Append(",\"error\":"); Js(sb, error); }
        link.Send(sb.Append('}').ToString());
    }

    Cell CellOf(string id)
    {
        long handle;
        Cell c;
        if (long.TryParse(id, NumberStyles.None, CultureInfo.InvariantCulture, out handle) && cells.TryGetValue(new IntPtr(handle), out c) && c.Active) return c;
        return null;
    }

    void OnRequest(Link link, string line)
    {
        Dictionary<string, object> r;
        try { r = json.DeserializeObject(line) as Dictionary<string, object>; } catch { return; }
        if (r == null) return;
        long n = Num(r, "n", -1);
        try
        {
            string kind = Str(r, "t");
            // A side connection looks and starts programs. What changes a window belongs to the driving connection.
            if (link.Side && (kind == "focus" || kind == "minimize" || kind == "restore" || kind == "resize" || kind == "close" || kind == "scale" || kind == "screen"))
            {
                Answer(link, n, false, "a side connection cannot " + kind + " a window");
                return;
            }
            switch (kind)
            {
                case "layout": link.SendLayout(lastLayout); Answer(link, n, true, null); break;
                case "focus": Wait(link, n, "focus", r); break;
                case "minimize": Wait(link, n, "minimize", r); break;
                case "restore": Wait(link, n, "restore", r); break;
                case "resize": Wait(link, n, "resize", r); break;
                case "close": CloseWindow(link, n, r); break;
                case "launch": Launch(link, n, r); break;
                case "apps": Apps(link, n, Str(r, "icons") != "False"); break;
                case "icon": IconOf(link, n, r); break;
                case "scale": OnScaleRequest(link, n, r); break;
                case "screen": OnScreenRequest(link, n, r); break;
                default: Answer(link, n, false, "this helper does not know the request " + Str(r, "t")); break;
            }
        }
        catch (Exception ex) { Answer(link, n, false, ex.Message); }
    }

    // Starts a request that Windows takes its time over, and answers it from what the window is found to be.
    void Wait(Link link, long n, string kind, Dictionary<string, object> r)
    {
        Cell c = CellOf(Str(r, "id"));
        if (c == null || !IsWindow(c.Src)) { Answer(link, n, false, "no window has that id"); return; }
        Pending p = new Pending();
        p.Link = link; p.N = n; p.Kind = kind; p.Win = c.Src;
        long now = Now();
        switch (kind)
        {
            case "minimize": p.Deadline = now + MinimizeWaitMs; break;
            case "restore": p.Deadline = now + RestoreWaitMs; break;
            case "focus":
                if (c.Popup) { Answer(link, n, false, "a popup takes no focus"); return; }
                p.Deadline = now + FocusWaitMs;
                // The host window got the focus, so an overflow cell lying under others comes to the top.
                c.Stamp = ++orderCounter; c.ThumbDirty = true; Kick();
                break;
            case "resize":
                p.W = (int)Num(r, "w", 0); p.H = (int)Num(r, "h", 0);
                if (c.Popup || (c.Style & WS_THICKFRAME) == 0) { Answer(link, n, false, "this window cannot be resized"); return; }
                if (c.Iconic || IsZoomed(c.Src)) { Answer(link, n, false, "a minimized or maximized window cannot be resized"); return; }
                if (p.W < MinCellSide || p.H < MinCellSide) { Answer(link, n, false, "the size is too small"); return; }
                p.W = Math.Min(p.W, ovW); p.H = Math.Min(p.H, ovH);
                p.Deadline = now + ResizeWaitMs;
                // Windows' size includes the invisible border around the frame.
                SetWindowPos(c.Src, IntPtr.Zero, 0, 0, p.W + (c.Win.R - c.Win.L) - (c.Frame.R - c.Frame.L), p.H + (c.Win.B - c.Win.T) - (c.Frame.B - c.Frame.T),
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
                // Its cell is placed anew at once, not after the wait a user's drag gets.
                c.MoveAt = now + 1;
                break;
        }
        pending.Add(p);
        CheckPending();
        if (pending.Count > 0 && !pendingTimer.Enabled) pendingTimer.Start();
    }

    void OnPendingTick(object sender, EventArgs e)
    {
        try { CheckPending(); } catch (Exception ex) { LogError(ex); }
    }

    void CheckPending()
    {
        long now = Now();
        foreach (Pending p in new List<Pending>(pending))
        {
            bool done = false, ok = false;
            string failure = "";
            IntPtr h = p.Win;
            if (!IsWindow(h)) { done = true; failure = "the window is gone"; }
            else switch (p.Kind)
            {
                case "minimize":
                    if (IsIconic(h)) { done = true; ok = true; }
                    else if (now >= p.NextTry) { ShowWindowAsync(h, SW_MINIMIZE); p.NextTry = now + RestoreRetryMs * 2; }
                    failure = "Windows did not minimize the window";
                    break;
                case "restore":
                    // A restore asked for during the minimize animation is ignored, so it is asked again until the window is up.
                    if (!IsIconic(h)) { done = true; ok = true; GiveFocus(h); }
                    else if (now >= p.NextTry) { ShowWindowAsync(h, SW_RESTORE); p.NextTry = now + RestoreRetryMs; }
                    failure = "Windows did not restore the window";
                    break;
                case "focus":
                    if (GetForegroundWindow() == h) { done = true; ok = true; }
                    else if (now >= p.NextTry) { GiveFocus(h); p.NextTry = now + FocusRetryMs; }
                    failure = "Windows kept the focus elsewhere";
                    break;
                case "resize":
                    RECT f;
                    if (DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, out f, 16) == 0 && f.R - f.L == p.W && f.B - f.T == p.H) { done = true; ok = true; }
                    failure = "the window did not take the size";
                    break;
            }
            if (!done && now >= p.Deadline) done = true;
            if (done) { pending.Remove(p); Answer(p.Link, p.N, ok, failure); }
        }
        if (pending.Count == 0) pendingTimer.Stop();
    }

    // SetForegroundWindow is refused to a program that did not get the last input, so this thread joins the input of the one that did.
    bool GiveFocus(IntPtr h)
    {
        if (IsIconic(h)) ShowWindowAsync(h, SW_RESTORE);
        uint pid;
        uint foregroundThread = GetWindowThreadProcessId(GetForegroundWindow(), out pid);
        uint mine = GetCurrentThreadId();
        bool joined = foregroundThread != 0 && foregroundThread != mine && AttachThreadInput(mine, foregroundThread, true);
        BringWindowToTop(h);
        SetForegroundWindow(h);
        if (joined) AttachThreadInput(mine, foregroundThread, false);
        return GetForegroundWindow() == h;
    }

    // WM_CLOSE, as the title bar's close button sends it. The program may ask to save, and then the cell stays: the answer says the request was sent.
    void CloseWindow(Link link, long n, Dictionary<string, object> r)
    {
        Cell c = CellOf(Str(r, "id"));
        if (c == null) { Answer(link, n, false, "no window has that id"); return; }
        if (PostMessage(c.Src, WM_CLOSE, IntPtr.Zero, IntPtr.Zero)) Answer(link, n, true, null);
        else Answer(link, n, false, "the window did not take the close message");
    }

    // Starts a program the way Explorer would, from a thread of its own, as that can take a moment.
    void Launch(Link link, long n, Dictionary<string, object> r)
    {
        string path = Str(r, "path"), args = Str(r, "args"), app = Str(r, "app");
        if (path.Length == 0 && app.Length > 0)
        {
            path = ShortcutOf(app);
            if (path == null) { Answer(link, n, false, "no program in the Start menu has that id"); return; }
        }
        if (path.Length == 0) { Answer(link, n, false, "launch needs a path or an app"); return; }
        ThreadPool.QueueUserWorkItem(delegate
        {
            try
            {
                ProcessStartInfo start = new ProcessStartInfo(path, args);
                start.UseShellExecute = true;
                string folder = Path.GetDirectoryName(path);
                if (!string.IsNullOrEmpty(folder) && Directory.Exists(folder)) start.WorkingDirectory = folder;
                Process.Start(start).Dispose();
                Answer(link, n, true, null);
            }
            catch (Exception ex) { Answer(link, n, false, ex.Message); }
        });
    }

    // ================================================================ The Start menu's programs
    const int IconSide = 48;
    const int AppsDepth = 6;
    // Shortcuts of this kind are in the list, marked minor, as nobody starts one to work.
    static readonly string[] MinorWords = new string[] { "uninstall", "help", "readme", "read me", "manual", "license", "release notes", "documentation" };

    static string[] StartMenuFolders()
    {
        return new string[] {
            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.CommonStartMenu), "Programs"),
            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.StartMenu), "Programs") };
    }

    // The id of a program is its shortcut's place in the Start menu, with forward slashes and no ".lnk"; the user's own copy wins over the shared one.
    static string ShortcutOf(string id)
    {
        if (id.IndexOf("..", StringComparison.Ordinal) >= 0 || Path.IsPathRooted(id)) return null;
        string[] folders = StartMenuFolders();
        for (int i = folders.Length - 1; i >= 0; i--)
        {
            string path = Path.Combine(folders[i], id.Replace('/', '\\') + ".lnk");
            if (File.Exists(path)) return path;
        }
        return null;
    }

    static string TargetOf(string shortcut)
    {
        try
        {
            Type shell = Type.GetTypeFromProgID("WScript.Shell");
            object sh = Activator.CreateInstance(shell);
            object link = shell.InvokeMember("CreateShortcut", BindingFlags.InvokeMethod, null, sh, new object[] { shortcut });
            return Convert.ToString(link.GetType().InvokeMember("TargetPath", BindingFlags.GetProperty, null, link, null), CultureInfo.InvariantCulture);
        }
        catch { return ""; }
    }

    static string IconOf(string shortcut)
    {
        try
        {
            using (Icon icon = Icon.ExtractAssociatedIcon(shortcut))
            using (Bitmap small = new Bitmap(IconSide, IconSide))
            using (Graphics g = Graphics.FromImage(small))
            using (MemoryStream png = new MemoryStream())
            {
                g.InterpolationMode = System.Drawing.Drawing2D.InterpolationMode.HighQualityBicubic;
                g.DrawImage(icon.ToBitmap(), 0, 0, IconSide, IconSide);
                small.Save(png, ImageFormat.Png);
                return Convert.ToBase64String(png.ToArray());
            }
        }
        catch { return ""; }
    }

    void Apps(Link link, long n, bool icons)
    {
        ThreadPool.QueueUserWorkItem(delegate
        {
            try
            {
                SortedDictionary<string, string> found = new SortedDictionary<string, string>(StringComparer.OrdinalIgnoreCase);
                foreach (string folder in StartMenuFolders())
                {
                    if (!Directory.Exists(folder)) continue;
                    foreach (string file in Directory.GetFiles(folder, "*.lnk", SearchOption.AllDirectories))
                    {
                        string id = file.Substring(folder.Length + 1);
                        id = id.Substring(0, id.Length - 4).Replace('\\', '/');
                        if (id.Split('/').Length > AppsDepth) continue;
                        found[id] = file;
                    }
                }
                StringBuilder sb = new StringBuilder("{\"t\":\"answer\",\"re\":" + L(n) + ",\"ok\":true,\"apps\":[");
                bool first = true;
                foreach (KeyValuePair<string, string> app in found)
                {
                    string name = Path.GetFileNameWithoutExtension(app.Value);
                    bool minor = false;
                    foreach (string word in MinorWords) if (name.IndexOf(word, StringComparison.OrdinalIgnoreCase) >= 0) minor = true;
                    if (!first) sb.Append(',');
                    first = false;
                    // The shortcut's own icon carries the arrow Windows puts on shortcuts, so the program's is taken where there is one.
                    string target = TargetOf(app.Value);
                    sb.Append("{\"id\":"); Js(sb, app.Key);
                    sb.Append(",\"name\":"); Js(sb, name);
                    sb.Append(",\"target\":"); Js(sb, target);
                    sb.Append(",\"icon\":"); Js(sb, icons ? IconOf(File.Exists(target) ? target : app.Value) : "");
                    sb.Append(",\"minor\":").Append(B(minor)).Append('}');
                }
                link.Send(sb.Append("]}").ToString());
            }
            catch (Exception ex) { Answer(link, n, false, ex.Message); }
        });
    }


    // ================================================================ Which program a window is of
    // `app` is the same for every window of a program and for every run of it: the program's full path, or
    // "aumid:" and the Application User Model ID of a packaged (Store) app. A Store app's window belongs to
    // ApplicationFrameHost.exe, the same process for all of them, so the program is the process of the
    // CoreWindow inside the frame. `name` is what the program calls itself, not the window's title.
    const uint PROCESS_QUERY_LIMITED_INFORMATION_ID = 0x1000;
    const int IdTriesMost = 3;                 // scans a frame host's window waits for its CoreWindow before it counts as the host's own
    Dictionary<string, string> programNames = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);

    static string FullPathOf(uint pid)
    {
        IntPtr process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION_ID, false, pid);
        if (process == IntPtr.Zero) return "";
        try
        {
            StringBuilder path = new StringBuilder(1024);
            int size = path.Capacity;
            return QueryFullProcessImageNameW(process, 0, path, ref size) ? path.ToString() : "";
        }
        finally { CloseHandle(process); }
    }

    // The ID of the package app the process belongs to, "" for an ordinary program.
    static string AumidOf(uint pid)
    {
        IntPtr process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION_ID, false, pid);
        if (process == IntPtr.Zero) return "";
        try
        {
            uint length = 512;
            StringBuilder id = new StringBuilder((int)length);
            return GetApplicationUserModelId(process, ref length, id) == 0 ? id.ToString() : "";
        }
        catch { return ""; }
        finally { CloseHandle(process); }
    }

    static string PackageFullNameOf(uint pid)
    {
        IntPtr process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION_ID, false, pid);
        if (process == IntPtr.Zero) return "";
        try
        {
            uint length = 256;
            StringBuilder name = new StringBuilder((int)length);
            return GetPackageFullName(process, ref length, name) == 0 ? name.ToString() : "";
        }
        catch { return ""; }
        finally { CloseHandle(process); }
    }

    static string PackageFolder(string fullName)
    {
        if (fullName.Length == 0) return "";
        try
        {
            uint length = 520;
            StringBuilder path = new StringBuilder((int)length);
            return GetPackagePathByFullName(fullName, ref length, path) == 0 ? path.ToString() : "";
        }
        catch { return ""; }
    }

    // The process of the CoreWindow inside a frame host's window, 0 while there is none.
    uint CoreWindowPid(IntPtr frame)
    {
        uint found = 0;
        EnumChildWindows(frame, delegate(IntPtr child, IntPtr l)
        {
            if (ClassOf(child) != "Windows.UI.Core.CoreWindow") return true;
            uint pid;
            GetWindowThreadProcessId(child, out pid);
            if (pid != 0) { found = pid; return false; }
            return true;
        }, IntPtr.Zero);
        return found;
    }

    void Identify(Cell c)
    {
        c.IdTries++;
        uint pid = c.Pid;
        bool host = string.Equals(c.Exe, "ApplicationFrameHost.exe", StringComparison.OrdinalIgnoreCase);
        if (host)
        {
            uint real = CoreWindowPid(c.Src);
            if (real != 0 && real != pid) pid = real;
            else if (c.IdTries <= IdTriesMost) return;      // the CoreWindow comes a moment after its frame: ask again at the next scan
        }
        string path = FullPathOf(pid);
        string aumid = AumidOf(pid);
        c.AppPid = pid; c.ExePath = path;
        c.App = aumid.Length > 0 ? "aumid:" + aumid : path;
        c.Name = NameOfProgram(c.App, path, pid);
        c.IdSettled = true;
    }

    string NameOfProgram(string app, string path, uint pid)
    {
        if (app.Length == 0) return "";
        string name;
        if (programNames.TryGetValue(app, out name)) return name;
        name = "";
        try
        {
            if (app.StartsWith("aumid:", StringComparison.OrdinalIgnoreCase)) name = PackageDisplayName(app.Substring(6), pid);
            if (name.Length == 0 && path.Length > 0)
            {
                FileVersionInfo info = FileVersionInfo.GetVersionInfo(path);
                name = (info.FileDescription ?? "").Trim();
                if (name.Length == 0) name = (info.ProductName ?? "").Trim();
                if (name.Length == 0) name = Path.GetFileNameWithoutExtension(path);
            }
        }
        catch { }
        if (name.Length > 100) name = name.Substring(0, 100);
        programNames[app] = name;
        return name;
    }

    // A package's own name for the app: its manifest's DisplayName, which is often "ms-resource:Name", read through the resource loader.
    string PackageDisplayName(string aumid, uint pid)
    {
        string fullName = PackageFullNameOf(pid);
        string folder = PackageFolder(fullName);
        if (folder.Length == 0) return "";
        string manifest = Path.Combine(folder, "AppxManifest.xml");
        if (!File.Exists(manifest)) return "";
        System.Xml.XmlDocument xml = new System.Xml.XmlDocument();
        xml.XmlResolver = null;
        xml.Load(manifest);
        string appId = aumid.IndexOf('!') >= 0 ? aumid.Substring(aumid.IndexOf('!') + 1) : "";
        string display = "", packageName = fullName.Split('_')[0];
        foreach (System.Xml.XmlNode node in xml.GetElementsByTagName("Application"))
        {
            System.Xml.XmlAttribute id = node.Attributes["Id"];
            if (appId.Length > 0 && (id == null || id.Value != appId)) continue;
            foreach (System.Xml.XmlNode child in node.ChildNodes)
                if (child.LocalName == "VisualElements" && child.Attributes["DisplayName"] != null) display = child.Attributes["DisplayName"].Value;
            break;
        }
        if (display.Length == 0)
            foreach (System.Xml.XmlNode node in xml.GetElementsByTagName("DisplayName"))
                if (node.ParentNode != null && node.ParentNode.LocalName == "Properties") display = node.InnerText;
        return ResolveResource(fullName, packageName, display);
    }

    static string ResolveResource(string fullName, string packageName, string text)
    {
        if (!text.StartsWith("ms-resource:", StringComparison.OrdinalIgnoreCase)) return text.Trim();
        string key = text.Substring(12);
        string[] tries = key.StartsWith("//") ? new string[] { "ms-resource:" + key }
            : new string[] { "ms-resource:" + key, "ms-resource://" + packageName + "/resources/" + key.TrimStart('/'), "ms-resource://" + packageName + "/" + key.TrimStart('/') };
        foreach (string candidate in tries)
        {
            StringBuilder output = new StringBuilder(256);
            try { if (SHLoadIndirectString("@{" + fullName + "? " + candidate + "}", output, 256, IntPtr.Zero) == 0 && output.Length > 0 && !output.ToString().StartsWith("ms-resource:")) return output.ToString().Trim(); }
            catch { }
        }
        return "";
    }

    // ================================================================ The icon of a window's program
    // Packaged app: the package's own logo. Else the program file's icon resource at the size wanted. Else
    // the window's own icon (the class icon or what it answers WM_GETICON with).
    void IconOf(Link link, long n, Dictionary<string, object> r)
    {
        Cell c = CellOf(Str(r, "id"));
        if (c == null) { Answer(link, n, false, "no window has that id"); return; }
        int side = (int)Math.Max(16, Math.Min(256, Num(r, "size", 64)));
        IntPtr window = c.Src; string path = c.ExePath; uint pid = c.AppPid != 0 ? c.AppPid : c.Pid;
        ThreadPool.QueueUserWorkItem(delegate
        {
            try
            {
                string b64 = null;
                string folder = PackageFolder(PackageFullNameOf(pid));
                using (Bitmap logo = folder.Length > 0 ? PackageLogo(folder, side) : null) { if (logo != null) b64 = PngBase64(logo, side); }
                if (b64 == null && path.Length > 0) b64 = IconFromFile(path, side);
                if (b64 == null) b64 = IconFromWindow(window, side);
                if (b64 == null) { Answer(link, n, false, "the program has no icon"); return; }
                StringBuilder sb = new StringBuilder("{\"t\":\"answer\",\"re\":" + L(n) + ",\"ok\":true,\"icon\":");
                Js(sb, b64);
                link.Send(sb.Append('}').ToString());
            }
            catch (Exception ex) { Answer(link, n, false, ex.Message); }
        });
    }

    // The logo files of the package's app in the manifest, the one nearest the size wanted at or above it.
    static Bitmap PackageLogo(string folder, int side)
    {
        string manifest = Path.Combine(folder, "AppxManifest.xml");
        if (!File.Exists(manifest)) return null;
        System.Xml.XmlDocument xml = new System.Xml.XmlDocument();
        xml.XmlResolver = null;
        xml.Load(manifest);
        List<string> logos = new List<string>();
        foreach (System.Xml.XmlNode node in xml.GetElementsByTagName("*"))
        {
            if (node.LocalName != "VisualElements" || node.Attributes == null) continue;
            foreach (string key in new string[] { "Square44x44Logo", "Square150x150Logo", "Logo" })
                if (node.Attributes[key] != null) logos.Add(node.Attributes[key].Value);
            break;
        }
        if (logos.Count == 0)
            foreach (System.Xml.XmlNode node in xml.GetElementsByTagName("Logo"))
                if (node.ParentNode != null && node.ParentNode.LocalName == "Properties") logos.Add(node.InnerText);
        foreach (string logo in logos)
        {
            string relative = logo.Replace('/', '\\');
            if (relative.Contains("..") || Path.IsPathRooted(relative)) continue;
            string full = Path.Combine(folder, relative);
            string dir = Path.GetDirectoryName(full), stem = Path.GetFileNameWithoutExtension(full);
            if (!Directory.Exists(dir)) continue;
            Bitmap best = null; int bestScore = int.MaxValue;
            foreach (string file in Directory.GetFiles(dir, stem + "*.png"))
            {
                string name = Path.GetFileName(file);
                string rest = name.Substring(stem.Length);
                if (rest.Length > 0 && rest[0] != '.' && rest[0] != '_') continue;
                try
                {
                    Bitmap candidate;
                    using (Bitmap raw = new Bitmap(file)) candidate = new Bitmap(raw);
                    int w = candidate.Width;
                    // nearest at or above the wanted size; unplated (no coloured square behind it) wins a tie
                    int score = (w >= side ? w - side : (side - w) * 4 + 1000) * 2 + (name.IndexOf("unplated", StringComparison.OrdinalIgnoreCase) >= 0 ? 0 : 1);
                    if (score < bestScore) { if (best != null) best.Dispose(); best = candidate; bestScore = score; }
                    else candidate.Dispose();
                }
                catch { }
            }
            if (best != null) return best;
        }
        return null;
    }

    static string IconFromFile(string path, int side)
    {
        IntPtr[] icons = new IntPtr[1]; int[] ids = new int[1];
        uint got;
        try { got = PrivateExtractIcons(path, 0, side, side, icons, ids, 1, 0); } catch { return null; }
        if (got == 0 || got == 0xFFFFFFFF || icons[0] == IntPtr.Zero) return null;
        try { return PngOfIcon(icons[0], side); }
        finally { DestroyIcon(icons[0]); }
    }

    static string IconFromWindow(IntPtr window, int side)
    {
        IntPtr icon;
        // ICON_BIG, then the class's, then ICON_SMALL2
        if (SendMessageTimeout(window, 0x7F, new IntPtr(1), IntPtr.Zero, 2, 150, out icon) == IntPtr.Zero) icon = IntPtr.Zero;
        if (icon == IntPtr.Zero) icon = GetClassLongPtr(window, -14);
        if (icon == IntPtr.Zero && SendMessageTimeout(window, 0x7F, new IntPtr(2), IntPtr.Zero, 2, 150, out icon) == IntPtr.Zero) icon = IntPtr.Zero;
        return icon == IntPtr.Zero ? null : PngOfIcon(icon, side);
    }

    // The icon is copied, so the handle stays whoever's it was.
    static string PngOfIcon(IntPtr handle, int side)
    {
        using (Icon icon = Icon.FromHandle(handle))
        using (Bitmap bitmap = icon.ToBitmap())
            return PngBase64(bitmap, side);
    }

    // The picture on a transparent square of the wanted side, kept in proportion.
    static string PngBase64(Bitmap source, int side)
    {
        using (Bitmap square = new Bitmap(side, side, PixelFormat.Format32bppArgb))
        using (Graphics g = Graphics.FromImage(square))
        using (MemoryStream png = new MemoryStream())
        {
            g.Clear(Color.Transparent);
            g.InterpolationMode = System.Drawing.Drawing2D.InterpolationMode.HighQualityBicubic;
            g.PixelOffsetMode = System.Drawing.Drawing2D.PixelOffsetMode.HighQuality;
            double k = Math.Min((double)side / source.Width, (double)side / source.Height);
            int w = Math.Max(1, (int)Math.Round(source.Width * k)), h = Math.Max(1, (int)Math.Round(source.Height * k));
            g.DrawImage(source, (side - w) / 2, (side - h) / 2, w, h);
            square.Save(png, ImageFormat.Png);
            return Convert.ToBase64String(png.ToArray());
        }
    }

    // ================================================================ Debug painting (debug=1 only)
    protected override void OnPaint(PaintEventArgs e)
    {
        base.OnPaint(e);
        if (!debugOn) return;
        foreach (Cell c in cells.Values)
        {
            if (!c.Active || !c.Placed) continue;
            e.Graphics.DrawString(c.Cls + " | " + c.Title, font, Brushes.White, c.X, c.Y - DebugLabelH);
            e.Graphics.DrawRectangle(Pens.Magenta, c.X - 1, c.Y - 1, c.CellW + 1, c.CellH + 1);
        }
    }

    // Runs until the form closes, which the stop script asks for with the quit file.
    public static void Run(string token)
    {
        SetProcessDpiAwarenessContext(new IntPtr(-4));      // per-monitor v2, before any window exists
        Application.EnableVisualStyles();
        Application.Run(new AppWindows(token));
    }
}

}
