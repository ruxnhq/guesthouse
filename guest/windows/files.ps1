# Runs in the user's session while a viewer is open, as only a program there can start a drag or see
# one. It takes files dropped on the viewer and starts a real Windows drag of them under the pointer,
# and it hands over the files of a drag that leaves the screen. It only answers the host, on QEMU's
# private network, and only requests that start with the agent's token.
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type -ReferencedAssemblies System.Windows.Forms, System.Drawing -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Drawing;
using System.IO;
using System.Net;
using System.Net.Sockets;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Windows.Forms;

namespace Guesthouse {

// A small window under the pointer that is on top of everything, never takes the focus and can't
// be seen. Windows only starts a drag from a window that was pressed, and only tells a window
// what is being dragged, so there has to be one.
public class QuietWindow : Form {
    const int NeverActive = 0x08000000, NoTaskbarButton = 0x80, OnTop = 0x8;
    const double AlmostClear = 0.02;

    [DllImport("user32.dll")] static extern IntPtr WindowFromPoint(Point point);
    [DllImport("dwmapi.dll")] static extern int DwmFlush();

    public QuietWindow(int size) {
        FormBorderStyle = FormBorderStyle.None;
        ShowInTaskbar = false;
        StartPosition = FormStartPosition.Manual;
        Size = new Size(size, size);
        Opacity = AlmostClear;
    }

    protected override bool ShowWithoutActivation { get { return true; } }

    protected override CreateParams CreateParams {
        get {
            CreateParams made = base.CreateParams;
            made.ExStyle |= NeverActive | NoTaskbarButton | OnTop;
            return made;
        }
    }

    public void ShowAtPointer() {
        Point pointer = Cursor.Position;
        Location = new Point(pointer.X - Width / 2, pointer.Y - Height / 2);
        Show();
        // A press goes through a window like this one until Windows has drawn it once, and it
        // draws a frame behind.
        Update();
        DwmFlush();
        DwmFlush();
    }

    // False on the lock screen or under a window Windows keeps above all others, where a press
    // would land on something else.
    public bool IsUnderPointer() {
        return WindowFromPoint(Cursor.Position) == Handle;
    }
}

// How far the drag of dropped files has got, in the words the host is told.
public static class Drag {
    public const string Idle = "idle", Offered = "shown", Going = "dragging";
    public const string Taken = "taken", OnDesktop = "desktop", Cancelled = "cancelled";
}

// Starts the drag of dropped files when the host presses on it.
public class Pad : QuietWindow {
    const int Side = 48;

    public volatile string State = Drag.Idle;
    string[] files;
    string folder;
    // The host's drag went away. Set from the listening thread.
    volatile bool abandoned;

    public Pad() : base(Side) { }

    public void Offer(string folder) {
        this.folder = folder;
        files = Directory.GetFileSystemEntries(folder);
        ShowAtPointer();
        State = Drag.Offered;
    }

    public void Abandon() { abandoned = true; }

    public void TakeBack() {
        if (State == Drag.Offered) {
            Hide();
            State = Drag.Idle;
        }
    }

    protected override void OnMouseDown(MouseEventArgs press) {
        if (press.Button != MouseButtons.Left || State != Drag.Offered) return;
        State = Drag.Going;
        abandoned = false;
        DataObject data = new DataObject(DataFormats.FileDrop, files);
        DragDropEffects effect = DoDragDrop(data, DragDropEffects.Copy | DragDropEffects.Move);
        Hide();
        if (effect != DragDropEffects.None) {
            // Explorer moves the files out, and whatever else took them reads them where they are.
            Dropped.RemoveIfEmpty(folder);
            State = Drag.Taken;
        } else if (abandoned) {
            Dropped.Remove(folder);
            State = Drag.Cancelled;
        } else {
            // Whatever was under the pointer doesn't take files, or runs as an administrator, which
            // Windows keeps a drag away from.
            Dropped.MoveToDesktop(folder);
            State = Drag.OnDesktop;
        }
    }

    protected override void OnQueryContinueDrag(QueryContinueDragEventArgs drag) {
        // Out of the way of what is under the pointer, now that the drag has its start.
        if (Visible) Hide();
        base.OnQueryContinueDrag(drag);
        // Windows asks this each time the pointer moves, which the host sees to. Escape would
        // not do: the key goes to the window with the focus, and this one never has it.
        if (abandoned) drag.Action = DragAction.Cancel;
    }
}

// Learns what a drag that is leaving the screen carries, without taking the drop.
public class Catcher : QuietWindow {
    const int Side = 96;

    public volatile string[] Caught;

    public Catcher() : base(Side) {
        AllowDrop = true;
    }

