using System.ComponentModel;
using System.Diagnostics;
using System.Globalization;
using System.Runtime.InteropServices;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Interop;
using System.Windows.Media;
using System.Windows.Threading;
using PoeAlarm.App.Capture;

namespace PoeAlarm.App.Monitoring;

/// <summary>
/// A neutral, click-through heads-up display that proves monitoring is active without using
/// green/yellow attention signals. It is shown only while the monitor is running.
/// </summary>
internal sealed class MonitoringHudWindow : Window
{
    private const int GwlExStyle = -20;
    private const long WsExTransparent = 0x00000020L;
    private const long WsExToolWindow = 0x00000080L;
    private const long WsExNoActivate = 0x08000000L;
    private const uint MonitorDefaultToNearest = 2;
    private const uint SwpNoActivate = 0x0010;
    private const uint SwpShowWindow = 0x0040;
    private const uint WdaExcludeFromCapture = 0x00000011;
    private const int WmDisplayChange = 0x007E;
    private const int WmDpiChanged = 0x02E0;
    private static readonly IntPtr HwndTopmost = new(-1);

    private readonly TextBlock elapsedText;
    private readonly TextBlock targetText;
    private readonly DispatcherTimer timer;
    private readonly Stopwatch elapsed = new();
    private ScreenRegion anchorRegion;
    private HwndSource? source;
    private IntPtr handle;
    private bool ownerCloseRequested;
    private bool isMonitoring;

    public MonitoringHudWindow()
    {
        AllowsTransparency = true;
        Background = Brushes.Transparent;
        Focusable = false;
        IsHitTestVisible = false;
        Left = -10_000;
        ResizeMode = ResizeMode.NoResize;
        ShowActivated = false;
        ShowInTaskbar = false;
        SizeToContent = SizeToContent.WidthAndHeight;
        Top = -10_000;
        Topmost = true;
        WindowStartupLocation = WindowStartupLocation.Manual;
        WindowStyle = WindowStyle.None;

        var title = new TextBlock
        {
            FontFamily = new FontFamily("Segoe UI"),
            FontSize = 13,
            FontWeight = FontWeights.SemiBold,
            Foreground = new SolidColorBrush(Color.FromRgb(232, 233, 236)),
            Text = "POE Alarm · 监控中",
            VerticalAlignment = VerticalAlignment.Center,
        };

        elapsedText = new TextBlock
        {
            FontFamily = new FontFamily("Consolas"),
            FontSize = 12,
            Foreground = new SolidColorBrush(Color.FromRgb(174, 178, 186)),
            HorizontalAlignment = HorizontalAlignment.Right,
            Text = "00:00",
            VerticalAlignment = VerticalAlignment.Center,
        };

        var header = new Grid();
        header.ColumnDefinitions.Add(new ColumnDefinition());
        header.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        Grid.SetColumn(title, 0);
        Grid.SetColumn(elapsedText, 1);
        header.Children.Add(title);
        header.Children.Add(elapsedText);

        var targetLabel = new TextBlock
        {
            FontFamily = new FontFamily("Segoe UI"),
            FontSize = 11,
            Foreground = new SolidColorBrush(Color.FromRgb(139, 143, 151)),
            Margin = new Thickness(0, 9, 0, 2),
            Text = "当前目标",
        };

        targetText = new TextBlock
        {
            FontFamily = new FontFamily("Segoe UI"),
            FontSize = 12.5,
            Foreground = Brushes.White,
            LineHeight = 18,
            MaxHeight = 54,
            TextTrimming = TextTrimming.CharacterEllipsis,
            TextWrapping = TextWrapping.Wrap,
        };

        var stack = new StackPanel();
        stack.Children.Add(header);
        stack.Children.Add(targetLabel);
        stack.Children.Add(targetText);

        Content = new Border
        {
            Background = new SolidColorBrush(Color.FromArgb(232, 24, 26, 31)),
            BorderBrush = new SolidColorBrush(Color.FromArgb(180, 116, 121, 131)),
            BorderThickness = new Thickness(1),
            CornerRadius = new CornerRadius(7),
            Padding = new Thickness(13, 10, 13, 11),
            Width = 420,
            Child = stack,
        };

        timer = new DispatcherTimer(DispatcherPriority.Background)
        {
            Interval = TimeSpan.FromSeconds(1),
        };
        timer.Tick += OnTimerTick;
    }

