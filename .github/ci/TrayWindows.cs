// Win32 helpers for the tray's Windows CI steps in workflows/ci.yml: find the
// tray's and the settings window's HWNDs and close the settings window.
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class TrayThreads {
    delegate bool EnumProc(IntPtr hwnd, IntPtr param);
    [DllImport("user32")] static extern bool EnumWindows(EnumProc cb, IntPtr param);
    [DllImport("user32")] static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32", CharSet = CharSet.Unicode)] static extern int GetClassNameW(IntPtr hwnd, StringBuilder name, int max);
    [DllImport("user32")] static extern IntPtr SendMessageTimeoutW(IntPtr hwnd, uint msg, IntPtr w, IntPtr l, uint flags, uint ms, out IntPtr result);
    public static IntPtr TrayWindow(int pid) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((h, p) => {
            uint owner; GetWindowThreadProcessId(h, out owner);
            var name = new StringBuilder(64); GetClassNameW(h, name, 64);
            if (owner == pid && name.ToString() == "tray_icon_app") { found = h; return false; }
            return true;
        }, IntPtr.Zero);
        return found;
    }
    [DllImport("user32")] static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32", CharSet = CharSet.Unicode)] static extern int GetWindowTextW(IntPtr hwnd, StringBuilder text, int max);
    // The tray's visible top-level window titled "Mini Launcher" (the settings window).
    public static IntPtr SettingsWindow(int pid) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((h, p) => {
            uint owner; GetWindowThreadProcessId(h, out owner);
            if (owner != pid || !IsWindowVisible(h)) return true;
            var title = new StringBuilder(64); GetWindowTextW(h, title, 64);
            if (title.ToString() == "Mini Launcher") { found = h; return false; }
            return true;
        }, IntPtr.Zero);
        return found;
    }
    public static string Describe(IntPtr hwnd) {
        var title = new StringBuilder(64); GetWindowTextW(hwnd, title, 64);
        var name = new StringBuilder(64); GetClassNameW(hwnd, name, 64);
        return string.Format("0x{0:X} \"{1}\" class {2}", hwnd.ToInt64(), title, name);
    }
    public static uint Thread(IntPtr hwnd) { uint pid; return GetWindowThreadProcessId(hwnd, out pid); }
    // WM_NULL with SMTO_ABORTIFHUNG; true if the window's thread answered within 2 s.
    public static bool Responds(IntPtr hwnd) { IntPtr r; return SendMessageTimeoutW(hwnd, 0, IntPtr.Zero, IntPtr.Zero, 2, 2000, out r) != IntPtr.Zero; }
    [DllImport("user32")] static extern bool PostMessageW(IntPtr hwnd, uint msg, IntPtr w, IntPtr l);
    // WM_CLOSE to this exact window (not whatever MainWindowHandle names now).
    public static bool Close(IntPtr hwnd) { return PostMessageW(hwnd, 0x0010, IntPtr.Zero, IntPtr.Zero); }
}