    protected override void OnDragEnter(DragEventArgs drag) {
        drag.Effect = DragDropEffects.None;
        string[] paths = drag.Data.GetData(DataFormats.FileDrop) as string[];
        if (paths != null && paths.Length > 0) Caught = paths;
    }

    protected override void OnDragOver(DragEventArgs drag) {
        drag.Effect = DragDropEffects.None;
    }
}

// Where dropped files wait for their drag: a folder for each drop, under the user's own.
public static class Dropped {
    static readonly TimeSpan Kept = TimeSpan.FromDays(1);

    static string Root {
        get {
            string local = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
            return Path.Combine(local, "Guesthouse", "Dropped");
        }
    }

    public static string Folder(string drop) {
        foreach (char letter in drop) {
            if (!char.IsLetterOrDigit(letter)) throw new ArgumentException("not a drop's name");
        }
        return Path.Combine(Root, drop);
    }

    // A name from the host, which uses / between folders. Nothing in it may lead out of the drop.
    public static string Inside(string drop, string name) {
        string path = Folder(drop);
        char[] refused = Path.GetInvalidFileNameChars();
        foreach (string part in name.Split('/')) {
            if (part.Length == 0 || part == "." || part == ".." || part.IndexOfAny(refused) >= 0) {
                throw new ArgumentException("not a file's name");
            }
            path = Path.Combine(path, part);
        }
        return path;
    }

    public static void Remove(string folder) {
        try { Directory.Delete(folder, true); } catch (IOException) { } catch (UnauthorizedAccessException) { }
    }

    public static void RemoveIfEmpty(string folder) {
        try { Directory.Delete(folder, false); } catch (IOException) { } catch (UnauthorizedAccessException) { }
    }

    // A program that was handed a file may read it long after the drop, a browser's upload for one,
    // so a drop is kept for a day and not removed when its drag ends.
    public static void RemoveOld() {
        if (!Directory.Exists(Root)) return;
        foreach (string folder in Directory.GetDirectories(Root)) {
            if (DateTime.UtcNow - Directory.GetCreationTimeUtc(folder) > Kept) Remove(folder);
        }
    }

    public static void MoveToDesktop(string folder) {
        string desktop = Environment.GetFolderPath(Environment.SpecialFolder.DesktopDirectory);
        foreach (string entry in Directory.GetFileSystemEntries(folder)) {
            string target = FreeName(desktop, Path.GetFileName(entry));
            if (Directory.Exists(entry)) Directory.Move(entry, target); else File.Move(entry, target);
        }
        Remove(folder);
    }

    static string FreeName(string folder, string name) {
        string target = Path.Combine(folder, name);
        string stem = Path.GetFileNameWithoutExtension(name), ending = Path.GetExtension(name);
        for (int copy = 2; File.Exists(target) || Directory.Exists(target); copy++) {
            target = Path.Combine(folder, stem + " (" + copy + ")" + ending);
        }
        return target;
    }
}

// What the last drag out of the screen carried: the host may ask for these files and no others.
public class Offer {
    const int MostEntries = 100000;

    readonly Dictionary<string, string> files = new Dictionary<string, string>();
    readonly StringBuilder listing = new StringBuilder();

    public Offer(string[] paths) {
        foreach (string path in paths) Add(path, FreeName(Path.GetFileName(path.TrimEnd('\\'))));
    }

    public string Listing { get { return listing.ToString(); } }

    public string PathOf(string name) {
        string path;
        return files.TryGetValue(name, out path) ? path : null;
    }

    string FreeName(string name) {
        string free = name;
        for (int copy = 2; files.ContainsKey(free); copy++) free = name + " (" + copy + ")";
        return free;
    }

    void Add(string path, string name) {
        if (files.Count >= MostEntries) return;
        FileAttributes kind;
        try { kind = File.GetAttributes(path); } catch (IOException) { return; } catch (UnauthorizedAccessException) { return; }
        // A link could lead anywhere, and round in circles.
        if ((kind & FileAttributes.ReparsePoint) != 0) return;
        files[name] = path;
        if ((kind & FileAttributes.Directory) == 0) {
            listing.Append("file " + new FileInfo(path).Length + " " + Wire.Pack(name) + "\n");
            return;
        }
        listing.Append("folder 0 " + Wire.Pack(name) + "\n");
        string[] entries;
        try { entries = Directory.GetFileSystemEntries(path); } catch (IOException) { return; } catch (UnauthorizedAccessException) { return; }
        foreach (string entry in entries) Add(entry, name + "/" + Path.GetFileName(entry));
    }
}

public static class Wire {
    const int MostLine = 64 << 10;

