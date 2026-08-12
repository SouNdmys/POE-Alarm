using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Windows;
using System.Windows.Input;
using System.Windows.Interop;

namespace PoeAlarm.App.Input;

public sealed class GlobalHotKeyRegistration : IDisposable
{
    private const int WmHotKey = 0x0312;
    private readonly int _identifier;
    private readonly IntPtr _windowHandle;
    private readonly HwndSource _source;
    private bool _disposed;

    public GlobalHotKeyRegistration(Window owner, int identifier, ModifierKeys modifiers, Key key)
    {
        ArgumentNullException.ThrowIfNull(owner);

        _identifier = identifier;
        _windowHandle = new WindowInteropHelper(owner).Handle;
        if (_windowHandle == IntPtr.Zero)
        {
            throw new InvalidOperationException("The owner window handle has not been created yet.");
        }

        _source = HwndSource.FromHwnd(_windowHandle)
                  ?? throw new InvalidOperationException("Could not attach to the owner window message source.");
        _source.AddHook(WindowProcedure);

        var virtualKey = (uint)KeyInterop.VirtualKeyFromKey(key);
        if (!RegisterHotKey(_windowHandle, _identifier, (uint)modifiers | 0x4000U, virtualKey))
        {
            _source.RemoveHook(WindowProcedure);
            throw new Win32Exception(Marshal.GetLastWin32Error(), "The global hotkey is already in use.");
        }
    }

    public event EventHandler? Pressed;

    private IntPtr WindowProcedure(IntPtr hwnd, int message, IntPtr wParam, IntPtr lParam, ref bool handled)
    {
        if (message == WmHotKey && wParam.ToInt32() == _identifier)
        {
            handled = true;
            Pressed?.Invoke(this, EventArgs.Empty);
        }

        return IntPtr.Zero;
    }

    public void Dispose()
    {
        if (_disposed)
        {
            return;
        }

        _disposed = true;
        _ = UnregisterHotKey(_windowHandle, _identifier);
        _source.RemoveHook(WindowProcedure);
    }

    [DllImport("user32.dll", SetLastError = true)]
    private static extern bool RegisterHotKey(IntPtr windowHandle, int identifier, uint modifiers, uint virtualKey);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern bool UnregisterHotKey(IntPtr windowHandle, int identifier);
}