    public void ShowMonitoring(string targetAffix, ScreenRegion region)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(targetAffix);
        if (!region.IsValid)
        {
            throw new ArgumentOutOfRangeException(nameof(region));
        }

        anchorRegion = region;
        targetText.Text = targetAffix.Trim();
        isMonitoring = true;
        elapsed.Restart();
        UpdateElapsedText();

        if (!IsVisible)
        {
            Show();
        }

        UpdateLayout();
        ReassertPosition();
        timer.Start();
    }

    public void HideMonitoring()
    {
        isMonitoring = false;
        timer.Stop();
        elapsed.Reset();
        if (IsVisible)
        {
            Hide();
        }
    }

    public void CloseFromOwner()
    {
        ownerCloseRequested = true;
        isMonitoring = false;
        timer.Stop();
        Close();
    }

    protected override void OnSourceInitialized(EventArgs e)
    {
        base.OnSourceInitialized(e);

        handle = new WindowInteropHelper(this).Handle;
        var extendedStyle = ComposeHudExtendedStyle(
            GetWindowLongPtr(handle, GwlExStyle).ToInt64());
        _ = SetWindowLongPtr(handle, GwlExStyle, new IntPtr(extendedStyle));
        _ = SetWindowDisplayAffinity(handle, WdaExcludeFromCapture);
        source = HwndSource.FromHwnd(handle);
        source?.AddHook(WindowProcedure);
    }

    protected override void OnClosing(CancelEventArgs e)
    {
        if (!ownerCloseRequested)
        {
            e.Cancel = true;
            HideMonitoring();
            return;
        }

        source?.RemoveHook(WindowProcedure);
        source = null;
        handle = IntPtr.Zero;
        base.OnClosing(e);
    }

    private void OnTimerTick(object? sender, EventArgs e)
    {
        if (!isMonitoring || !IsVisible)
        {
            return;
        }

        UpdateElapsedText();
        ReassertPosition();
    }

    private void UpdateElapsedText()
    {
        var format = elapsed.Elapsed.TotalHours >= 1 ? @"hh\:mm\:ss" : @"mm\:ss";
        elapsedText.Text = elapsed.Elapsed.ToString(format, CultureInfo.InvariantCulture);
    }

    private void ReassertPosition()
    {
        if (!isMonitoring || !IsVisible || handle == IntPtr.Zero ||
            source?.CompositionTarget is not { } compositionTarget)
        {
            return;
        }

        var anchor = new NativePoint(
            anchorRegion.X + (anchorRegion.Width / 2),
            anchorRegion.Y + (anchorRegion.Height / 2));
        var monitor = MonitorFromPoint(anchor, MonitorDefaultToNearest);
        var monitorInfo = new MonitorInfo
        {
            Size = Marshal.SizeOf<MonitorInfo>(),
        };
        if (monitor == IntPtr.Zero || !GetMonitorInfo(monitor, ref monitorInfo))
        {
            return;
        }

        var physicalSize = compositionTarget.TransformToDevice.Transform(
            new Vector(ActualWidth, ActualHeight));
        const int margin = 18;
        var width = (int)Math.Ceiling(physicalSize.X);
        var height = (int)Math.Ceiling(physicalSize.Y);
        var candidates = new[]
        {
            new NativeRect
            {
                Left = monitorInfo.WorkArea.Right - width - margin,
                Top = monitorInfo.WorkArea.Top + margin,
            },
            new NativeRect
            {
                Left = monitorInfo.WorkArea.Right - width - margin,
                Top = monitorInfo.WorkArea.Bottom - height - margin,
            },
            new NativeRect
            {
                Left = monitorInfo.WorkArea.Left + margin,
                Top = monitorInfo.WorkArea.Top + margin,
            },
            new NativeRect
            {
                Left = monitorInfo.WorkArea.Left + margin,
                Top = monitorInfo.WorkArea.Bottom - height - margin,
            },
        };
        for (var index = 0; index < candidates.Length; index++)
        {
            candidates[index].Right = candidates[index].Left + width;
            candidates[index].Bottom = candidates[index].Top + height;
        }

        var selected = candidates[0];
        foreach (var candidate in candidates)
        {
            if (!Intersects(candidate, anchorRegion))
            {
                selected = candidate;
                break;
            }
        }

        _ = SetWindowPos(
            handle,
            HwndTopmost,
            selected.Left,
            selected.Top,
            width,
            height,
            SwpNoActivate | SwpShowWindow);
    }

    private static bool Intersects(NativeRect window, ScreenRegion region) =>
        window.Left < region.X + region.Width &&
        window.Right > region.X &&
        window.Top < region.Y + region.Height &&
        window.Bottom > region.Y;

    private static long ComposeHudExtendedStyle(long extendedStyle) =>
        extendedStyle | WsExTransparent | WsExToolWindow | WsExNoActivate;

    private IntPtr WindowProcedure(
        IntPtr windowHandle,
        int message,
        IntPtr wParam,
        IntPtr lParam,
        ref bool handled)
    {
        if (message is WmDisplayChange or WmDpiChanged)
        {
            _ = Dispatcher.BeginInvoke(() =>
            {
                if (handle == windowHandle && source is not null && IsVisible)
                {
                    UpdateLayout();
                    ReassertPosition();
                }
            }, DispatcherPriority.Loaded);
        }

        return IntPtr.Zero;
    }

    private static IntPtr GetWindowLongPtr(IntPtr windowHandle, int index) =>
        IntPtr.Size == 8
            ? GetWindowLongPtr64(windowHandle, index)
            : new IntPtr(GetWindowLong32(windowHandle, index));

    private static IntPtr SetWindowLongPtr(IntPtr windowHandle, int index, IntPtr newValue) =>
        IntPtr.Size == 8
            ? SetWindowLongPtr64(windowHandle, index, newValue)
            : new IntPtr(SetWindowLong32(windowHandle, index, newValue.ToInt32()));

    [DllImport("user32.dll", EntryPoint = "GetWindowLongW")]
    private static extern int GetWindowLong32(IntPtr windowHandle, int index);

    [DllImport("user32.dll", EntryPoint = "GetWindowLongPtrW")]
    private static extern IntPtr GetWindowLongPtr64(IntPtr windowHandle, int index);

    [DllImport("user32.dll", EntryPoint = "SetWindowLongW")]
    private static extern int SetWindowLong32(IntPtr windowHandle, int index, int newValue);

    [DllImport("user32.dll", EntryPoint = "SetWindowLongPtrW")]
    private static extern IntPtr SetWindowLongPtr64(IntPtr windowHandle, int index, IntPtr newValue);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool SetWindowPos(
        IntPtr windowHandle,
        IntPtr insertAfter,
        int x,
        int y,
        int width,
        int height,
        uint flags);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool SetWindowDisplayAffinity(IntPtr windowHandle, uint affinity);

    [DllImport("user32.dll")]
    private static extern IntPtr MonitorFromPoint(NativePoint point, uint flags);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetMonitorInfo(IntPtr monitor, ref MonitorInfo monitorInfo);

    [StructLayout(LayoutKind.Sequential)]
    private readonly struct NativePoint
    {
        public NativePoint(int x, int y)
        {
            X = x;
            Y = y;
        }

        public readonly int X;
        public readonly int Y;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct NativeRect
    {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct MonitorInfo
    {
        public int Size;
        public NativeRect Monitor;
        public NativeRect WorkArea;
        public uint Flags;
    }
}