    // Names travel in Base64, as they can hold spaces and any letter.
    public static string Pack(string text) { return Convert.ToBase64String(Encoding.UTF8.GetBytes(text)); }
    public static string Unpack(string packed) { return Encoding.UTF8.GetString(Convert.FromBase64String(packed)); }

    // One byte at a time, as a file's bytes may follow the line.
    public static string ReadLine(Stream stream) {
        MemoryStream line = new MemoryStream();
        for (int next = stream.ReadByte(); next >= 0 && next != '\n'; next = stream.ReadByte()) {
            if (line.Length >= MostLine) throw new IOException("the request is too long");
            line.WriteByte((byte)next);
        }
        return Encoding.UTF8.GetString(line.ToArray());
    }

    public static void Write(Stream stream, string text) {
        byte[] bytes = Encoding.UTF8.GetBytes(text);
        stream.Write(bytes, 0, bytes.Length);
    }

    public static void Copy(Stream from, Stream to, long length) {
        byte[] buffer = new byte[1 << 20];
        while (length > 0) {
            int read = from.Read(buffer, 0, (int)Math.Min(buffer.Length, length));
            if (read <= 0) throw new EndOfStreamException();
            to.Write(buffer, 0, read);
            length -= read;
        }
    }
}

public class Helper {
    const int Port = 8767;
    const string HostAddress = "10.0.2.2";
    // What the host expects of this helper. An app that expects another starts its own.
    const string Version = "files 1";
    const int ReadMilliseconds = 30000, BetweenLooks = 10, BetweenNudges = 30;
    const int StillLooks = 4, MostLooks = 60, StaysAfterCatch = 700;

    [DllImport("user32.dll")] static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] static extern bool GetGUIThreadInfo(int thread, ref GuiThread info);

    [StructLayout(LayoutKind.Sequential)]
    struct GuiThread {
        public int Size, Flags;
        public IntPtr Active, Focus, Capture, MenuOwner, MoveSize, Caret;
        public int CaretLeft, CaretTop, CaretRight, CaretBottom;
    }
    const int MovingOrSizing = 0x2;

    // A window that is being moved or sized has the pointer, and it isn't a drag of files.
    static bool WindowIsBeingMoved() {
        GuiThread front = new GuiThread();
        front.Size = Marshal.SizeOf(typeof(GuiThread));
        return GetGUIThreadInfo(0, ref front) && (front.Flags & MovingOrSizing) != 0;
    }

    readonly string token;
    readonly Pad pad = new Pad();
    readonly Catcher catcher = new Catcher();
    volatile Offer offer;

    Helper(string token) { this.token = token; }

    public static void Run(string token) {
        // Windows then gives the pointer's place in the screen's own pixels, whatever its scale.
        SetProcessDPIAware();
        Dropped.RemoveOld();
        Helper helper = new Helper(token);
        // A window can only be asked to do something from another thread once Windows knows it.
        IntPtr made = helper.pad.Handle;
        made = helper.catcher.Handle;
        Thread listening = new Thread(helper.Listen);
        listening.IsBackground = true;
        listening.Start();
        Application.Run();
    }

    void Listen() {
        TcpListener listener = new TcpListener(IPAddress.Any, Port);
        listener.Start();
        while (true) {
            TcpClient client = listener.AcceptTcpClient();
            // A file can take a while, and the host asks how a drag is going meanwhile.
            ThreadPool.QueueUserWorkItem(delegate { Serve(client); });
        }
    }

    void Serve(TcpClient client) {
        try {
            if (((IPEndPoint)client.Client.RemoteEndPoint).Address.ToString() != HostAddress) return;
            NetworkStream stream = client.GetStream();
            stream.ReadTimeout = ReadMilliseconds;
            string[] words = Wire.ReadLine(stream).Split(' ');
            if (words.Length < 2 || words[0] != token) return;
            string answer;
            try { answer = Answer(words, stream); } catch (Exception problem) { answer = "failed " + problem.Message; }
            if (answer != null) Wire.Write(stream, answer + "\n");
        } catch (IOException) {
        } catch (SocketException) {
        } finally {
            client.Close();
        }
    }

    void OnWindows(MethodInvoker action) { pad.Invoke(action); }

    void TakeBackPad() {
        if (pad.State == Drag.Offered) OnWindows(pad.TakeBack);
    }

