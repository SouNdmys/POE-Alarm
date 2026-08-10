using System.ComponentModel;
using System.Runtime.InteropServices;

namespace PoeAlarm.App.Capture;

/// <summary>
/// Captures a small desktop region into a reusable, OCR-friendly BGRA buffer.
/// Native capture resources and the managed pixel array are reused between scans.
/// </summary>
public sealed class GdiScreenCapture : IScreenCapture
{
    private const int DibRgbColors = 0;
    private const int Srccopy = 0x00CC0020;
    private const int CaptureBlt = 0x40000000;

    private readonly object _captureGate = new();
    private IntPtr _screenDc;
    private IntPtr _memoryDc;
    private IntPtr _bitmap;
    private IntPtr _previousObject;
    private IntPtr _pixelAddress;
    private byte[] _pixels = [];
    private int _width;
    private int _height;
    private int _stride;
    private bool _disposed;

    public CapturedFrame Capture(ScreenRegion region)
    {
        if (!region.IsValid)
        {
            throw new ArgumentOutOfRangeException(nameof(region), "Capture region must have a positive size.");
        }

        lock (_captureGate)
        {
            ObjectDisposedException.ThrowIf(_disposed, this);
            EnsureResources(region.Width, region.Height);

            if (!BitBlt(
                    _memoryDc,
                    0,
                    0,
                    region.Width,
                    region.Height,
                    _screenDc,
                    region.X,
                    region.Y,
                    Srccopy | CaptureBlt))
            {
                var error = Marshal.GetLastWin32Error();
                ReleaseResources();
                throw new Win32Exception(error, "Desktop capture failed.");
            }

            Marshal.Copy(_pixelAddress, _pixels, 0, _pixels.Length);
            return new CapturedFrame(
                region.Width,
                region.Height,
                _stride,
                _pixels,
                DateTimeOffset.UtcNow);
        }
    }

    private void EnsureResources(int width, int height)
    {
        if (_bitmap != IntPtr.Zero && _width == width && _height == height)
        {
            return;
        }

        ReleaseResources();

        try
        {
            _screenDc = GetDC(IntPtr.Zero);
            if (_screenDc == IntPtr.Zero)
            {
                throw new Win32Exception(Marshal.GetLastWin32Error(), "Could not acquire the desktop device context.");
            }

            _memoryDc = CreateCompatibleDC(_screenDc);
            if (_memoryDc == IntPtr.Zero)
            {
                throw new Win32Exception(Marshal.GetLastWin32Error(), "Could not create a capture device context.");
            }

            _stride = checked(width * 4);
            var bitmapInfo = new BitmapInfo
            {
                Header = new BitmapInfoHeader
                {
                    Size = (uint)Marshal.SizeOf<BitmapInfoHeader>(),
                    Width = width,
                    Height = -height,
                    Planes = 1,
                    BitCount = 32,
                    Compression = 0,
                    SizeImage = (uint)checked(_stride * height),
                },
            };

            _bitmap = CreateDIBSection(
                _screenDc,
                ref bitmapInfo,
                DibRgbColors,
                out _pixelAddress,
                IntPtr.Zero,
                0);
            if (_bitmap == IntPtr.Zero || _pixelAddress == IntPtr.Zero)
            {
                throw new Win32Exception(Marshal.GetLastWin32Error(), "Could not allocate the capture bitmap.");
            }

            _previousObject = SelectObject(_memoryDc, _bitmap);
            if (_previousObject == IntPtr.Zero || _previousObject == new IntPtr(-1))
            {
                throw new Win32Exception(Marshal.GetLastWin32Error(), "Could not select the capture bitmap.");
            }

            _width = width;
            _height = height;
            _pixels = new byte[checked(_stride * height)];
        }
        catch
        {
            ReleaseResources();
            throw;
        }
    }

    public void Dispose()
    {
        lock (_captureGate)
        {
            if (_disposed)
            {
                return;
            }

            _disposed = true;
            ReleaseResources();
        }

        GC.SuppressFinalize(this);
    }

    private void ReleaseResources()
    {
        if (_previousObject != IntPtr.Zero && _previousObject != new IntPtr(-1) && _memoryDc != IntPtr.Zero)
        {
            _ = SelectObject(_memoryDc, _previousObject);
        }

        if (_bitmap != IntPtr.Zero)
        {
            _ = DeleteObject(_bitmap);
        }

        if (_memoryDc != IntPtr.Zero)
        {
            _ = DeleteDC(_memoryDc);
        }

        if (_screenDc != IntPtr.Zero)
        {
            _ = ReleaseDC(IntPtr.Zero, _screenDc);
        }

        _screenDc = IntPtr.Zero;
        _memoryDc = IntPtr.Zero;
        _bitmap = IntPtr.Zero;
        _previousObject = IntPtr.Zero;
        _pixelAddress = IntPtr.Zero;
        _pixels = [];
        _width = 0;
        _height = 0;
        _stride = 0;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct BitmapInfoHeader
    {
        public uint Size;
        public int Width;
        public int Height;
        public ushort Planes;
        public ushort BitCount;
        public uint Compression;
        public uint SizeImage;
        public int XPelsPerMeter;
        public int YPelsPerMeter;
        public uint ClrUsed;
        public uint ClrImportant;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct BitmapInfo
    {
        public BitmapInfoHeader Header;
        public uint Colors;
    }

    [DllImport("user32.dll", SetLastError = true)]
    private static extern IntPtr GetDC(IntPtr windowHandle);

    [DllImport("user32.dll")]
    private static extern int ReleaseDC(IntPtr windowHandle, IntPtr deviceContext);

    [DllImport("gdi32.dll", SetLastError = true)]
    private static extern IntPtr CreateCompatibleDC(IntPtr deviceContext);

    [DllImport("gdi32.dll", SetLastError = true)]
    private static extern bool DeleteDC(IntPtr deviceContext);

    [DllImport("gdi32.dll", SetLastError = true)]
    private static extern IntPtr CreateDIBSection(
        IntPtr deviceContext,
        ref BitmapInfo bitmapInfo,
        uint usage,
        out IntPtr bits,
        IntPtr section,
        uint offset);

    [DllImport("gdi32.dll", SetLastError = true)]
    private static extern IntPtr SelectObject(IntPtr deviceContext, IntPtr value);

    [DllImport("gdi32.dll", SetLastError = true)]
    private static extern bool DeleteObject(IntPtr value);

    [DllImport("gdi32.dll", SetLastError = true)]
    private static extern bool BitBlt(
        IntPtr destination,
        int destinationX,
        int destinationY,
        int width,
        int height,
        IntPtr source,
        int sourceX,
        int sourceY,
        int rasterOperation);
}
