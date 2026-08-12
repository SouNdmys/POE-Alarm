using System.ComponentModel;
using System.Diagnostics;
using System.Globalization;
using System.Runtime.InteropServices;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Interop;
using System.Windows.Media;
using System.Windows.Threading;
using PoeAlarm.App.Capture;
using PoeAlarm.App.Localization;

namespace PoeAlarm.App.Monitoring;

/// <summary>
/// Compact three-state HUD. Idle and monitoring modes never activate and are click-through at
/// both WPF and Win32 levels; placement mode temporarily accepts input so the user can drag it.
/// No animation runs here and the one-second timer is active only while monitoring.
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
    private const uint WdaNone = 0x00000000;
    private const uint WdaExcludeFromCapture = 0x00000011;
    private const int WmDisplayChange = 0x007E;
    private const int WmDpiChanged = 0x02E0;
    private static readonly IntPtr HwndTopmost = new(-1);

    private static readonly SolidColorBrush IdleBackground = Brush(246, 247, 248, 246);
    private static readonly SolidColorBrush IdleBorder = Brush(221, 225, 228);
    private static readonly SolidColorBrush IdleTitle = Brush(61, 69, 75);
    private static readonly SolidColorBrush IdleSecondary = Brush(113, 121, 127);
    private static readonly SolidColorBrush IdleDot = Brush(139, 147, 153);
    private static readonly SolidColorBrush MonitoringBackground = Brush(235, 249, 240, 246);
    private static readonly SolidColorBrush MonitoringBorder = Brush(111, 191, 139);
    private static readonly SolidColorBrush MonitoringTitle = Brush(26, 92, 53);
    private static readonly SolidColorBrush MonitoringSecondary = Brush(65, 126, 87);
    private static readonly SolidColorBrush MonitoringDot = Brush(38, 174, 87);
    private static readonly SolidColorBrush PlacementBackground = Brush(242, 250, 245, 250);
    private static readonly SolidColorBrush PlacementBorder = Brush(64, 151, 95);

    private readonly Border card;
    private readonly Border dragSurface;
    private readonly Border statusDot;
    private readonly TextBlock titleText;
    private readonly TextBlock elapsedText;
    private readonly TextBlock targetLabelText;
    private readonly TextBlock targetText;
    private readonly TextBlock detailText;
    private readonly Button finishPlacementButton;
    private readonly DispatcherTimer timer;
    private readonly Stopwatch elapsed = new();

    private HudPlacement placement;
    private ScreenRegion? anchorRegion;
    private HudState state;
    private HudState stateBeforePlacement;
    private HwndSource? source;
    private IntPtr handle;
    private bool ownerCloseRequested;
    private bool excludeFromCapture;

    public MonitoringHudWindow()
        : this(HudPlacement.Default, excludeFromCapture: false)
    {
    }

    public MonitoringHudWindow(HudPlacement? placement, bool excludeFromCapture)
    {
        this.placement = (placement ?? HudPlacement.Default).Sanitize();
        this.excludeFromCapture = excludeFromCapture;

        AllowsTransparency = true;
        Background = Brushes.Transparent;
        Focusable = false;
        IsHitTestVisible = false;
        Left = -10_000;
        ResizeMode = ResizeMode.NoResize;
        ShowActivated = false;
        ShowInTaskbar = false;
        SizeToContent = SizeToContent.Height;
        Top = -10_000;
        Topmost = true;
        Width = 336;
        WindowStartupLocation = WindowStartupLocation.Manual;
        WindowStyle = WindowStyle.None;

        titleText = new TextBlock
        {
            FontFamily = new FontFamily("Segoe UI Variable Text, Microsoft YaHei UI"),
            FontSize = 13,
            FontWeight = FontWeights.SemiBold,
            VerticalAlignment = VerticalAlignment.Center,
        };
        statusDot = new Border
        {
            CornerRadius = new CornerRadius(4),
            Height = 8,
            Margin = new Thickness(0, 0, 8, 0),
            VerticalAlignment = VerticalAlignment.Center,
            Width = 8,
        };
        elapsedText = new TextBlock
        {
            FontFamily = new FontFamily("Cascadia Mono, Consolas"),
            FontSize = 11.5,
            HorizontalAlignment = HorizontalAlignment.Right,
            Text = "00:00",
            VerticalAlignment = VerticalAlignment.Center,
        };

        var header = new Grid();
        header.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        header.ColumnDefinitions.Add(new ColumnDefinition());
        header.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        Grid.SetColumn(statusDot, 0);
        Grid.SetColumn(titleText, 1);
        Grid.SetColumn(elapsedText, 2);
        header.Children.Add(statusDot);
        header.Children.Add(titleText);
        header.Children.Add(elapsedText);

        dragSurface = new Border
        {
            Background = Brushes.Transparent,
            Child = header,
            Cursor = Cursors.Arrow,
        };
        dragSurface.MouseLeftButtonDown += OnDragSurfaceMouseLeftButtonDown;

        targetLabelText = new TextBlock
        {
            FontFamily = new FontFamily("Segoe UI Variable Text, Microsoft YaHei UI"),
            FontSize = 10.5,
            Margin = new Thickness(0, 9, 0, 2),
        };
        targetText = new TextBlock
        {
            FontFamily = new FontFamily("Segoe UI Variable Text, Microsoft YaHei UI"),
            FontSize = 12.2,
            Foreground = Brush(34, 42, 38),
            LineHeight = 17,
            MaxHeight = 51,
            TextTrimming = TextTrimming.CharacterEllipsis,
            TextWrapping = TextWrapping.Wrap,
        };
        detailText = new TextBlock
        {
            FontFamily = new FontFamily("Segoe UI Variable Text, Microsoft YaHei UI"),
            FontSize = 10.5,
            LineHeight = 15,
            Margin = new Thickness(0, 8, 0, 0),
            TextWrapping = TextWrapping.Wrap,
        };
        finishPlacementButton = new Button
        {
            Background = Brush(45, 135, 77),
            BorderBrush = Brushes.Transparent,
            BorderThickness = new Thickness(0),
            Content = "完成",
            Cursor = Cursors.Hand,
            Foreground = Brushes.White,
            FontFamily = new FontFamily("Segoe UI Variable Text, Microsoft YaHei UI"),
            FontSize = 11.5,
            FontWeight = FontWeights.SemiBold,
            Height = 30,
            HorizontalAlignment = HorizontalAlignment.Right,
            Margin = new Thickness(0, 10, 0, 0),
            Padding = new Thickness(16, 0, 16, 0),
            Visibility = Visibility.Collapsed,
        };
        finishPlacementButton.Click += (_, _) => EndPlacement();

        var stack = new StackPanel();
        stack.Children.Add(dragSurface);
        stack.Children.Add(targetLabelText);
        stack.Children.Add(targetText);
        stack.Children.Add(detailText);
        stack.Children.Add(finishPlacementButton);

        card = new Border
        {
            BorderThickness = new Thickness(1),
            CornerRadius = new CornerRadius(12),
            Padding = new Thickness(14, 11, 14, 12),
            Child = stack,
        };
        Content = card;

        timer = new DispatcherTimer(DispatcherPriority.Background)
        {
            Interval = TimeSpan.FromSeconds(1),
        };
        timer.Tick += OnTimerTick;
        SizeChanged += (_, _) =>
        {
            if (IsVisible && state != HudState.Placement)
            {
                ReassertPosition();
            }
        };

        state = HudState.Idle;
        ApplyLanguage();
        ApplyStateVisuals();
    }

    public event EventHandler<HudPlacementChangedEventArgs>? PlacementChanged;

    public HudPlacement Placement => placement;

    public void ShowIdle(string? targetAffix, ScreenRegion? region = null)
    {
        state = HudState.Idle;
        anchorRegion = region is { IsValid: true } valid ? valid : null;
        targetText.Text = NormalizeTarget(targetAffix);
        elapsed.Reset();
        timer.Stop();
        ApplyLanguage();
        ApplyStateVisuals();
        RestoreClickThrough();
        ShowAndPosition();
    }

    public void ShowMonitoring(string targetAffix, ScreenRegion region)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(targetAffix);
        if (!region.IsValid)
        {
            throw new ArgumentOutOfRangeException(nameof(region));
        }

        state = HudState.Monitoring;
        anchorRegion = region;
        targetText.Text = targetAffix.Trim();
        elapsed.Restart();
        UpdateElapsedText();
        ApplyLanguage();
        ApplyStateVisuals();
        RestoreClickThrough();
        ShowAndPosition();
        timer.Start();
    }

    public void BeginPlacement(string? targetAffix, ScreenRegion? region = null)
    {
        if (state != HudState.Placement)
        {
            stateBeforePlacement = state;
        }

        state = HudState.Placement;
        anchorRegion = region is { IsValid: true } valid ? valid : anchorRegion;
        targetText.Text = NormalizeTarget(targetAffix);
        timer.Stop();
        ApplyLanguage();
        ApplyStateVisuals();
        ShowAndPosition();
        EnablePlacementInteraction();
    }

    public HudPlacement EndPlacement()
    {
        if (state != HudState.Placement)
        {
            return placement;
        }

        placement = CapturePlacement().Sanitize();
        state = stateBeforePlacement == HudState.Monitoring
            ? HudState.Monitoring
            : HudState.Idle;
        ApplyLanguage();
        ApplyStateVisuals();
        RestoreClickThrough();
        if (state == HudState.Monitoring)
        {
            timer.Start();
        }

        PlacementChanged?.Invoke(this, new HudPlacementChangedEventArgs(placement));
        return placement;
    }

    public void HideHud()
    {
        timer.Stop();
        elapsed.Reset();
        if (state == HudState.Placement)
        {
            state = HudState.Idle;
            RestoreClickThrough();
        }

        if (IsVisible)
        {
            Hide();
        }
    }

    // Kept for compatibility with the 0.4.x owner while its HUD lifecycle is migrated.
    public void HideMonitoring() => HideHud();

    public void ApplyLanguage()
    {
        var strings = UiText.Current;
        var english = strings.LanguageCode.StartsWith("en", StringComparison.OrdinalIgnoreCase);
        targetLabelText.Text = strings.HudTargetLabel;
        finishPlacementButton.Content = english ? "Done" : "完成";
        (titleText.Text, detailText.Text) = state switch
        {
            HudState.Monitoring => (
                strings.HudTitle,
                english ? "Click-through · Monitoring stays active until a match or manual stop."
                        : "点击穿透 · 命中或手动停止前会持续监控。"),
            HudState.Placement => (
                english ? "POE Alarm · Position HUD" : "POE Alarm · 调整浮窗",
                english ? "Drag this title bar, then choose Done."
                        : "拖动标题栏调整位置，然后点击完成。"),
            _ => (
                english ? "POE Alarm · Not monitoring" : "POE Alarm · 未监控",
                english ? "After every successful block, choose Start monitoring for the next roll."
                        : "每次拦截成功后，下一轮都需重新点击“开始监控”。"),
        };
    }

    public void SetExcludeFromCapture(bool exclude)
    {
        excludeFromCapture = exclude;
        ApplyCaptureAffinity();
    }

    public void CloseFromOwner()
    {
        ownerCloseRequested = true;
        timer.Stop();
        Close();
    }

    protected override void OnSourceInitialized(EventArgs e)
    {
        base.OnSourceInitialized(e);
        handle = new WindowInteropHelper(this).Handle;
        source = HwndSource.FromHwnd(handle);
        source?.AddHook(WindowProcedure);
        RestoreClickThrough();
        ApplyCaptureAffinity();
    }

    protected override void OnClosing(CancelEventArgs e)
    {
        if (!ownerCloseRequested)
        {
            e.Cancel = true;
            HideHud();
            return;
        }

        source?.RemoveHook(WindowProcedure);
        source = null;
        handle = IntPtr.Zero;
        base.OnClosing(e);
    }

    private void ShowAndPosition()
    {
        if (!IsVisible)
        {
            Show();
        }

        UpdateLayout();
        ReassertPosition();
    }

    private void OnTimerTick(object? sender, EventArgs e)
    {
        if (state != HudState.Monitoring || !IsVisible)
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

    private void ApplyStateVisuals()
    {
        var (background, border, title, secondary, dot) = state switch
        {
            HudState.Monitoring => (
                MonitoringBackground, MonitoringBorder, MonitoringTitle,
                MonitoringSecondary, MonitoringDot),
            HudState.Placement => (
                PlacementBackground, PlacementBorder, MonitoringTitle,
                MonitoringSecondary, MonitoringDot),
            _ => (IdleBackground, IdleBorder, IdleTitle, IdleSecondary, IdleDot),
        };
        card.Background = background;
        card.BorderBrush = border;
        card.BorderThickness = state == HudState.Placement
            ? new Thickness(1.5)
            : new Thickness(1);
        titleText.Foreground = title;
        elapsedText.Foreground = secondary;
        targetLabelText.Foreground = secondary;
        detailText.Foreground = secondary;
        statusDot.Background = dot;
        elapsedText.Visibility = state == HudState.Monitoring
            ? Visibility.Visible
            : Visibility.Collapsed;
        finishPlacementButton.Visibility = state == HudState.Placement
            ? Visibility.Visible
            : Visibility.Collapsed;
        dragSurface.Cursor = state == HudState.Placement ? Cursors.SizeAll : Cursors.Arrow;
    }

    private void OnDragSurfaceMouseLeftButtonDown(object sender, MouseButtonEventArgs e)
    {
        if (state != HudState.Placement || e.LeftButton != MouseButtonState.Pressed)
        {
            return;
        }

        try
        {
            DragMove();
            placement = CapturePlacement().Sanitize();
        }
        catch (InvalidOperationException)
        {
            // The button may have been released between the routed event and DragMove.
        }
    }

    private void EnablePlacementInteraction()
    {
        Focusable = true;
        IsHitTestVisible = true;
        if (handle == IntPtr.Zero)
        {
            return;
        }

        var current = GetWindowLongPtr(handle, GwlExStyle).ToInt64();
        var interactive = (current | WsExToolWindow) & ~WsExTransparent & ~WsExNoActivate;
        _ = SetWindowLongPtr(handle, GwlExStyle, new IntPtr(interactive));
    }

    private void RestoreClickThrough()
    {
        Focusable = false;
        IsHitTestVisible = false;
        if (handle == IntPtr.Zero)
        {
            return;
        }

        var extendedStyle = ComposeHudExtendedStyle(
            GetWindowLongPtr(handle, GwlExStyle).ToInt64());
        _ = SetWindowLongPtr(handle, GwlExStyle, new IntPtr(extendedStyle));
    }

    private void ApplyCaptureAffinity()
    {
        if (handle != IntPtr.Zero)
        {
            _ = SetWindowDisplayAffinity(
                handle,
                excludeFromCapture ? WdaExcludeFromCapture : WdaNone);
        }
    }

    private void ReassertPosition()
    {
        if (!IsVisible || handle == IntPtr.Zero ||
            source?.CompositionTarget is not { } compositionTarget)
        {
            return;
        }

        var monitor = ResolveMonitor();
        if (!TryGetMonitorInfo(monitor, out var monitorInfo))
        {
            return;
        }

        var physicalSize = compositionTarget.TransformToDevice.Transform(
            new Vector(ActualWidth, ActualHeight));
        var width = Math.Max(1, (int)Math.Ceiling(physicalSize.X));
        var height = Math.Max(1, (int)Math.Ceiling(physicalSize.Y));
        var position = placement.IsManual
            ? ResolveManualPosition(monitorInfo.WorkArea, width, height)
            : ResolveAutomaticPosition(monitorInfo.WorkArea, width, height);

        _ = SetWindowPos(
            handle,
            HwndTopmost,
            position.X,
            position.Y,
            width,
            height,
            SwpNoActivate | SwpShowWindow);
    }

    private IntPtr ResolveMonitor()
    {
        if (placement.IsManual && !string.IsNullOrWhiteSpace(placement.MonitorDeviceName))
        {
            var stored = FindMonitor(placement.MonitorDeviceName);
            if (stored != IntPtr.Zero)
            {
                return stored;
            }
        }

        if (anchorRegion is { IsValid: true } region)
        {
            return MonitorFromPoint(
                new NativePoint(
                    region.X + (region.Width / 2),
                    region.Y + (region.Height / 2)),
                MonitorDefaultToNearest);
        }

        return MonitorFromWindow(handle, MonitorDefaultToNearest);
    }

    private NativePoint ResolveManualPosition(NativeRect workArea, int width, int height)
    {
        var availableWidth = Math.Max(0, workArea.Right - workArea.Left - width);
        var availableHeight = Math.Max(0, workArea.Bottom - workArea.Top - height);
        return new NativePoint(
            workArea.Left + (int)Math.Round(availableWidth * placement.RelativeX!.Value),
            workArea.Top + (int)Math.Round(availableHeight * placement.RelativeY!.Value));
    }

    private NativePoint ResolveAutomaticPosition(NativeRect workArea, int width, int height)
    {
        const int gap = 12;
        const int edgeMargin = 14;
        var minX = workArea.Left + edgeMargin;
        var maxX = Math.Max(minX, workArea.Right - width - edgeMargin);
        var minY = workArea.Top + edgeMargin;
        var maxY = Math.Max(minY, workArea.Bottom - height - edgeMargin);
        if (anchorRegion is { IsValid: true } region)
        {
            var candidates = new[]
            {
                new NativePoint(region.X + region.Width + gap, Math.Clamp(region.Y, minY, maxY)),
                new NativePoint(region.X - width - gap, Math.Clamp(region.Y, minY, maxY)),
                new NativePoint(Math.Clamp(region.X, minX, maxX), region.Y - height - gap),
                new NativePoint(Math.Clamp(region.X, minX, maxX), region.Y + region.Height + gap),
            };
            foreach (var candidate in candidates)
            {
                var rect = new NativeRect
                {
                    Left = candidate.X,
                    Top = candidate.Y,
                    Right = candidate.X + width,
                    Bottom = candidate.Y + height,
                };
                if (Contains(workArea, rect) && !Intersects(rect, region))
                {
                    return candidate;
                }
            }
        }

        var corners = new[]
        {
            new NativePoint(maxX, minY),
            new NativePoint(maxX, maxY),
            new NativePoint(minX, minY),
            new NativePoint(minX, maxY),
        };
        if (anchorRegion is not { IsValid: true } anchor)
        {
            return corners[0];
        }

        foreach (var corner in corners)
        {
            var rect = new NativeRect
            {
                Left = corner.X,
                Top = corner.Y,
                Right = corner.X + width,
                Bottom = corner.Y + height,
            };
            if (!Intersects(rect, anchor))
            {
                return corner;
            }
        }

        return corners[0];
    }

    private HudPlacement CapturePlacement()
    {
        if (handle == IntPtr.Zero || !GetWindowRect(handle, out var windowRect))
        {
            return placement;
        }

        var center = new NativePoint(
            windowRect.Left + ((windowRect.Right - windowRect.Left) / 2),
            windowRect.Top + ((windowRect.Bottom - windowRect.Top) / 2));
        var monitor = MonitorFromPoint(center, MonitorDefaultToNearest);
        if (!TryGetMonitorInfo(monitor, out var info))
        {
            return placement;
        }

        var width = windowRect.Right - windowRect.Left;
        var height = windowRect.Bottom - windowRect.Top;
        var availableWidth = Math.Max(0, info.WorkArea.Right - info.WorkArea.Left - width);
        var availableHeight = Math.Max(0, info.WorkArea.Bottom - info.WorkArea.Top - height);
        var x = availableWidth == 0
            ? 0
            : Math.Clamp(
                (double)(windowRect.Left - info.WorkArea.Left) / availableWidth,
                0,
                1);
        var y = availableHeight == 0
            ? 0
            : Math.Clamp(
                (double)(windowRect.Top - info.WorkArea.Top) / availableHeight,
                0,
                1);
        return HudPlacement.CreateManual(x, y, info.DeviceName);
    }

    private static IntPtr FindMonitor(string deviceName)
    {
        var result = IntPtr.Zero;
        MonitorEnumProcedure callback = (monitor, _, _, _) =>
        {
            if (TryGetMonitorInfo(monitor, out var info) &&
                string.Equals(info.DeviceName, deviceName, StringComparison.OrdinalIgnoreCase))
            {
                result = monitor;
                return false;
            }

            return true;
        };
        _ = EnumDisplayMonitors(IntPtr.Zero, IntPtr.Zero, callback, IntPtr.Zero);
        GC.KeepAlive(callback);
        return result;
    }

    private static bool TryGetMonitorInfo(IntPtr monitor, out MonitorInfoEx monitorInfo)
    {
        monitorInfo = new MonitorInfoEx
        {
            Size = Marshal.SizeOf<MonitorInfoEx>(),
            DeviceName = string.Empty,
        };
        return monitor != IntPtr.Zero && GetMonitorInfo(monitor, ref monitorInfo);
    }

    private static bool Contains(NativeRect outer, NativeRect inner) =>
        inner.Left >= outer.Left && inner.Top >= outer.Top &&
        inner.Right <= outer.Right && inner.Bottom <= outer.Bottom;

    private static bool Intersects(NativeRect window, ScreenRegion region) =>
        window.Left < region.X + region.Width &&
        window.Right > region.X &&
        window.Top < region.Y + region.Height &&
        window.Bottom > region.Y;

    private static string NormalizeTarget(string? targetAffix) =>
        string.IsNullOrWhiteSpace(targetAffix) ? UiText.Current.HudEmptyTarget : targetAffix.Trim();

    private static SolidColorBrush Brush(byte red, byte green, byte blue, byte alpha = 255)
    {
        var brush = new SolidColorBrush(Color.FromArgb(alpha, red, green, blue));
        brush.Freeze();
        return brush;
    }

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

    private enum HudState
    {
        Idle,
        Monitoring,
        Placement,
    }

    [StructLayout(LayoutKind.Sequential)]
    private readonly struct NativePoint(int x, int y)
    {
        public readonly int X = x;
        public readonly int Y = y;
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
    private struct MonitorInfoEx
    {
        public int Size;
        public NativeRect Monitor;
        public NativeRect WorkArea;
        public uint Flags;

        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)]
        public string DeviceName;
    }

    private delegate bool MonitorEnumProcedure(
        IntPtr monitor,
        IntPtr deviceContext,
        IntPtr monitorRectangle,
        IntPtr data);

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

    [DllImport("user32.dll")]
    private static extern IntPtr MonitorFromWindow(IntPtr windowHandle, uint flags);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetMonitorInfo(IntPtr monitor, ref MonitorInfoEx monitorInfo);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool EnumDisplayMonitors(
        IntPtr deviceContext,
        IntPtr clipRectangle,
        MonitorEnumProcedure callback,
        IntPtr data);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetWindowRect(IntPtr windowHandle, out NativeRect rectangle);
}

internal sealed class HudPlacementChangedEventArgs(HudPlacement placement) : EventArgs
{
    public HudPlacement Placement { get; } = placement;
}