    // Null once the answer has gone out already.
    string Answer(string[] words, Stream stream) {
        switch (words[1]) {
            case "hello": return Version;
            case "put": return Put(words[2], Wire.Unpack(words[3]), long.Parse(words[4]), stream);
            case "folder":
                Directory.CreateDirectory(Dropped.Inside(words[2], Wire.Unpack(words[3])));
                return "ok";
            case "drag": return StartDrag(Dropped.Folder(words[2]));
            case "wait": return Wait(words[2], int.Parse(words[3]));
            case "keep":
                TakeBackPad();
                Dropped.MoveToDesktop(Dropped.Folder(words[2]));
                return Drag.OnDesktop;
            case "cancel":
                // A drag that has started ends when the pointer next moves, and removes its files.
                if (pad.State == Drag.Going) {
                    pad.Abandon();
                    return "ok";
                }
                TakeBackPad();
                Dropped.Remove(Dropped.Folder(words[2]));
                return "ok";
            case "catch": return Catch(int.Parse(words[2]));
            case "get": return Get(Wire.Unpack(words[2]), stream);
            default: return "unknown";
        }
    }

    string Put(string drop, string name, long length, Stream stream) {
        string path = Dropped.Inside(drop, name);
        Directory.CreateDirectory(Path.GetDirectoryName(path));
        using (FileStream file = new FileStream(path, FileMode.Create, FileAccess.Write)) {
            Wire.Copy(stream, file, length);
        }
        return "ok";
    }

    string StartDrag(string folder) {
        if (pad.State == Drag.Going) return "busy";
        if (!Directory.Exists(folder) || Directory.GetFileSystemEntries(folder).Length == 0) return "empty";
        bool reachable = false;
        WaitForPointer();
        OnWindows(delegate {
            pad.TakeBack();
            pad.Offer(folder);
            reachable = pad.IsUnderPointer();
            if (!reachable) pad.TakeBack();
        });
        return reachable ? Drag.Offered : "covered";
    }

    // The host's last moves may still be on their way through QEMU's tablet, and a window put
    // under a pointer that moves on would miss the press.
    static void WaitForPointer() {
        Point last = Cursor.Position;
        for (int looks = 0, still = 0; looks < MostLooks && still < StillLooks; looks++) {
            Thread.Sleep(BetweenLooks);
            Point now = Cursor.Position;
            still = now == last ? still + 1 : 0;
            last = now;
        }
    }

    // Answers once the drag has started, or once it has ended, or when the time is up.
    string Wait(string what, int milliseconds) {
        DateTime end = DateTime.UtcNow.AddMilliseconds(milliseconds);
        while (DateTime.UtcNow < end) {
            string state = pad.State;
            bool started = state != Drag.Offered;
            bool ended = started && state != Drag.Going;
            if (what == "started" ? started : ended) break;
            Thread.Sleep(BetweenLooks);
        }
        return pad.State;
    }

    // Windows tells a window of a drag when the pointer moves onto it, so the pointer is moved a
    // pixel to and fro until it has.
    string Catch(int milliseconds) {
        // While a drop waits for the user's answer, to Explorer's question about a file of the
        // same name for one, the windows can't be reached, and the pointer isn't dragging anything.
        if (pad.State == Drag.Going || WindowIsBeingMoved()) return "none";
        catcher.Caught = null;
        WaitForPointer();
        Point pointer = Cursor.Position;
        OnWindows(catcher.ShowAtPointer);
        DateTime end = DateTime.UtcNow.AddMilliseconds(milliseconds);
        for (int nudge = 0; catcher.Caught == null && DateTime.UtcNow < end; nudge++) {
            int aside = pointer.X > 0 ? -1 : 1;
            SetCursorPos(pointer.X + (nudge % 2 == 0 ? aside : 0), pointer.Y);
            Thread.Sleep(BetweenNudges);
        }
        SetCursorPos(pointer.X, pointer.Y);
        string[] caught = catcher.Caught;
        if (caught == null) {
            OnWindows(catcher.Hide);
            return "none";
        }
        // The host ends the drag now. Until it has, the window stays, as it refuses a drop that
        // whatever else is under the pointer might take.
        ThreadPool.QueueUserWorkItem(delegate {
            Thread.Sleep(StaysAfterCatch);
            OnWindows(catcher.Hide);
        });
        offer = new Offer(caught);
        return "files\n" + offer.Listing.TrimEnd();
    }

    string Get(string name, Stream stream) {
        Offer offered = offer;
        string path = offered == null ? null : offered.PathOf(name);
        if (path == null) return "gone";
        using (FileStream file = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete)) {
            Wire.Write(stream, "ok " + file.Length + "\n");
            Wire.Copy(file, stream, file.Length);
        }
        return null;
    }
}

}
'@
[Guesthouse.Helper]::Run((Get-Content C:\gpupv\token.txt -Raw).Trim())
